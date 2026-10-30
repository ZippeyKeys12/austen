use z3::Model;
use z3::ast::Bool;

use super::{MaxSatEngine, Optimizer, Prover, SolveStatus, SolverConfig};

enum Backend {
    Hard(Prover),
    Skewed(Optimizer),
}

pub struct ConstraintSolver {
    backend: Backend,
}

impl ConstraintSolver {
    pub fn new(config: &SolverConfig) -> Self {
        Self {
            backend: Backend::Hard(Prover::new(config)),
        }
    }

    pub fn with_skews(config: &SolverConfig) -> Self {
        Self::with_skews_and_engine(config, MaxSatEngine::MaxRes)
    }

    pub fn with_skews_and_engine(config: &SolverConfig, engine: MaxSatEngine) -> Self {
        Self {
            backend: Backend::Skewed(Optimizer::with_engine(config, engine)),
        }
    }

    pub fn assert(&self, formula: &Bool) {
        match &self.backend {
            Backend::Hard(prover) => prover.assert(formula),
            Backend::Skewed(optimizer) => optimizer.assert(formula),
        }
    }

    pub fn assert_soft(&self, formula: &Bool) {
        match &self.backend {
            Backend::Skewed(optimizer) => optimizer.assert_soft(formula),
            Backend::Hard(_) => {
                panic!("skews need a solver built by ConstraintSolver::with_skews")
            }
        }
    }

    pub fn push(&self) {
        match &self.backend {
            Backend::Hard(prover) => prover.push(),
            Backend::Skewed(optimizer) => optimizer.push(),
        }
    }

    pub fn pop(&self, n: u32) {
        match &self.backend {
            Backend::Hard(prover) => prover.pop(n),
            Backend::Skewed(optimizer) => optimizer.pop(n),
        }
    }

    pub fn check(&self) -> SolveStatus {
        match &self.backend {
            Backend::Hard(prover) => prover.check(),
            Backend::Skewed(optimizer) => optimizer.check(),
        }
    }

    pub fn check_with_model(&self) -> (SolveStatus, Option<Model>) {
        match &self.backend {
            Backend::Hard(prover) => prover.check_with_model(),
            Backend::Skewed(optimizer) => optimizer.check_with_model(),
        }
    }
}
