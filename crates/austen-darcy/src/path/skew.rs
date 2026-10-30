use austen_lib::gen_dsl::choice::translate::{ConstraintError, check_constraint_with_evaluator};
use austen_lib::gen_dsl::{
    Env, EnvMark, Evaluator, GenModule, HeapMark, Incoming, IndexTerm, LogicalConstraint,
    MarkedHeap, Region, Scalar, Sym, Value,
};

use super::ResourceClaim;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Precheck {
    #[default]
    Passed,
    Failed,
    AssertUnevaluatable,
}

impl Precheck {
    pub(crate) fn passed(self) -> bool {
        self == Self::Passed
    }
}

#[derive(Default)]
pub(super) struct SkewCheck {
    env: Env,
    writes: MarkedHeap,
    verdict: Precheck,
}

#[derive(Clone, Copy)]
pub(super) struct Mark {
    env: EnvMark,
    writes: HeapMark,
    verdict: Precheck,
}

impl SkewCheck {
    pub(super) fn passed(&self) -> bool {
        self.verdict.passed()
    }

    pub(super) fn verdict(&self) -> Precheck {
        self.verdict
    }

    pub(super) fn mark(&self) -> Mark {
        Mark {
            env: self.env.mark(),
            writes: self.writes.save(),
            verdict: self.verdict,
        }
    }

    pub(super) fn rewind(&mut self, mark: Mark) {
        self.env.rewind(mark.env);
        self.writes.restore(mark.writes);
        self.verdict = mark.verdict;
    }

    pub(super) fn bind(&mut self, sym: &Sym, value: Option<&Value>) {
        if self.passed() {
            match value {
                Some(value) => self.env.insert(sym, value.clone()),
                None => self.verdict = Precheck::Failed,
            }
        }
    }

    pub(super) fn constraint_with_evaluator(
        &mut self,
        constraint: &LogicalConstraint,
        ctx: &GenModule,
        evaluator: &Evaluator<'_>,
    ) {
        if self.passed() {
            self.verdict =
                match check_constraint_with_evaluator(constraint, &self.env, ctx, evaluator) {
                    Ok(()) => Precheck::Passed,
                    Err(ConstraintError::Failed) => Precheck::Failed,
                    Err(ConstraintError::Unevaluatable) => Precheck::AssertUnevaluatable,
                };
        }
    }

    pub(super) fn pin_with_evaluator(
        &mut self,
        pin: &IndexTerm,
        _ctx: &GenModule,
        evaluator: &Evaluator<'_>,
    ) {
        if self.passed() && evaluator.eval(pin, &self.env).and_then(|v| v.as_bool()) != Some(true) {
            self.verdict = Precheck::Failed;
        }
    }

    pub(super) fn resource_with_evaluator(
        &mut self,
        claim: &ResourceClaim,
        _ctx: &GenModule,
        incoming: &Incoming,
        evaluator: &Evaluator<'_>,
    ) {
        if self.passed() && self.check_resource(claim, incoming, evaluator).is_none() {
            self.verdict = Precheck::Failed;
        }
    }

    fn check_resource(
        &mut self,
        claim: &ResourceClaim,
        incoming: &Incoming,
        evaluator: &Evaluator<'_>,
    ) -> Option<()> {
        if !evaluator.eval(&claim.active, &self.env)?.as_bool()? {
            return Some(());
        }
        let address = |term| match evaluator.eval(term, &self.env)?.as_scalar()? {
            Scalar::Pointer { addr, .. } => Some(addr),
            _ => None,
        };
        let first = address(&claim.first_addr)?;
        let last = address(&claim.last_addr)?;
        if first == 0 || first > last {
            return None;
        }
        if claim.tail_bytes == 0 {
            return Some(());
        }
        let size = (last - first).checked_add(claim.tail_bytes)?;
        let extent = Region { base: first, size };
        incoming.grant_write(&extent)?;
        self.writes.mark(extent).ok()?;
        Some(())
    }
}
