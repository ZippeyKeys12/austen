mod factory;
pub use factory::DarcyFactory;

use std::{sync::Arc, time::Instant};

use austen_lib::gen_dsl::passes::PassProfile;
use austen_lib::gen_dsl::{
    ChoiceTree, Entropy, EntropyBacked, Evaluator, GenFailure, GenModule, GenModuleError, GenTerm,
    Incoming, ProduceCx, RejectReason, RejectTally, RejectingProducer, Rng, TreeProducer,
    gen_term_watermark,
};

pub mod decode;

pub(crate) mod map_plan;
pub mod path;
pub mod query;
mod strategy;

pub use austen_lib::gen_dsl::smt;

pub use path::{DrawVar, ResourceClaim, SymbolicPath};
pub use query::Outcome;
pub use smt::SolverConfig;

pub use austen_lib::gen_dsl::rand_like::DEFAULT_MAX_SIZE;

pub const DEFAULT_SOLVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

pub const DEFAULT_SOLVE_MEMORY_MB: u64 = 2048;

pub const PASSES: PassProfile = PassProfile::NONE;

enum Validation {
    Production,
    Wire(Arc<GenModule>),
    Entry {
        module: Arc<GenModule>,
        entry: austen_lib::gen_dsl::Sym,
    },
}

pub struct Darcy<R> {
    module: Arc<GenModule>,
    validation: Validation,
    incoming: Incoming,
    max_size: u64,
    generator_timeout: Option<std::time::Duration>,
    max_attempts: u32,
    max_array_length: usize,
    solver: SolverConfig,
    strategies: Vec<strategy::Strategy>,
    rng: R,
    rejects: RejectTally,
    site_index: austen_lib::gen_dsl::SiteIndex,
    sites: austen_lib::gen_dsl::SiteTally,
    timings: austen_lib::gen_dsl::Timings,
    last_owned_bytes: u64,
}

impl<R: Rng> Darcy<R> {
    pub fn new(
        module: Arc<GenModule>,
        incoming: impl Into<Incoming>,
        max_attempts: u32,
        rng: R,
    ) -> Result<Self, GenModuleError> {
        module.validate()?;
        let incoming = incoming.into();
        incoming.heap().debug_check_published();
        let site_index = austen_lib::gen_dsl::SiteIndex::new(&module);
        let strategies = (0..=module.defs.len())
            .map(|_| strategy::Strategy::default())
            .collect();
        Ok(Self {
            module,
            validation: Validation::Production,
            incoming,
            max_size: DEFAULT_MAX_SIZE,
            generator_timeout: None,
            max_attempts,
            max_array_length: austen_lib::gen_dsl::passes::DEFAULT_MAX_ARRAY_LENGTH,
            solver: SolverConfig {
                timeout: Some(DEFAULT_SOLVE_TIMEOUT),
                memory_max_mb: Some(DEFAULT_SOLVE_MEMORY_MB),
            },
            strategies,
            rng,
            rejects: RejectTally::default(),
            site_index,
            sites: austen_lib::gen_dsl::SiteTally::new(),
            timings: austen_lib::gen_dsl::Timings::new(),
            last_owned_bytes: 0,
        })
    }

    pub fn rejections(&self) -> RejectTally {
        self.rejects
    }

    #[must_use]
    pub fn with_max_size(mut self, max_size: u64) -> Self {
        self.max_size = max_size.clamp(1, austen_lib::gen_dsl::rand_like::MAX_SIZE_LIMIT);
        self
    }

    #[must_use]
    pub fn with_max_array_length(mut self, max_array_length: usize) -> Self {
        self.max_array_length = max_array_length.clamp(
            1,
            austen_lib::gen_dsl::choice::translate::MAX_MAP_ITERATIONS as usize,
        );
        self
    }

    #[must_use]
    pub fn with_generator_timeout(mut self, timeout: Option<std::time::Duration>) -> Self {
        self.generator_timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_solver_config(mut self, config: SolverConfig) -> Self {
        self.solver = config;
        self
    }

    pub fn with_validator(mut self, module: Arc<GenModule>) -> Result<Self, GenModuleError> {
        module.validate()?;
        self.validation = Validation::Wire(module);
        Ok(self)
    }

    pub fn with_validator_entry(
        mut self,
        module: Arc<GenModule>,
        entry: austen_lib::gen_dsl::Sym,
    ) -> Result<Self, GenModuleError> {
        module.validate()?;
        module.function_generator(&entry)?;
        self.validation = Validation::Entry { module, entry };
        Ok(self)
    }

    fn attempt_with_evaluator(
        &mut self,
        term: &GenTerm,
        size: austen_lib::gen_dsl::TestCaseSize,
        sym_seed: u64,
        deadline: Option<std::time::Instant>,
        evaluator: &Evaluator<'_>,
    ) -> Result<Option<ChoiceTree>, GenFailure> {
        let setup = path::Setup {
            term,
            ctx: &self.module,
            incoming: &self.incoming,
            max_array_length: self.max_array_length,
            deadline,
        };
        let gathered = timed(&mut self.timings, "darcy:select-path", || {
            path::Gather::run_with_evaluator(
                &setup,
                size,
                sym_seed,
                &mut self.rng,
                &self.site_index,
                evaluator,
            )
        });
        let (gathered, precheck) = match gathered {
            Ok(g) => g,
            Err(path::Stop::Hard(f)) => return Err(f),
            Err(path::Stop::Reject(r)) => {
                self.rejects.bump(r);
                return Ok(None);
            }
        };

        let mut unsupported_site = None;
        let mut query_measurement = None;
        let outcome = if let Some(values) = (precheck.passed()
            && gathered.default_types(&self.module).is_empty())
        .then(|| query::skew_values(&gathered))
        .flatten()
        {
            Ok(Outcome::Model(values))
        } else {
            let profile = match &term.term {
                austen_lib::gen_dsl::GenTermKind::Call { fsym, .. } => {
                    self.module.defs.iter().position(|(name, _)| name == fsym)
                }
                _ => None,
            }
            .unwrap_or(self.module.defs.len());
            let policy = &mut self.strategies[profile];
            if precheck == path::Precheck::AssertUnevaluatable {
                let redraw = self.rng.decide(&mut || policy.choose_redraw());
                policy.observe_redraw(redraw);
                if redraw {
                    self.rejects.bump(RejectReason::AssertUnevaluatable);
                    return Ok(None);
                }
            }
            let wmax = self.rng.decide(&mut || policy.choose_wmax());
            let engine = if wmax {
                smt::MaxSatEngine::WMax
            } else {
                smt::MaxSatEngine::MaxRes
            };
            let started = Instant::now();
            let outcome = timed(&mut self.timings, "darcy:query-solver", || {
                query::solve_with_engine(
                    &gathered,
                    &self.module,
                    &self.incoming,
                    &self.solver,
                    engine,
                    &mut unsupported_site,
                )
            });
            let elapsed = started.elapsed();
            let phase = match engine {
                smt::MaxSatEngine::MaxRes => "darcy:query-maxres",
                smt::MaxSatEngine::WMax => "darcy:query-wmax",
            };
            self.timings.entry(phase).or_default().observe(elapsed);
            query_measurement = Some((profile, engine, started));
            outcome
        };
        let outcome = match outcome {
            Ok(o) => o,
            Err(e) => {
                self.observe_query(query_measurement, false);
                tracing::debug!(target: "austen::darcy", error = %e, "the query could not be built");
                self.rejects.bump(RejectReason::SolverUnsupported);
                if let Some(site) = &unsupported_site {
                    austen_lib::gen_dsl::SiteIndex::count(&mut self.sites, site, false);
                }
                return Ok(None);
            }
        };

        match outcome {
            Outcome::Unsat => {
                self.observe_query(query_measurement, true);
                self.rejects.bump(RejectReason::PathUnsat);
                Ok(None)
            }
            Outcome::Unknown { reason } => {
                self.observe_query(query_measurement, false);
                tracing::debug!(target: "austen::darcy", ?reason, "the solver gave up");
                self.rejects.bump(RejectReason::SolverUnknown);
                Ok(None)
            }
            Outcome::Model(model) => {
                let decoded = timed(&mut self.timings, "darcy:concretize", || {
                    let (validator, validator_root) = match &self.validation {
                        Validation::Production => (&*self.module, None),
                        Validation::Wire(module) => (&**module, None),
                        Validation::Entry { module, entry } => {
                            let def = module
                                .generator(entry)
                                .expect("with_validator_entry resolved this definition");
                            (
                                &**module,
                                Some(def.root_call_with_size(size, self.incoming.heap())),
                            )
                        }
                    };
                    decode::concretise_with_evaluator(
                        gathered,
                        &model,
                        term,
                        &self.module,
                        validator,
                        &self.incoming,
                        self.max_array_length,
                        validator_root,
                        evaluator,
                    )
                });
                self.observe_query(query_measurement, decoded.is_ok());
                match decoded {
                    Ok((tree, bytes)) => {
                        if precheck == path::Precheck::AssertUnevaluatable
                            && let Some((profile, _, _)) = query_measurement
                        {
                            self.strategies[profile].observe_repair_model(true);
                        }
                        self.last_owned_bytes = bytes;
                        Ok(Some(tree))
                    }
                    Err(e) => match e.structural() {
                        Ok(hard) => Err(hard),
                        Err(e) => {
                            if precheck == path::Precheck::AssertUnevaluatable
                                && let Some((profile, _, _)) = query_measurement
                            {
                                self.strategies[profile].observe_repair_model(false);
                            }
                            tracing::debug!(target: "austen::darcy", error = %e, "the model could not be materialised");
                            self.rejects.bump(RejectReason::ModelInvalid);
                            Ok(None)
                        }
                    },
                }
            }
        }
    }

    fn observe_query(
        &mut self,
        measurement: Option<(usize, smt::MaxSatEngine, Instant)>,
        useful: bool,
    ) {
        if let Some((profile, engine, started)) = measurement {
            let elapsed = started.elapsed();
            let phase = match engine {
                smt::MaxSatEngine::MaxRes => "darcy:repair-maxres",
                smt::MaxSatEngine::WMax => "darcy:repair-wmax",
            };
            self.timings.entry(phase).or_default().observe(elapsed);
            let failure_cost = self
                .solver
                .timeout
                .filter(|timeout| !timeout.is_zero())
                .unwrap_or(DEFAULT_SOLVE_TIMEOUT);
            self.strategies[profile].observe(engine, elapsed, useful, failure_cost);
        }
    }
}

pub type SeededDarcy = Darcy<Entropy>;

impl SeededDarcy {
    pub fn seeded(
        module: Arc<GenModule>,
        incoming: impl Into<Incoming>,
        max_attempts: u32,
        seed: u64,
    ) -> Result<Self, GenModuleError> {
        Self::new(module, incoming, max_attempts, Entropy::seeded(seed))
    }

    pub fn entropy(&self) -> &Entropy {
        &self.rng
    }
}

fn timed<T>(
    timings: &mut austen_lib::gen_dsl::Timings,
    phase: &'static str,
    f: impl FnOnce() -> T,
) -> T {
    let started = Instant::now();
    let result = f();
    timings.entry(phase).or_default().observe(started.elapsed());
    result
}

impl<R: Rng> TreeProducer for Darcy<R> {
    fn produce_in(
        &mut self,
        term: &GenTerm,
        cx: ProduceCx<'_, '_>,
    ) -> Result<Option<ChoiceTree>, GenFailure> {
        let ProduceCx { evaluator, stop } = cx;
        self.timings.clear();
        self.sites.clear();
        self.last_owned_bytes = 0;
        if stop() {
            return Ok(None);
        }
        if self.max_attempts == 0 {
            return Err(GenFailure::RetriesExhausted { attempts: 0 });
        }
        let module = Arc::clone(&self.module);
        let (root_size, evaluator) = austen_lib::gen_dsl::request_evaluator(
            term,
            &module,
            &self.incoming,
            evaluator,
            &mut self.rng,
            self.max_size,
        )?;
        let sym_seed = gen_term_watermark(term, &self.module);
        let root_size = root_size.or(evaluator.size()).unwrap_or_else(|| {
            austen_lib::gen_dsl::TestCaseSize::sample_uniform(&mut self.rng, self.max_size)
        });
        let deadline = self
            .generator_timeout
            .map(|d| std::time::Instant::now() + d);
        for attempt in 0..self.max_attempts {
            if stop() {
                return Ok(None);
            }
            if attempt > 0 && deadline.is_some_and(|at| std::time::Instant::now() >= at) {
                break;
            }
            if let Some(tree) =
                self.attempt_with_evaluator(term, root_size, sym_seed, deadline, &evaluator)?
            {
                return Ok(Some(tree));
            }
        }
        Err(GenFailure::RetriesExhausted {
            attempts: self.max_attempts,
        })
    }
}

impl RejectingProducer for SeededDarcy {
    fn rejections(&self) -> RejectTally {
        self.rejects
    }

    fn sites(&self) -> &austen_lib::gen_dsl::SiteTally {
        &self.sites
    }

    fn timings(&self) -> &austen_lib::gen_dsl::Timings {
        &self.timings
    }

    fn last_owned_bytes(&self) -> u64 {
        self.last_owned_bytes
    }
}

impl EntropyBacked for SeededDarcy {
    fn entropy_mut(&mut self) -> &mut Entropy {
        &mut self.rng
    }
}
