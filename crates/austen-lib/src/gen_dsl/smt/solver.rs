use std::cell::Cell;
use std::time::Duration;

use z3::ast::Bool;
use z3::{Model, Params, SatResult};

#[derive(Debug, Clone, Default)]
pub struct SolverConfig {
    pub timeout: Option<Duration>,
    pub memory_max_mb: Option<u64>,
}

impl SolverConfig {
    pub fn scope<R: Send + Sync>(&self, f: impl FnOnce() -> R + Send + Sync) -> R {
        z3::with_z3_config(&self.context_config(), f)
    }

    pub fn context_config(&self) -> z3::Config {
        let cfg = z3::Config::new();
        if let Some(mb) = self.memory_max_mb {
            z3::set_global_param("memory_high_watermark_mb", &mb.div_ceil(2).to_string());
            z3::set_global_param("memory_max_size", &mb.to_string());
        }
        cfg
    }

    pub fn timeout_ms(&self) -> Option<u32> {
        self.timeout
            .map(|timeout| u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX))
    }

    pub fn effective_memory_limit(&self) -> Option<u64> {
        self.memory_max_mb
            .or_else(|| z3::get_global_param("memory_max_size")?.parse().ok())
    }

    pub(in crate::gen_dsl) fn params(&self) -> Params {
        let mut params = Params::new();
        if let Some(ms) = self.timeout_ms() {
            params.set_u32("timeout", ms);
        }
        params
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SolveStatus {
    Sat,
    Unsat,
    Unknown { reason: Option<String> },
}

impl SolveStatus {
    pub fn is_sat(&self) -> bool {
        matches!(self, SolveStatus::Sat)
    }

    pub fn is_unsat(&self) -> bool {
        matches!(self, SolveStatus::Unsat)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proof {
    Proved,
    Refuted,
    Unknown { reason: Option<String> },
}

pub struct Prover {
    solver: z3::Solver,
    last: Cell<Option<SatResult>>,
}

impl Prover {
    pub fn new(config: &SolverConfig) -> Self {
        Self::with_params(config, |_| {})
    }

    pub fn with_params(config: &SolverConfig, configure: impl FnOnce(&mut Params)) -> Self {
        let mut params = config.params();
        configure(&mut params);
        let solver = z3::Solver::new();
        solver.set_params(&params);
        Prover {
            solver,
            last: Cell::new(None),
        }
    }

    pub fn assert(&self, formula: &Bool) {
        self.last.set(None);
        self.solver.assert(formula);
    }

    pub fn assert_tracked(&self, formula: &Bool, label: &str) -> Bool {
        self.last.set(None);
        let marker = Bool::new_const(label);
        self.solver.assert_and_track(formula, &marker);
        marker
    }

    pub fn push(&self) {
        self.last.set(None);
        self.solver.push();
    }

    pub fn pop(&self, n: u32) {
        self.last.set(None);
        self.solver.pop(n);
    }

    pub fn to_smt2(&self) -> String {
        self.solver.to_smt2()
    }

    pub fn check(&self) -> SolveStatus {
        let result = self.solver.check();
        self.last.set(Some(result));
        match result {
            SatResult::Sat => SolveStatus::Sat,
            SatResult::Unsat => SolveStatus::Unsat,
            SatResult::Unknown => SolveStatus::Unknown {
                reason: self.solver.get_reason_unknown(),
            },
        }
    }

    pub fn model(&self) -> Option<Model> {
        match self.last.get() {
            Some(SatResult::Sat) => self.solver.get_model(),
            Some(_) => None,
            None => self.check_with_model().1,
        }
    }

    pub fn check_with_model(&self) -> (SolveStatus, Option<Model>) {
        let status = self.check();
        let model = if status.is_sat() {
            self.solver.get_model()
        } else {
            None
        };
        (status, model)
    }

    pub fn prove(&self, goal: &Bool) -> Proof {
        self.push();
        self.assert(&goal.not());
        let out = match self.check() {
            SolveStatus::Unsat => Proof::Proved,
            SolveStatus::Sat => Proof::Refuted,
            SolveStatus::Unknown { reason } => Proof::Unknown { reason },
        };
        self.pop(1);
        out
    }

    pub fn prove_under(&self, assumptions: &[&Bool], goal: &Bool) -> Proof {
        self.push();
        for a in assumptions {
            self.assert(a);
        }
        let out = self.prove(goal);
        self.pop(1);
        out
    }
}
