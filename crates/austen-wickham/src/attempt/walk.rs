use austen_lib::gen_dsl::MarkedHeap;
use austen_lib::gen_dsl::choice::translate::{
    AsgnError, ConstraintError, MAX_CALL_DEPTH, MapDomainError, RootCallError,
    check_asgn_incoming_with_evaluator, check_constraint_with_evaluator, choice_bits,
    enter_root_with_evaluator, evaluated_write_choice_with_evaluator, index_scalar,
    map_candidate_range_with_evaluator, spent_rec_size,
};
use austen_lib::gen_dsl::{
    AttachError, Env, Evaluator, GenFailure, GenModule, GenTerm, GenTermKind, Incoming,
    LogicalConstraint, PrimitiveContext, PrimitiveDraw, PrimitiveGenerator, Region, Rng, Scope,
    TestCaseSize, Value,
};

use austen_lib::gen_dsl::RejectReason;

pub(crate) enum Stop {
    Reject(RejectReason),
    Hard(GenFailure),
}

pub(crate) struct Attempt<'a, 'e, R> {
    pub rng: &'a mut R,
    pub heap: MarkedHeap,
    pub incoming: &'a Incoming,
    pub writes: Vec<Region>,
    pub initial_size: TestCaseSize,
    pub primitive: &'a dyn PrimitiveGenerator,
    pub deadline: Option<std::time::Instant>,
    pub ctx: &'a GenModule,
    pub site_index: &'a austen_lib::gen_dsl::SiteIndex,
    pub sites: &'a mut austen_lib::gen_dsl::SiteTally,
    pub evaluator: &'a Evaluator<'e>,
}

pub(crate) fn walk_root<'a, R: Rng>(
    att: &mut Attempt<'a, '_, R>,
    term: &'a GenTerm,
    scope: &mut Scope,
) -> Result<Option<Value>, Stop> {
    match enter_root_with_evaluator(term, att.ctx, att.incoming, att.evaluator) {
        Ok((body, mut env)) => walk(att, 0, body, scope, &mut env),
        Err(RootCallError::Structural(error)) => Err(Stop::Hard(error.into())),
        Err(RootCallError::CallArg(_)) => Err(Stop::Reject(RejectReason::CallArg)),
        Err(RootCallError::RecursionBudget { .. }) => {
            Err(Stop::Reject(RejectReason::RecursionBudget))
        }
    }
}

pub(crate) fn walk<'a, R: Rng>(
    att: &mut Attempt<'a, '_, R>,
    depth: usize,
    term: &'a GenTerm,
    scope: &mut Scope,
    env: &mut Env,
) -> Result<Option<Value>, Stop> {
    if att
        .deadline
        .is_some_and(|at| std::time::Instant::now() >= at)
    {
        return Err(Stop::Reject(RejectReason::GeneratorTimeout));
    }
    match &term.term {
        GenTermKind::Arbitrary => {
            choice_bits(&term.bt).map_err(Stop::Hard)?;
            let mut context = PrimitiveContext {
                rng: &mut *att.rng,
                size: att.initial_size,
                heap: &mut att.heap,
                heap_bounds: att.incoming.heap(),
                declarations: att.ctx.declarations(),
            };
            match att
                .primitive
                .arbitrary(&term.bt, &mut context)
                .map_err(Stop::Hard)?
            {
                PrimitiveDraw::Value(value) => Ok(Some(value)),
                PrimitiveDraw::Rejected(reason) => Err(Stop::Reject(reason)),
            }
        }
        GenTermKind::WriteChoice { label, value, rest } => {
            let Some(choice) =
                evaluated_write_choice_with_evaluator(value, env, att.ctx, att.evaluator)
                    .map_err(Stop::Hard)?
            else {
                return Err(Stop::Reject(RejectReason::AssertUnevaluatable));
            };
            scope.insert(*label, choice);
            walk(att, depth, rest, scope, env)
        }
        GenTermKind::LetStar { var, value, rest } => {
            if let Some(v) = walk(att, depth, value, scope, env)? {
                env.insert(var, v);
            }
            walk(att, depth, rest, scope, env)
        }
        GenTermKind::Return(it) => Ok(att.evaluator.eval(it, env)),
        GenTermKind::Assert { constraint, rest } => {
            let verdict = check_constraint_with_evaluator(constraint, env, att.ctx, att.evaluator);
            att.site_index.observe(att.sites, term, verdict.is_ok());
            match verdict {
                Ok(()) => walk(att, depth, rest, scope, env),
                Err(ConstraintError::Failed) => Err(Stop::Reject(RejectReason::AssertFailed)),
                Err(ConstraintError::Unevaluatable) => Err(Stop::Reject(match constraint {
                    LogicalConstraint::Forall { .. } => RejectReason::AssertForall,
                    LogicalConstraint::T(_) => RejectReason::AssertUnevaluatable,
                })),
            }
        }
        GenTermKind::Scope { name, body } => {
            let key = name.eval_key_with_evaluator(env, att.evaluator);
            let mut child = Scope::new();
            let v = walk(att, depth, body, &mut child, env)?;
            scope.try_attach(key, child).map_err(|error| match error {
                AttachError::MissingKey => Stop::Reject(RejectReason::ScopeKey),
                AttachError::Merge(error) => Stop::Hard(error.into()),
            })?;
            Ok(v)
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            let Some(c) = att.evaluator.eval(cond, env).and_then(|v| v.as_bool()) else {
                return Err(Stop::Reject(RejectReason::IteCond));
            };
            let arm = if c { then_ } else { else_ };
            walk(att, depth, arm, scope, env)
        }
        GenTermKind::Call { fsym, args } => {
            let ctx = att.ctx;
            let def = ctx
                .resolve_call(fsym, args.len())
                .map_err(|error| Stop::Hard(error.into()))?;
            if depth >= MAX_CALL_DEPTH {
                return Err(Stop::Reject(RejectReason::CallDepth));
            }
            let mut callee_env = Env::new();
            for ((param, _), arg) in def.iargs.iter().zip(args) {
                let Some(v) = att.evaluator.eval(arg, env) else {
                    return Err(Stop::Reject(RejectReason::CallArg));
                };
                callee_env.insert(param, v);
            }
            if spent_rec_size(def, &callee_env) {
                return Err(Stop::Reject(RejectReason::RecursionBudget));
            }
            walk(att, depth + 1, &def.body, scope, &mut callee_env)
        }
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => {
            let candidates = match map_candidate_range_with_evaluator(
                perm,
                var,
                var_bt,
                env,
                att.ctx,
                att.evaluator,
            ) {
                Ok(candidates) => candidates,
                Err(MapDomainError::Unsupported) => {
                    return Err(Stop::Hard(GenFailure::UnsupportedMapDomain));
                }
                Err(MapDomainError::Unevaluatable | MapDomainError::TooLarge { .. }) => {
                    return Err(Stop::Reject(RejectReason::MapBound));
                }
            };
            let mut entries = Vec::new();
            for i in candidates.range.clone() {
                let v = env.scoped(var, index_scalar(var_bt, i), |env| {
                    match candidates.holds_at_with_evaluator(perm, env, att.ctx, att.evaluator) {
                        Ok(true) => walk(att, depth, body, scope, env),
                        Ok(false) => Ok(None),
                        Err(_) => Err(Stop::Reject(RejectReason::MapBound)),
                    }
                })?;
                if let Some(v) = v {
                    entries.push((i, v));
                }
            }
            Ok(Some(Value::iteration_map(entries)))
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => {
            let verdict = check_asgn_incoming_with_evaluator(
                addr,
                ct,
                value,
                env,
                att.incoming,
                &mut att.writes,
                att.ctx,
                att.evaluator,
            );
            if !matches!(verdict, Err(AsgnError::UnsizedCtype { .. })) {
                att.site_index.observe(att.sites, term, verdict.is_ok());
            }
            match verdict {
                Ok(_) => walk(att, depth, rest, scope, env),
                Err(AsgnError::UnsizedCtype { ct }) => {
                    Err(Stop::Hard(GenFailure::UnsizedCtype { ct }))
                }
                Err(AsgnError::Unevaluatable { .. }) => {
                    Err(Stop::Reject(RejectReason::AsgnUnevaluatable))
                }
                Err(AsgnError::OutOfBounds { .. }) => {
                    Err(Stop::Reject(RejectReason::AsgnOutOfBounds))
                }
                Err(AsgnError::Overlap { .. }) => Err(Stop::Reject(RejectReason::AsgnOverlap)),
            }
        }
    }
}
