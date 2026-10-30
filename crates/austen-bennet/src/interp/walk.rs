use austen_lib::gen_dsl::choice::translate::{
    ConstraintError, MAX_CALL_DEPTH, MapCandidates, MapDomainError,
    check_constraint_with_evaluator, evaluated_write_choice_with_evaluator, index_scalar,
    map_candidate_range_with_evaluator, spent_rec_size,
};
use austen_lib::gen_dsl::{GenDef, IndexTerm, LogicalConstraint, ScopeKey, ScopeName};

use super::*;

enum Step<'a> {
    Visit(&'a GenTerm, Option<Value>),
    Done(Option<Value>),
}

enum Frame<'a, M> {
    BindValue(Binder<'a, M>),
    BindRest {
        binder: Binder<'a, M>,
        after_draw: M,
    },
    Scope {
        term: &'a GenTerm,
        name: &'a ScopeName,
        key: Option<ScopeKey>,
        parent: Trie,
    },
    Ite {
        term: &'a GenTerm,
    },
    Call {
        term: &'a GenTerm,
        def: &'a GenDef,
        args: &'a [IndexTerm],
        caller: Env,
    },
    Map {
        map: Map<'a>,
        index: i128,
        env: EnvMark,
    },
}

impl<'a, M> Frame<'a, M> {
    fn term(&self) -> &'a GenTerm {
        match self {
            Self::BindValue(binder) | Self::BindRest { binder, .. } => binder.term,
            Self::Scope { term, .. } | Self::Ite { term, .. } | Self::Call { term, .. } => term,
            Self::Map { map, .. } => map.term,
        }
    }
}

struct Binder<'a, M> {
    term: &'a GenTerm,
    var: &'a Sym,
    value: &'a GenTerm,
    rest: &'a GenTerm,
    checkpoint: Checkpoint,
    budget: u32,
    deterministic: bool,
    persistent: Option<PointerReq>,
    working: Option<PointerReq>,
    replay_from: Option<M>,
}

impl<M: Copy> Binder<'_, M> {
    fn retry<R: Rng + Rewind<Mark = M>>(
        &mut self,
        att: &mut Attempt<'_, '_, R>,
        trie: &mut Trie,
        env: &mut Env,
        failure: Failure,
        after_draw: M,
    ) -> Result<Option<Value>, Stop> {
        if !failure.blames(self.var) {
            return Err(Stop::Fail(failure));
        }
        if failure.kind != FailureKind::Assign && self.budget == 0 {
            return Err(exhausted(
                att,
                failure,
                self.var,
                self.value,
                self.deterministic,
            ));
        }
        let mut progressed = false;
        if failure.kind == FailureKind::Assign
            && let Some(learned) = failure.requirement_of(self.var)
        {
            let mut met = self.working.map_or(learned, |cur| cur.meet(learned));

            if met.has_ownership()
                && let BaseType::Loc(Some(pointee)) = &self.value.bt
            {
                let alignment = good_pointer_alignment(pointee, att.ctx.declarations().structs())
                    .map_err(|_| {
                    Stop::Hard(GenFailure::UnsizedCtype {
                        ct: (**pointee).clone(),
                    })
                })?;
                met = met.meet(PointerReq::aligned(alignment));
            }

            if !met.has_congruence_solution() {
                return Err(Stop::Fail(failure));
            }
            if met.ownership.is_bottom(att.capacity) {
                att.reject(RejectReason::RepairCapacity);
                return Err(Stop::Fail(failure));
            }
            progressed = Some(met) != self.working;
            self.working = Some(met);
            self.persistent = self.working;
        }
        if !progressed {
            if self.budget == 0 {
                return Err(exhausted(
                    att,
                    failure,
                    self.var,
                    self.value,
                    self.deterministic,
                ));
            }
            self.budget -= 1;
        }
        self.checkpoint.restore(att, trie, env);
        let correction = if matches!(&self.value.term, GenTermKind::Arbitrary)
            && address::supported(&self.value.bt)
            && let Some(requirement) = self
                .working
                .filter(|requirement| requirement.has_ownership())
        {
            match plan_pointer_repair(att, requirement) {
                Ok(address) => Some(address::carrier(address, &self.value.bt)),
                Err(RepairPlanError::Capacity) => {
                    att.reject(RejectReason::RepairCapacity);
                    return Err(Stop::Fail(failure));
                }
                Err(RepairPlanError::Congruence) => {
                    return Err(Stop::Fail(failure));
                }
            }
        } else {
            None
        };
        self.replay_from = (failure.kind == FailureKind::Assign && correction.is_some()
            || (!failure.young && self.persistent != self.working))
            .then_some(after_draw);
        Ok(correction)
    }
}

struct Map<'a> {
    term: &'a GenTerm,
    var: &'a Sym,
    var_bt: &'a BaseType,
    perm: &'a IndexTerm,
    body: &'a GenTerm,
    candidates: MapCandidates,
    entries: Vec<(i128, Value)>,
}

impl<'a> Map<'a> {
    fn next<R: Rng + Rewind>(
        mut self,
        att: &mut Attempt<'a, '_, R>,
        frames: &mut Vec<Frame<'a, R::Mark>>,
        env: &mut Env,
    ) -> Result<Step<'a>, Stop> {
        while let Some(index) = self.candidates.range.next() {
            att.check_stop()?;
            let mark = env.mark();
            env.insert(self.var, index_scalar(self.var_bt, index));
            match self
                .candidates
                .holds_at_with_evaluator(self.perm, env, att.ctx, att.evaluator)
            {
                Ok(true) => {
                    let body = self.body;
                    frames.push(Frame::Map {
                        map: self,
                        index,
                        env: mark,
                    });
                    return Ok(Step::Visit(body, None));
                }
                Ok(false) => env.rewind(mark),
                Err(_) => {
                    let failure = att.fail(
                        FailureKind::Assert,
                        RejectReason::MapBound,
                        self.perm.free_vars(),
                    );
                    env.rewind(mark);
                    return Err(map_failure(failure, self.var, self.perm));
                }
            }
        }
        Ok(Step::Done(Some(Value::iteration_map(self.entries))))
    }
}

fn map_failure(stop: Stop, var: &Sym, perm: &IndexTerm) -> Stop {
    match stop {
        Stop::Fail(mut failure) => {
            failure.retarget(var, perm.free_vars());
            Stop::Fail(failure)
        }
        hard => hard,
    }
}

pub(super) fn walk<'a, R: Rng + Rewind>(
    att: &mut Attempt<'a, '_, R>,
    term: &'a GenTerm,
    trie: &mut Trie,
    env: &mut Env,
) -> Result<Option<Value>, Stop> {
    let previous = att.current_term;
    let mut depth = 0;
    let mut frames = Vec::new();
    let mut next = Ok(Step::Visit(term, None));
    loop {
        let out = match next {
            Ok(Step::Visit(term, repair)) => {
                att.current_term = Some(term);
                next = att
                    .step()
                    .and_then(|()| visit(att, &mut depth, term, repair, &mut frames, trie, env));
                continue;
            }
            Ok(Step::Done(value)) => Ok(value),
            Err(stop) => Err(stop),
        };
        let Some(frame) = frames.pop() else {
            att.current_term = previous;
            return out;
        };
        att.current_term = Some(frame.term());
        next = resume(att, &mut depth, frame, out, &mut frames, trie, env);
    }
}

fn active_path_vars<M>(frames: &[Frame<'_, M>]) -> SymSet {
    let mut vars = SymSet::new();
    for frame in frames.iter().rev() {
        match frame {
            Frame::Call { .. } => break,
            Frame::Ite { term } => {
                if let GenTermKind::Ite { cond, .. } = &term.term {
                    vars.extend(cond.free_vars());
                }
            }
            _ => {}
        }
    }
    vars
}

fn visit<'a, R: Rng + Rewind>(
    att: &mut Attempt<'a, '_, R>,
    depth: &mut usize,
    term: &'a GenTerm,
    correction: Option<Value>,
    frames: &mut Vec<Frame<'a, R::Mark>>,
    trie: &mut Trie,
    env: &mut Env,
) -> Result<Step<'a>, Stop> {
    match &term.term {
        GenTermKind::Arbitrary => draw(att, term, correction).map(Step::Done),
        GenTermKind::Return(it) => Ok(Step::Done(att.evaluator.eval(it, env))),
        GenTermKind::WriteChoice { label, value, rest } => {
            let Some(choice) =
                evaluated_write_choice_with_evaluator(value, env, att.ctx, att.evaluator)
                    .map_err(Stop::Hard)?
            else {
                return Err(att.fail(
                    FailureKind::Assert,
                    RejectReason::AssertUnevaluatable,
                    value.free_vars(),
                ));
            };
            trie.insert(*label, choice);
            Ok(Step::Visit(rest, None))
        }
        GenTermKind::LetStar { var, value, rest } => {
            let deterministic = matches!(value.term, GenTermKind::Return(_));
            frames.push(Frame::BindValue(Binder {
                term,
                var,
                value,
                rest,
                checkpoint: Checkpoint::save(att, trie, env),
                budget: if deterministic { 0 } else { att.max_backtracks },
                deterministic,
                persistent: None,
                working: None,
                replay_from: None,
            }));
            Ok(Step::Visit(value, None))
        }
        GenTermKind::Assert { constraint, rest } => {
            match check_constraint_with_evaluator(constraint, env, att.ctx, att.evaluator) {
                Ok(()) => {
                    att.site_index.observe(att.sites, term, true);
                    Ok(Step::Visit(rest, None))
                }
                Err(ConstraintError::Failed) => {
                    if let Some(failure) =
                        failed_pointer_good(constraint, env, att.ctx, att.evaluator, att.capacity)
                    {
                        att.reject(RejectReason::AsgnOutOfBounds);
                        Err(Stop::Fail(failure))
                    } else {
                        Err(att.fail(
                            FailureKind::Assert,
                            RejectReason::AssertFailed,
                            constraint.free_vars_bts().into_keys(),
                        ))
                    }
                }
                Err(ConstraintError::Unevaluatable) => Err(att.fail_unblamed(
                    FailureKind::Assert,
                    match constraint {
                        LogicalConstraint::Forall { .. } => RejectReason::AssertForall,
                        LogicalConstraint::T(_) => RejectReason::AssertUnevaluatable,
                    },
                )),
            }
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => {
            assign(att, term, addr, ct, value, env)?;
            Ok(Step::Visit(rest, None))
        }
        GenTermKind::Scope { name, body } => {
            let key = name.eval_key_with_evaluator(env, att.evaluator);
            let parent = std::mem::take(trie);
            frames.push(Frame::Scope {
                term,
                name,
                key,
                parent,
            });
            Ok(Step::Visit(body, None))
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            let Some(c) = att.evaluator.eval(cond, env).and_then(|v| v.as_bool()) else {
                return Err(att.fail(FailureKind::Assert, RejectReason::IteCond, cond.free_vars()));
            };
            frames.push(Frame::Ite { term });
            Ok(Step::Visit(if c { then_ } else { else_ }, None))
        }
        GenTermKind::Call { fsym, args } => {
            let def = att
                .ctx
                .resolve_call(fsym, args.len())
                .map_err(|error| Stop::Hard(error.into()))?;
            if *depth >= MAX_CALL_DEPTH {
                return Err(att.fail(
                    FailureKind::Depth,
                    RejectReason::CallDepth,
                    active_path_vars(frames),
                ));
            }
            let mut callee = Env::new();
            for ((param, _), arg) in def.iargs.iter().zip(args) {
                let Some(value) = att.evaluator.eval(arg, env) else {
                    return Err(att.fail(
                        FailureKind::Assert,
                        RejectReason::CallArg,
                        arg.free_vars(),
                    ));
                };
                callee.insert(param, value);
            }
            if spent_rec_size(def, &callee) {
                return Err(att.fail(
                    FailureKind::Depth,
                    RejectReason::RecursionBudget,
                    active_path_vars(frames),
                ));
            }
            let caller = std::mem::replace(env, callee);
            frames.push(Frame::Call {
                term,
                def,
                args,
                caller,
            });
            *depth += 1;
            Ok(Step::Visit(&def.body, None))
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
                    return Err(att.fail(
                        FailureKind::Assert,
                        RejectReason::MapBound,
                        perm.free_vars(),
                    ));
                }
            };
            Map {
                term,
                var,
                var_bt,
                perm,
                body,
                candidates,
                entries: Vec::new(),
            }
            .next(att, frames, env)
        }
    }
}

fn resume<'a, R: Rng + Rewind>(
    att: &mut Attempt<'a, '_, R>,
    depth: &mut usize,
    frame: Frame<'a, R::Mark>,
    out: Result<Option<Value>, Stop>,
    frames: &mut Vec<Frame<'a, R::Mark>>,
    trie: &mut Trie,
    env: &mut Env,
) -> Result<Step<'a>, Stop> {
    match frame {
        Frame::BindValue(mut binder) => {
            let bound = out?;
            binder.working = binder.persistent;
            if let Some(mark) = binder.replay_from {
                att.rng.restore(mark);
            }
            let after_draw = att.rng.save();
            if let Some(value) = bound {
                env.insert(binder.var, value);
            }
            let rest = binder.rest;
            frames.push(Frame::BindRest { binder, after_draw });
            Ok(Step::Visit(rest, None))
        }
        Frame::BindRest {
            mut binder,
            after_draw,
        } => match out {
            Err(Stop::Fail(failure)) => {
                let repair = binder.retry(att, trie, env, failure, after_draw)?;
                let value = binder.value;
                frames.push(Frame::BindValue(binder));
                Ok(Step::Visit(value, repair))
            }
            other => other.map(Step::Done),
        },
        Frame::Scope {
            name, key, parent, ..
        } => {
            let child = std::mem::replace(trie, parent);
            let value = out?;
            let child = child.finish().map_err(|error| Stop::Hard(error.into()))?;
            if !child.is_empty() {
                let Some(key) = key else {
                    return Err(att.fail(
                        FailureKind::Assert,
                        RejectReason::ScopeKey,
                        name.free_vars(),
                    ));
                };
                trie.push_scope(key, child);
            }
            Ok(Step::Done(value))
        }
        Frame::Ite { .. } => out.map(Step::Done),
        Frame::Call {
            def, args, caller, ..
        } => {
            let callee = std::mem::replace(env, caller);
            *depth -= 1;
            out.map(Step::Done).map_err(|stop| match stop {
                Stop::Fail(failure) => {
                    let blame_path = failure.kind == FailureKind::Depth;
                    let mut failure = remap_call(failure, def, args, &callee, env, att.capacity);
                    if blame_path {
                        for sym in active_path_vars(frames) {
                            failure.blame(sym, None);
                        }
                    }
                    Stop::Fail(failure)
                }
                hard => hard,
            })
        }
        Frame::Map {
            mut map,
            index,
            env: mark,
        } => {
            env.rewind(mark);
            match out {
                Ok(value) => {
                    if let Some(value) = value {
                        map.entries.push((index, value));
                    }
                    map.next(att, frames, env)
                }
                Err(stop) => Err(map_failure(stop, map.var, map.perm)),
            }
        }
    }
}
