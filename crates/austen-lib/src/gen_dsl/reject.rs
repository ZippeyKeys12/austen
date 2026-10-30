use strum::{EnumCount, EnumIter, IntoEnumIterator};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, EnumIter, EnumCount)]
pub enum RejectReason {
    AssertFailed,
    AssertUnevaluatable,
    AssertForall,
    ScopeKey,
    IteCond,
    CallDepth,
    RecursionBudget,
    CallArg,
    MapBound,
    AsgnUnevaluatable,
    AsgnOutOfBounds,
    AsgnOverlap,
    PointerCapacity,
    GeneratorTimeout,
    LetExhausted,
    RepairCapacity,
    SearchBudget,
    PathUnsat,
    SolverUnknown,
    SolverUnsupported,
    ModelInvalid,
}

impl RejectReason {
    pub fn token(self) -> &'static str {
        match self {
            RejectReason::AssertFailed => "assert-failed",
            RejectReason::AssertUnevaluatable => "assert-unevaluatable",
            RejectReason::AssertForall => "assert-forall",
            RejectReason::ScopeKey => "scope-key",
            RejectReason::IteCond => "ite-cond",
            RejectReason::CallDepth => "call-depth",
            RejectReason::RecursionBudget => "recursion-budget",
            RejectReason::CallArg => "call-arg",
            RejectReason::MapBound => "map-bound",
            RejectReason::AsgnUnevaluatable => "asgn-unevaluatable",
            RejectReason::AsgnOutOfBounds => "asgn-out-of-bounds",
            RejectReason::AsgnOverlap => "asgn-overlap",
            RejectReason::PointerCapacity => "pointer-capacity",
            RejectReason::LetExhausted => "let-exhausted",
            RejectReason::RepairCapacity => "repair-capacity",
            RejectReason::SearchBudget => "search-budget",
            RejectReason::GeneratorTimeout => "generator-timeout",
            RejectReason::PathUnsat => "path-unsat",
            RejectReason::SolverUnknown => "solver-unknown",
            RejectReason::SolverUnsupported => "solver-unsupported",
            RejectReason::ModelInvalid => "model-invalid",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RejectTally([u64; RejectReason::COUNT]);

impl RejectTally {
    pub fn bump(&mut self, reason: RejectReason) {
        self.0[reason as usize] += 1;
    }

    pub fn get(&self, reason: RejectReason) -> u64 {
        self.0[reason as usize]
    }

    pub fn total(&self) -> u64 {
        self.0.iter().sum()
    }

    pub fn since(&self, before: &Self) -> Self {
        let mut delta = *self;
        for (into, from) in delta.0.iter_mut().zip(before.0) {
            *into = into.saturating_sub(from);
        }
        delta
    }

    pub fn nonzero(&self) -> Vec<(&'static str, u64)> {
        RejectReason::iter()
            .filter(|r| self.get(*r) > 0)
            .map(|r| (r.token(), self.get(r)))
            .collect()
    }
}

impl std::ops::AddAssign for RejectTally {
    fn add_assign(&mut self, other: Self) {
        for (into, from) in self.0.iter_mut().zip(other.0) {
            *into += from;
        }
    }
}
