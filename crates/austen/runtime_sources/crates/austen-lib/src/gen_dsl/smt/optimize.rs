use z3::ast::Bool;
use z3::{Model, Optimize, SatResult};

use crate::gen_dsl::smt::solver::{SolveStatus, SolverConfig};

pub const SKEW_WEIGHT: u64 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MaxSatEngine {
    #[default]
    MaxRes,
    WMax,
}

pub struct Optimizer {
    opt: Optimize,
}

impl Optimizer {
    pub fn new(config: &SolverConfig) -> Self {
        Self::with_engine(config, MaxSatEngine::MaxRes)
    }

    pub fn with_engine(config: &SolverConfig, engine: MaxSatEngine) -> Self {
        let opt = Optimize::new();
        let mut params = config.params();
        params.set_symbol(
            "maxsat_engine",
            match engine {
                MaxSatEngine::MaxRes => "maxres",
                MaxSatEngine::WMax => "wmax",
            },
        );
        opt.set_params(&params);
        Optimizer { opt }
    }

    pub fn assert(&self, formula: &Bool) {
        self.opt.assert(formula);
    }

    pub fn assert_soft(&self, formula: &Bool) {
        self.opt.assert_soft(formula, SKEW_WEIGHT, None);
    }

    pub(in crate::gen_dsl) fn push(&self) {
        self.opt.push();
    }

    pub(in crate::gen_dsl) fn pop(&self, n: u32) {
        for _ in 0..n {
            self.opt.pop();
        }
    }

    pub fn check(&self) -> SolveStatus {
        match self.opt.check(&[]) {
            SatResult::Sat => SolveStatus::Sat,
            SatResult::Unsat => SolveStatus::Unsat,
            SatResult::Unknown => SolveStatus::Unknown {
                reason: self.opt.get_reason_unknown(),
            },
        }
    }

    pub fn check_with_model(&self) -> (SolveStatus, Option<Model>) {
        let status = self.check();
        let model = if status.is_sat() {
            self.opt.get_model()
        } else {
            None
        };
        (status, model)
    }
}
