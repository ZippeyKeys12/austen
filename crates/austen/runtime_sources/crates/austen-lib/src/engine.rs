use crate::gen_dsl::passes::PassProfile;
use crate::observe;
use crate::observe::{RunEvent, Warning};
use crate::wire::{OracleStatus, Tally};
use enum_dispatch::enum_dispatch;
use std::cell::RefCell;

use crate::campaign::{Budget, CampaignConfig, CampaignError, CampaignReport, Engine};
use crate::coverage::{Entry as CoverageEntry, Journal, Provenance, Timing};
use crate::external::{
    Artifacts, CandidateGate, CandidateGateError, ExternalRequest, SeedCandidate, StagedSeed,
};
use crate::findings::{self, ArtifactValidator, Crashes, LaunchValidationSession, Replay};
use crate::{AnyDelegatedEngine, AnyGenerator, DelegatedEngine};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopOwner {
    Austen,
    DelegatedTool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionSelectionOwner {
    Austen,
    Generator,
    DelegatedTool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoveragePolicy {
    pub observed_by_austen: bool,
    pub can_run_blind: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineDescriptor {
    pub loop_owner: LoopOwner,
    pub generic_seed: bool,
    pub entropy_tape: bool,
    pub function_selection: FunctionSelectionOwner,
    pub producer_options: bool,
    pub max_array_length: bool,
    pub coverage: CoveragePolicy,
}

pub(crate) const TREE_ENGINE_DESCRIPTOR: EngineDescriptor = EngineDescriptor {
    loop_owner: LoopOwner::Austen,
    generic_seed: true,
    entropy_tape: true,
    function_selection: FunctionSelectionOwner::Austen,
    producer_options: true,
    max_array_length: true,
    coverage: CoveragePolicy {
        observed_by_austen: true,
        can_run_blind: true,
    },
};

pub const RAW_GENERATOR_DESCRIPTOR: EngineDescriptor = EngineDescriptor {
    loop_owner: LoopOwner::Austen,
    generic_seed: true,
    entropy_tape: false,
    function_selection: FunctionSelectionOwner::Generator,
    producer_options: false,
    max_array_length: false,
    coverage: CoveragePolicy {
        observed_by_austen: true,
        can_run_blind: true,
    },
};

#[enum_dispatch(Engine)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnyEngine {
    AustenDriven(AnyGenerator),
    Delegated(AnyDelegatedEngine),
}

#[derive(Debug)]
pub struct EngineRegistry {
    engines: &'static [AnyEngine],
}

impl EngineRegistry {
    pub const fn new(engines: &'static [AnyEngine]) -> Self {
        Self { engines }
    }

    pub fn iter(&self) -> impl Iterator<Item = AnyEngine> + '_ {
        self.engines.iter().copied()
    }

    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.iter().map(|engine| engine.name())
    }

    pub fn resolve(&self, name: &str) -> Option<AnyEngine> {
        self.iter().find(|engine| engine.name() == name)
    }

    pub fn tree_engines(&self) -> impl Iterator<Item = crate::AnyTreeEngine> + '_ {
        self.iter().filter_map(|engine| match engine {
            AnyEngine::AustenDriven(AnyGenerator::Tree(tree)) => Some(tree),
            _ => None,
        })
    }

    pub fn tree_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.tree_engines()
            .map(|engine| crate::TreeEngineFactory::name(&engine))
    }

    pub fn resolve_tree(&self, name: &str) -> Option<crate::AnyTreeEngine> {
        self.tree_engines()
            .find(|engine| crate::TreeEngineFactory::name(engine) == name)
    }

    pub fn delegated_engines(&self) -> impl Iterator<Item = AnyDelegatedEngine> + '_ {
        self.iter().filter_map(|engine| match engine {
            AnyEngine::Delegated(delegated) => Some(delegated),
            _ => None,
        })
    }

    pub fn fuzzers(&self) -> impl Iterator<Item = crate::AnyFuzzer> + '_ {
        self.delegated_engines()
            .filter_map(AnyDelegatedEngine::as_fuzzer)
    }

    pub fn resolve_delegated(&self, name: &str) -> Option<AnyDelegatedEngine> {
        self.delegated_engines()
            .find(|engine| DelegatedEngine::name(engine) == name)
    }

    pub fn resolve_fuzzer(&self, name: &str) -> Option<crate::AnyFuzzer> {
        self.fuzzers()
            .find(|fuzzer| crate::Fuzzer::name(fuzzer) == name)
    }

    pub fn driver_symbols(&self) -> Vec<&'static str> {
        crate::wire::IN_PROCESS_DRIVER_SYMBOLS
            .iter()
            .copied()
            .chain(
                self.fuzzers()
                    .filter_map(|fuzzer| crate::Fuzzer::driver_symbol(&fuzzer)),
            )
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn driver_in(
        &self,
        bin: &std::path::Path,
    ) -> Result<Option<&'static str>, crate::object::ProbeError> {
        let drivers = self.driver_symbols();
        let defined = crate::object::defined([bin], &drivers)?;
        Ok(drivers.into_iter().find(|driver| defined.contains(driver)))
    }
}

impl AnyEngine {
    pub fn selection_label(&self) -> String {
        format!("{} (--engine={})", self.display_name(), self.name())
    }

    pub fn descriptor(&self) -> EngineDescriptor {
        match self {
            AnyEngine::AustenDriven(generator) => crate::GeneratorFactory::descriptor(generator),
            AnyEngine::Delegated(engine) => EngineDescriptor {
                loop_owner: LoopOwner::DelegatedTool,
                generic_seed: false,
                entropy_tape: false,
                function_selection: FunctionSelectionOwner::DelegatedTool,
                producer_options: false,
                max_array_length: true,
                coverage: CoveragePolicy {
                    observed_by_austen: false,
                    can_run_blind: DelegatedEngine::capabilities(engine).blind,
                },
            },
        }
    }
}

pub struct LaunchCandidateGate<'a> {
    validator: &'a dyn ArtifactValidator,
    reporter: &'a observe::Reporter,
    journal: RefCell<&'a mut Journal>,
}

impl<'a> LaunchCandidateGate<'a> {
    pub fn new(
        validator: &'a dyn ArtifactValidator,
        reporter: &'a observe::Reporter,
        journal: &'a mut Journal,
    ) -> Self {
        Self {
            validator,
            reporter,
            journal: RefCell::new(journal),
        }
    }
}

impl CandidateGate for LaunchCandidateGate<'_> {
    fn validate(&self, candidates: &[SeedCandidate]) -> Result<Vec<usize>, CandidateGateError> {
        let _phase = self.reporter.phase(observe::Phase::ValidateSeeds);
        let mut indices = std::collections::HashMap::with_capacity(candidates.len());
        let batch = candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| {
                if indices
                    .insert(candidate.source().to_path_buf(), index)
                    .is_some()
                {
                    return Err(CandidateGateError::new(format!(
                        "candidate source {} appeared more than once",
                        candidate.source().display()
                    )));
                }
                Ok((candidate.source().to_path_buf(), candidate.bytes().to_vec()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut session = LaunchValidationSession::new(self.validator);
        let mut journal = self.journal.borrow_mut();
        let canonical = findings::validate_in_chunks(batch, self.reporter, |chunk| {
            session.validate_batch(chunk, &mut journal)
        })
        .map_err(|error| CandidateGateError::new(error.to_string()))?;
        canonical
            .into_iter()
            .map(|candidate| {
                indices.get(candidate.source()).copied().ok_or_else(|| {
                    CandidateGateError::new(format!(
                        "validator returned unknown candidate source {}",
                        candidate.source().display()
                    ))
                })
            })
            .collect()
    }

    fn promoted(&self, seeds: &[StagedSeed]) -> Result<(), CandidateGateError> {
        let mut journal = self.journal.borrow_mut();
        for seed in seeds {
            journal
                .record_required(&CoverageEntry {
                    elapsed_ms: Some(0),
                    why: Provenance::Calibration,
                    timing: Timing::Observed,
                    path: seed.path.clone(),
                    bytes: seed.bytes,
                })
                .map_err(|error| CandidateGateError::new(error.to_string()))?;
        }
        Ok(())
    }
}

impl Engine for AnyDelegatedEngine {
    fn name(&self) -> &'static str {
        DelegatedEngine::name(self)
    }

    fn display_name(&self) -> &'static str {
        DelegatedEngine::display_name(self)
    }

    fn profiles(&self) -> Vec<PassProfile> {
        vec![PassProfile::NONE]
    }

    fn campaign(&self, cfg: CampaignConfig) -> Result<CampaignReport, CampaignError> {
        if cfg.exit_fast {
            return Err(CampaignError::Unrunnable(
                "--exit-fast requires an Austen-driven campaign whose loop can stop after a finding"
                    .into(),
            ));
        }
        if cfg.strict_duration {
            return Err(CampaignError::Unrunnable(
                "--strict-duration requires an Austen-driven campaign whose loop owns the dispatch boundary"
                    .into(),
            ));
        }
        if cfg.tyche.is_some() {
            return Err(CampaignError::Unrunnable(
                "Tyche output requires an Austen-driven campaign whose individual attempts are observable".into(),
            ));
        }
        crate::campaign::validate_functions(&cfg.functions)?;
        crate::campaign::validate_generator_modules(
            &cfg.functions,
            &cfg.wire_module,
            &cfg.production_modules,
        )?;
        if cfg.budget == Budget::Tape {
            let engine = AnyEngine::Delegated(*self).selection_label();
            return Err(CampaignError::Unrunnable(format!(
                "{engine} owns its own entropy, so it cannot be bounded by a \
                 replayed tape — give it a duration, or none at all",
            )));
        }
        let duration_secs = cfg.budget.seconds();
        let function_ids: Vec<u8> = cfg
            .functions
            .iter()
            .map(|function| function.target_id)
            .collect();
        let started = std::time::SystemTime::now();
        let mut journal = Journal::open(&cfg.journal, DelegatedEngine::name(self), started);
        let folders = findings::ArtifactFolders::new(&cfg.corpus, &cfg.crashes);
        let seed_sources = folders.launch_seed_sources();
        let protected_paths = cfg.output_paths(&folders);
        let outcome = {
            let candidate_gate = LaunchCandidateGate {
                validator: cfg.validator.as_ref(),
                reporter: &cfg.reporter,
                journal: RefCell::new(&mut journal),
            };
            DelegatedEngine::campaign(
                self,
                &ExternalRequest {
                    harness: &cfg.harness,
                    generator: &cfg.generator_path,
                    work: &cfg.work,
                    function_ids: &function_ids,
                    seed_sources: &seed_sources,
                    protected_paths: &protected_paths,
                    candidate_gate: &candidate_gate,
                    heap_bytes: cfg.heap_bytes,
                    allow_unstable_addresses: cfg.allow_unstable_addresses,
                    duration_secs,
                    corpus_snapshot_interval: cfg.corpus_snapshot_interval,
                    build_only: matches!(cfg.budget, Budget::Build),
                    instrumented: cfg.instrumented,
                    instrumentation: cfg.instrumentation,
                    tool_output: cfg.tool_output,
                    reporter: &cfg.reporter,
                    cancel: &cfg.cancel,
                },
            )
        };
        let mut outcome = outcome?;

        let (findings, corpus, retained, capture_degradation) = match &outcome.artifacts {
            None => (
                Crashes::default(),
                None,
                findings::RetainedArtifacts::default(),
                None,
            ),
            Some(Artifacts {
                crashes_dir,
                corpus_dir,
                not_findings,
                captured,
                capture_error,
            }) => {
                let _phase = cfg.reporter.phase(observe::Phase::ImportFindings);
                let (found, kept, retained, capture_degradation) = findings::archive_external(
                    crashes_dir,
                    corpus_dir.as_deref(),
                    captured,
                    capture_error.as_deref(),
                    &folders,
                    not_findings,
                    &Replay {
                        harness: &cfg.harness,
                        env: &outcome.env,
                        tally: &cfg.work.join("replay-tally"),
                        cover: None,
                    },
                    &cfg.reporter,
                    cfg.validator.as_ref(),
                    &mut journal,
                )?;
                (found, Some(kept), retained, capture_degradation)
            }
        };
        if let (Some(artifacts), Some(error)) = (outcome.artifacts.as_mut(), capture_degradation) {
            artifacts.capture_error = Some(error);
        }

        if let Some(why) = journal.broken() {
            let message = format!("the corpus journal could not be written ({why})");
            cfg.reporter
                .emit(|| RunEvent::Warning(Warning::CoverageUnavailable { message }));
        }

        check_delegated_startup(
            cfg.budget,
            cfg.cancel.is_cancelled(),
            outcome.tally,
            &cfg.harness,
        )?;
        if let Some(error) = oracle_campaign_error(outcome.tally, outcome.oracle_status) {
            return Err(error);
        }
        crate::campaign::warn_oracle_unsupported(&cfg.reporter, outcome.oracle_status);

        Ok(CampaignReport {
            engine: Some(outcome.engine),
            corpus_snapshot_interval: cfg.corpus_snapshot_interval,
            findings,
            corpus,
            retained,
            tally: outcome.tally,
            oracle: outcome.oracle_status,
            internal: None,
            external: Some(outcome),
        })
    }
}

fn check_delegated_startup(
    budget: Budget,
    cancelled: bool,
    tally: Option<Tally>,
    harness: &std::path::Path,
) -> Result<(), CampaignError> {
    if budget != Budget::Build && !cancelled && tally.is_none() {
        return Err(crate::external::ExternalError::Spawn {
            program: harness.display().to_string(),
            source: std::io::Error::other(
                "delegated campaign ended without a claimed harness tally; \
                 harness startup was not observed; inspect the delegated tool's \
                 output for startup diagnostics",
            ),
        }
        .into());
    }
    Ok(())
}

fn oracle_campaign_error(
    tally: Option<Tally>,
    status: Option<OracleStatus>,
) -> Option<CampaignError> {
    match (status, tally) {
        (Some(status), _) if status.pre_internal_error > 0 => {
            Some(CampaignError::OracleInternal(format!(
                "the harness reported {} internal precondition error(s)",
                status.pre_internal_error,
            )))
        }
        (Some(status), Some(tally)) if status.observed() > tally.selected => {
            Some(CampaignError::OracleInternal(format!(
                "the harness reported an oracle result without a selection \
                 (selected: {}, oracle results: {})",
                tally.selected,
                status.observed(),
            )))
        }
        (Some(status), Some(tally)) if status.pre_passed < tally.dispatched => {
            Some(CampaignError::OracleInternal(format!(
                "the harness dispatched {} input(s) but reported only {} passed precondition(s)",
                tally.dispatched, status.pre_passed,
            )))
        }
        (Some(status), Some(tally)) if tally.selected > 0 && status.observed() == 0 => {
            Some(CampaignError::OracleInternal(format!(
                "the harness selected {} input(s) and reported no oracle results",
                tally.selected,
            )))
        }
        (None, Some(tally)) if tally.selected > 0 => Some(CampaignError::OracleInternal(
            "the harness selected campaign inputs without reporting oracle results".into(),
        )),
        _ => None,
    }
}
