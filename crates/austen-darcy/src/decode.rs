use austen_lib::gen_dsl::choice::translate::{
    MAX_CALL_DEPTH, MapDomainError, evaluated_write_choice_with_evaluator, index_scalar,
    spent_rec_size,
};
use austen_lib::gen_dsl::{
    AttachError, BaseType, ChoiceTree, Env, Evaluator, GenCallError, GenFailure, GenModule,
    GenTerm, GenTermKind, Incoming, IndexTerm, Label, Region, Scalar, Scope, ScopeMergeError, Sym,
    Value,
};

use crate::map_plan::MapPlan;
use crate::path::{DrawVar, SymbolicPath};
use crate::smt::ModelValue;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("the custom producer emitted an incompatible tree: {why}")]
    InvalidTree { why: String },
    #[error(transparent)]
    Default(#[from] austen_lib::gen_dsl::DefaultError),
    #[error("the model has {actual} values for {expected} symbolic draws")]
    ModelLength { expected: usize, actual: usize },
    #[error("the concrete walk reached draw {at}, past the {available} model values")]
    DrawExhausted { at: usize, available: usize },
    #[error("draw {at} was gathered as {gathered:?}, but replay reached {replayed:?}")]
    DrawSiteType {
        at: usize,
        gathered: BaseType,
        replayed: BaseType,
    },
    #[error("model value {value:?} does not inhabit draw {at}'s type {bt:?}")]
    ModelType {
        at: usize,
        bt: BaseType,
        value: ModelValue,
    },
    #[error("model value {value:?} at draw {at} does not fit Austen's value carrier")]
    ModelRange { at: usize, value: ModelValue },
    #[error("the concrete walk consumed {consumed} of {available} model values")]
    ModelRemainder { consumed: usize, available: usize },
    #[error("cannot evaluate the {context}")]
    Unevaluatable { context: &'static str },
    #[error("choice write {} cannot be materialised: {source}", .label.0)]
    Write {
        label: Label,
        #[source]
        source: GenFailure,
    },
    #[error("call to unknown generator {fsym:?}")]
    UnknownGenerator { fsym: Sym },
    #[error("call to {fsym:?} passes {actual} args, definition takes {expected}")]
    CallArityMismatch {
        fsym: Sym,
        expected: usize,
        actual: usize,
    },
    #[error("calls nest deeper than {} at {fsym:?}", MAX_CALL_DEPTH)]
    CallDepthExceeded { fsym: Sym },
    #[error("call to {fsym:?} reaches a spent recursion budget")]
    RecursionBudget { fsym: Sym },
    #[error("map permission is not a recognized bounds conjunction")]
    UnsupportedMapDomain,
    #[error("map permission cannot be evaluated for the model")]
    MapBound,
    #[error(transparent)]
    ScopeMerge(#[from] ScopeMergeError),
    #[error("the materialised tree does not validate: {why}")]
    Invalid { why: String },
}

impl From<GenCallError> for DecodeError {
    fn from(error: GenCallError) -> Self {
        match error {
            GenCallError::UnknownGenerator { name } => Self::UnknownGenerator { fsym: name },
            GenCallError::Arity {
                name,
                expected,
                actual,
            } => Self::CallArityMismatch {
                fsym: name,
                expected,
                actual,
            },
        }
    }
}

impl DecodeError {
    pub fn structural(self) -> Result<GenFailure, Self> {
        match self {
            Self::InvalidTree { why } => Ok(GenFailure::InvalidChoiceTree { reason: why }),
            Self::Default(error) => Ok(error.into()),
            Self::Write { source, .. } => Ok(source),
            Self::UnknownGenerator { fsym } => Ok(GenFailure::UnknownGenerator { fsym }),
            Self::CallArityMismatch {
                fsym,
                expected,
                actual,
            } => Ok(GenFailure::CallArityMismatch {
                fsym,
                expected,
                actual,
            }),
            Self::UnsupportedMapDomain => Ok(GenFailure::UnsupportedMapDomain),
            Self::ScopeMerge(error) => Ok(error.into()),
            per_attempt @ (Self::ModelLength { .. }
            | Self::DrawExhausted { .. }
            | Self::DrawSiteType { .. }
            | Self::ModelType { .. }
            | Self::ModelRange { .. }
            | Self::ModelRemainder { .. }
            | Self::Unevaluatable { .. }
            | Self::CallDepthExceeded { .. }
            | Self::RecursionBudget { .. }
            | Self::MapBound
            | Self::Invalid { .. }) => Err(per_attempt),
        }
    }
}

impl From<MapDomainError> for DecodeError {
    fn from(error: MapDomainError) -> Self {
        match error {
            MapDomainError::Unsupported => Self::UnsupportedMapDomain,
            MapDomainError::Unevaluatable | MapDomainError::TooLarge { .. } => Self::MapBound,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn concretise(
    symbolic: SymbolicPath,
    model: &[ModelValue],
    production_term: &GenTerm,
    production_module: &GenModule,
    validator_module: &GenModule,
    bounds: Region,
    max_array_length: usize,
) -> Result<ChoiceTree, DecodeError> {
    let incoming = Incoming::from(bounds);
    concretise_measured(
        symbolic,
        model,
        production_term,
        production_module,
        validator_module,
        &incoming,
        max_array_length,
        None,
    )
    .map(|(tree, _)| tree)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn concretise_measured(
    symbolic: SymbolicPath,
    model: &[ModelValue],
    production_term: &GenTerm,
    production_module: &GenModule,
    validator_module: &GenModule,
    incoming: &Incoming,
    max_array_length: usize,
    validator_root: Option<GenTerm>,
) -> Result<(ChoiceTree, u64), DecodeError> {
    let size = austen_lib::gen_dsl::TestCaseSize::new(symbolic.size)
        .unwrap_or(austen_lib::gen_dsl::TestCaseSize::MIN);
    let evaluator = Evaluator::sampled(
        production_module,
        size,
        0,
        &austen_lib::gen_dsl::DefaultTypes::default(),
    )?;
    evaluator.merge_defaults(&symbolic.defaults)?;
    concretise_with_evaluator(
        symbolic,
        model,
        production_term,
        production_module,
        validator_module,
        incoming,
        max_array_length,
        validator_root,
        &evaluator,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn concretise_with_evaluator(
    symbolic: SymbolicPath,
    model: &[ModelValue],
    production_term: &GenTerm,
    production_module: &GenModule,
    validator_module: &GenModule,
    incoming: &Incoming,
    max_array_length: usize,
    validator_root: Option<GenTerm>,
    evaluator: &Evaluator<'_>,
) -> Result<(ChoiceTree, u64), DecodeError> {
    if model.len() != symbolic.draws.len() {
        return Err(DecodeError::ModelLength {
            expected: symbolic.draws.len(),
            actual: model.len(),
        });
    }

    let mut env = Env::new();
    incoming.bind_globals(&mut env);
    let mut root = Scope::new();
    let mut materialiser = Materialiser {
        ctx: production_module,
        draws: &symbolic.draws,
        model,
        cursor: 0,
        owned_bytes: 0,
        max_array_length,
        evaluator,
    };
    let result = materialiser.walk_root(production_term, &mut root, &mut env);
    if let Some(error) = evaluator.default_error() {
        return Err(error.into());
    }
    result?;
    if materialiser.cursor != model.len() {
        return Err(DecodeError::ModelRemainder {
            consumed: materialiser.cursor,
            available: model.len(),
        });
    }

    let mut tree = ChoiceTree::new(root);
    tree.defaults = evaluator.default_choices();
    let custom_root = validator_root.is_some();
    let validator_term =
        validator_root.unwrap_or_else(|| production_module.wire_root(production_term));
    if let GenTermKind::Call { fsym, args } = &validator_term.term {
        validator_module.resolve_call(fsym, args.len())?;
    }
    let wire_evaluator = evaluator.for_module(validator_module);
    let validated =
        validator_term.validate_with_evaluator(validator_module, incoming, &tree, &wire_evaluator);
    if let Some(error) = evaluator.default_error() {
        return Err(error.into());
    }
    if let Err(error) = validated {
        if custom_root {
            austen_lib::gen_dsl::TreeLinearizer::new(validator_module, incoming)
                .linearize_with_evaluator(&validator_term, Env::new(), &tree, &wire_evaluator)
                .map_err(|error| match error {
                    austen_lib::gen_dsl::TranslateError::Default(error) => {
                        DecodeError::Default(error)
                    }
                    error => DecodeError::InvalidTree {
                        why: error.to_string(),
                    },
                })?;
        }
        return Err(DecodeError::Invalid {
            why: error.to_string(),
        });
    }
    tree.defaults = evaluator.default_choices();
    Ok((tree, materialiser.owned_bytes))
}

struct Materialiser<'a, 'e> {
    ctx: &'a GenModule,
    draws: &'a [DrawVar],
    model: &'a [ModelValue],
    cursor: usize,
    owned_bytes: u64,
    max_array_length: usize,
    evaluator: &'a Evaluator<'e>,
}

impl Materialiser<'_, '_> {
    fn walk_root(
        &mut self,
        term: &GenTerm,
        scope: &mut Scope,
        env: &mut Env,
    ) -> Result<Option<Value>, DecodeError> {
        let GenTermKind::Call { fsym, args } = &term.term else {
            return self.walk(0, term, scope, env);
        };
        let def = self.ctx.resolve_call(fsym, args.len())?;
        let mut callee_env = Env::new();
        for ((param, _), arg) in def.iargs.iter().zip(args) {
            let value = self
                .evaluator
                .eval(arg, env)
                .ok_or(DecodeError::Unevaluatable {
                    context: "root call argument",
                })?;
            callee_env.insert(param, value);
        }
        if spent_rec_size(def, &callee_env) {
            return Err(DecodeError::RecursionBudget { fsym: fsym.clone() });
        }
        self.walk(0, &def.body, scope, &mut callee_env)
    }

    fn walk(
        &mut self,
        depth: usize,
        term: &GenTerm,
        scope: &mut Scope,
        env: &mut Env,
    ) -> Result<Option<Value>, DecodeError> {
        match &term.term {
            GenTermKind::Arbitrary => self.draw(&term.bt).map(Some),

            GenTermKind::WriteChoice { label, value, rest } => {
                let choice =
                    evaluated_write_choice_with_evaluator(value, env, self.ctx, self.evaluator)
                        .map_err(|source| DecodeError::Write {
                            label: *label,
                            source,
                        })?
                        .ok_or(DecodeError::Unevaluatable {
                            context: "choice-write value",
                        })?;
                if scope.choices.contains_key(label) {
                    return Err(ScopeMergeError::DuplicateChoice {
                        path: Vec::new(),
                        label: *label,
                    }
                    .into());
                }
                scope.insert(*label, choice);
                self.walk(depth, rest, scope, env)
            }

            GenTermKind::LetStar { var, value, rest } => {
                let bound = self.walk(depth, value, scope, env)?;
                let mark = env.mark();
                if let Some(value) = bound {
                    env.insert(var, value);
                }
                let result = self.walk(depth, rest, scope, env);
                env.rewind(mark);
                result
            }

            GenTermKind::Return(value) => Ok(self.evaluator.eval(value, env)),

            GenTermKind::Assert { rest, .. } => self.walk(depth, rest, scope, env),
            GenTermKind::Asgn { ct, rest, .. } => {
                if let Some(bytes) = ct.byte_size_in(&self.ctx.structs) {
                    self.owned_bytes += bytes;
                }
                self.walk(depth, rest, scope, env)
            }

            GenTermKind::Scope { name, body } => {
                let key = name.eval_key_with_evaluator(env, self.evaluator);
                let mut fragment = Scope::new();
                let result = self.walk(depth, body, &mut fragment, env)?;
                scope
                    .try_attach(key, fragment)
                    .map_err(|error| match error {
                        AttachError::MissingKey => DecodeError::Unevaluatable {
                            context: "scope index",
                        },
                        AttachError::Merge(error) => error.into(),
                    })?;
                Ok(result)
            }

            GenTermKind::Ite { cond, then_, else_ } => {
                let take_then = self
                    .evaluator
                    .eval(cond, env)
                    .and_then(|v| v.as_bool())
                    .ok_or(DecodeError::Unevaluatable {
                        context: "ite condition",
                    })?;
                self.walk(depth, if take_then { then_ } else { else_ }, scope, env)
            }

            GenTermKind::Call { fsym, args } => self.call(depth, fsym, args, scope, env),

            GenTermKind::Map {
                var,
                var_bt,
                perm,
                body,
            } => self.map(depth, (var, var_bt, perm, body), scope, env),
        }
    }

    fn draw(&mut self, replayed: &BaseType) -> Result<Value, DecodeError> {
        let at = self.cursor;
        let draw = self.draws.get(at).ok_or(DecodeError::DrawExhausted {
            at,
            available: self.draws.len(),
        })?;
        let model = self.model.get(at).ok_or(DecodeError::DrawExhausted {
            at,
            available: self.model.len(),
        })?;
        if &draw.bt != replayed {
            return Err(DecodeError::DrawSiteType {
                at,
                gathered: draw.bt.clone(),
                replayed: replayed.clone(),
            });
        }
        let value = model_value(at, replayed, model)?;
        self.cursor += 1;
        Ok(value)
    }

    fn call(
        &mut self,
        depth: usize,
        fsym: &Sym,
        args: &[IndexTerm],
        scope: &mut Scope,
        env: &mut Env,
    ) -> Result<Option<Value>, DecodeError> {
        let def = self.ctx.resolve_call(fsym, args.len())?;
        if depth >= MAX_CALL_DEPTH {
            return Err(DecodeError::CallDepthExceeded { fsym: fsym.clone() });
        }
        let mut callee_env = Env::new();
        for ((param, _), arg) in def.iargs.iter().zip(args) {
            let value = self
                .evaluator
                .eval(arg, env)
                .ok_or(DecodeError::Unevaluatable {
                    context: "call argument",
                })?;
            callee_env.insert(param, value);
        }
        if spent_rec_size(def, &callee_env) {
            return Err(DecodeError::RecursionBudget { fsym: fsym.clone() });
        }
        self.walk(depth + 1, &def.body, scope, &mut callee_env)
    }

    fn map(
        &mut self,
        depth: usize,
        (var, var_bt, perm, body): (&Sym, &BaseType, &IndexTerm, &GenTerm),
        scope: &mut Scope,
        env: &mut Env,
    ) -> Result<Option<Value>, DecodeError> {
        let plan = MapPlan::new(perm, var, var_bt, self.ctx, self.max_array_length)?;
        let mut entries = Vec::new();
        for offset in 0..plan.candidate_count() {
            let candidate = plan.candidate(offset)?;
            let i = self
                .evaluator
                .eval(&candidate, env)
                .and_then(|value| value.as_int())
                .ok_or(DecodeError::MapBound)?;
            let mut fragment = Scope::new();
            let previous_bytes = self.owned_bytes;
            let (active, value) = env.scoped(var, index_scalar(var_bt, i), |env| {
                let active = self
                    .evaluator
                    .eval(perm, env)
                    .and_then(|value| value.as_bool())
                    .ok_or(DecodeError::MapBound)?;
                let value = self.walk(depth, body, &mut fragment, env)?;
                Ok::<_, DecodeError>((active, value))
            })?;
            if active {
                scope.try_merge(fragment)?;
                if let Some(value) = value {
                    entries.push((i, value));
                }
            } else {
                self.owned_bytes = previous_bytes;
            }
        }
        Ok(Some(Value::iteration_map(entries)))
    }
}

fn model_value(at: usize, bt: &BaseType, model: &ModelValue) -> Result<Value, DecodeError> {
    let mismatch = || DecodeError::ModelType {
        at,
        bt: bt.clone(),
        value: model.clone(),
    };
    let range = || DecodeError::ModelRange {
        at,
        value: model.clone(),
    };
    let scalar = match (bt, model) {
        (BaseType::Bool, ModelValue::Bool(value)) => Scalar::Bool(*value),
        (BaseType::Bits(expected_sign, expected_width), ModelValue::Bits { sign, width, .. })
            if expected_sign == sign && expected_width == width =>
        {
            let value = model
                .signed_value()
                .and_then(|n| i128::try_from(&n).ok())
                .ok_or_else(range)?;
            Scalar::bits(*sign, *width, value).unwrap_or_else(|| Scalar::Int(value.into()))
        }
        (BaseType::Loc(_), ModelValue::Pointer { addr, .. }) => Scalar::heap_pointer(*addr),
        (BaseType::Loc(_), ModelValue::Null) => Scalar::heap_pointer(0),
        _ => return Err(mismatch()),
    };
    Ok(Value::Scalar(scalar))
}
