pub mod coverage;
mod generator_sources;
mod schedule;
pub mod shrink;
pub mod suite;

use crate::{object, observe};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub use crate::gen_dsl::{FunctionTableError, Label, RecordingError, ScopeError, Sym};
pub use crate::wire::{DEBUG_CHILD_ENV, DEFAULT_HEAP_BYTES, TestCaseSize, is_default_base};
pub use coverage::{CoverageOutcome, CoverageSummary, EvaluationBinary};
pub use generator_sources::{
    GeneratorSourceError, LoadedGeneratorSources, spec_without_generators,
};
pub use shrink::ShrinkSummary;
pub use suite::{SuiteMinimizationSummary, SuiteOutcomeSummary, SuitePassSummary};

use crate::shrink::{AnyShrinker, SuiteTool, SuiteToolHandle};
use crate::spec::{ConvertError, Post};
use crate::{Budget, external::ToolOutput, instrumentation::Instrumentation};

use crate::gen_dsl::{DslFormatter, Entropy, GenDef, GenModule, recording};
pub use crate::gen_dsl::{
    choice::translate::MAX_MAP_ITERATIONS,
    passes::{InlineMode, PassOptions, PassProfile},
};
use crate::{
    CampaignConfig, CampaignError, CampaignFunction, CampaignReport, Corpus, Crashes,
    FunctionSelectorKind, Journal, LoopStats, Measured, ProductionModules, ProtectedPath,
    Provenance, RetainedArtifact, RetainedArtifacts, Schedule, SizeSelectorKind, Timing, Verdict,
    findings::{
        ArtifactFolders, HarnessValidator, PreconditionValidation, Store, ValidationSession,
    },
    internal::target,
    resolve_corpus_snapshot_interval,
};
pub const DEFAULT_MAX_GENERATOR_SIZE: TestCaseSize =
    TestCaseSize::new(crate::gen_dsl::rand_like::DEFAULT_MAX_SIZE)
        .expect("the default generator-size bound is valid");
pub const DEFAULT_GENERATOR_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1000);
pub use crate::observe::{Cancel, Phase, Reporter, RunEvent};
use crate::spec::SpecModule;
use crate::wire::{AustenInput, Trust};

#[derive(Debug, Clone)]
pub struct RunConfig {
    pub spec: PathBuf,
    pub function: Option<String>,
    pub custom_generators: Option<LoadedGeneratorSources>,
    pub ignore_custom_generators: bool,
    pub dsl_log_dir: Option<PathBuf>,
    pub pass_options: PassOptions,
    pub harness: PathBuf,
    pub schedule: Option<Schedule>,
    pub stage_harnesses: Vec<Option<HarnessFacts>>,
    pub function_selector: FunctionSelectorKind,
    pub size_selector: SizeSelectorKind,
    pub max_test_case_size: TestCaseSize,
    pub duration_secs: Option<u64>,
    pub corpus_snapshot_interval: Option<Duration>,
    pub build_only: bool,
    pub exit_fast: bool,
    pub strict_duration: bool,
    pub seed: u64,
    pub generator_timeout: Option<std::time::Duration>,
    pub chain_discard_limit: Option<NonZeroU64>,
    pub input: Option<AustenInput>,
    pub record: Option<PathBuf>,
    pub tyche: Option<PathBuf>,
    pub replay: Option<PathBuf>,
    pub crashes: PathBuf,
    pub corpus: PathBuf,
    pub shrink: Option<AnyShrinker>,
    pub shrink_harness: Option<PathBuf>,
    pub suite_minimizer: Option<SuiteToolHandle>,
    pub cov_harness: Result<EvaluationBinary, String>,
    pub additional_protected_paths: Vec<ProtectedPath>,
    pub work: PathBuf,
    pub heap_bytes: u64,
    pub allow_unstable_addresses: bool,
    pub instrumented: bool,
    pub instrumentation: Instrumentation,
    pub coverage_guards: Option<u64>,
    pub debug_child: bool,
    pub tool_output: ToolOutput,
    pub reporter: Reporter,
    pub cancel: Cancel,
}

impl RunConfig {
    pub fn new(
        spec: PathBuf,
        harness: PathBuf,
        work: PathBuf,
        schedule: impl Into<Option<Schedule>>,
    ) -> Self {
        Self {
            crashes: PathBuf::from("./crashes"),
            corpus: PathBuf::from("./corpus"),
            shrink: None,
            shrink_harness: None,
            suite_minimizer: None,
            cov_harness: Err("no evaluation binary was provided".to_string()),
            generator_timeout: Some(DEFAULT_GENERATOR_TIMEOUT),
            chain_discard_limit: None,
            additional_protected_paths: Vec::new(),
            spec,
            function: None,
            custom_generators: None,
            ignore_custom_generators: false,
            dsl_log_dir: None,
            pass_options: PassOptions::NONE,
            harness,
            schedule: schedule.into(),
            stage_harnesses: Vec::new(),
            function_selector: FunctionSelectorKind::default(),
            size_selector: SizeSelectorKind::default(),
            max_test_case_size: DEFAULT_MAX_GENERATOR_SIZE,
            duration_secs: None,
            corpus_snapshot_interval: None,
            build_only: false,
            exit_fast: false,
            strict_duration: false,
            seed: 0,
            input: None,
            record: None,
            tyche: None,
            replay: None,
            work,
            heap_bytes: DEFAULT_HEAP_BYTES,
            allow_unstable_addresses: false,
            instrumented: false,
            instrumentation: Instrumentation::Absent,
            coverage_guards: None,
            debug_child: false,
            tool_output: ToolOutput::Silence,
            reporter: Reporter::silent(),
            cancel: Cancel::never(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessFacts {
    pub harness: PathBuf,
    pub instrumented: bool,
    pub instrumentation: Instrumentation,
    pub coverage_guards: Option<u64>,
}

#[derive(Debug)]
pub struct StageOutcome {
    pub index: usize,
    pub engine: String,
    pub budget: Budget,
    pub seed: Option<u64>,
    pub harness: PathBuf,
    pub elapsed: Duration,
    pub campaign: Option<CampaignReport>,
    pub failed: bool,
}

pub struct Report {
    pub function: Option<String>,
    pub entropy: Option<EntropySource>,
    pub budget: Option<Budget>,
    pub campaign: CampaignReport,
    pub stages: Vec<StageOutcome>,
    pub shrink: Option<ShrinkSummary>,
    pub suite_minimization: Option<SuiteMinimizationSummary>,
    pub coverage: CoverageOutcome,
}

impl RunConfig {
    pub fn journal(&self) -> PathBuf {
        self.work.join("corpus.journal")
    }

    fn campaign_protected_paths(&self) -> Vec<ProtectedPath> {
        let mut paths = vec![ProtectedPath::new("specification", self.spec.clone())];
        if let Some(path) = &self.dsl_log_dir {
            paths.push(ProtectedPath::new("DSL log directory", path.clone()));
        }
        if let Some(path) = &self.replay {
            paths.push(ProtectedPath::new("replay recording", path.clone()));
        }
        if let Some(path) = &self.shrink_harness {
            paths.push(ProtectedPath::new("shrink harness", path.clone()));
        }
        for facts in self.stage_harnesses.iter().flatten() {
            paths.push(ProtectedPath::new("stage harness", facts.harness.clone()));
        }
        if let Ok(binary) = &self.cov_harness {
            paths.push(ProtectedPath::new(
                "evaluation harness",
                binary.harness.clone(),
            ));
            if let Some(path) = &binary.object {
                paths.push(ProtectedPath::new("evaluation object", path.clone()));
            }
        }
        paths.extend(self.additional_protected_paths.iter().cloned());
        paths
    }

    pub fn stage_harness(&self, index: usize) -> HarnessFacts {
        self.stage_harnesses
            .get(index)
            .and_then(Option::as_ref)
            .cloned()
            .unwrap_or_else(|| HarnessFacts {
                harness: self.harness.clone(),
                instrumented: self.instrumented,
                instrumentation: self.instrumentation,
                coverage_guards: self.coverage_guards,
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntropySource {
    Seeded(u64),
    Replayed { path: PathBuf, bytes: Option<usize> },
}

impl EntropySource {
    pub fn planned(cfg: &RunConfig) -> Self {
        match &cfg.replay {
            Some(path) => EntropySource::Replayed {
                path: path.clone(),
                bytes: None,
            },
            None => EntropySource::Seeded(cfg.seed),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    #[error("read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not entropy this run can replay: {source}")]
    NotARecording {
        path: PathBuf,
        #[source]
        source: crate::gen_dsl::RecordingError,
    },
    #[error("parse {path}: {message}")]
    ParseSpec { path: PathBuf, message: String },
    #[error(transparent)]
    GeneratorSource(GeneratorSourceError),
    #[error("the specification in {path} has no functions")]
    NoFunctions { path: PathBuf },
    #[error("the specification in {path} has no function named {function}")]
    NoSuchFunction { path: PathBuf, function: String },
    #[error("the specification in {path} declares function {name} more than once")]
    DuplicateFunction { path: PathBuf, name: String },
    #[error(
        "the specification in {path} declares {count} functions; at most {max} fit in the selector byte"
    )]
    TooManyFunctions {
        path: PathBuf,
        count: usize,
        max: usize,
    },
    #[error("convert {function}: {message}")]
    Convert { function: String, message: String },
    #[error(
        "converting {function} produced the ill-scoped generator {generator:?}: \
         {source} — this is a defect in a conversion pass, not something the \
         specification asked for"
    )]
    NotWellScoped {
        function: String,
        generator: Sym,
        #[source]
        source: ScopeError,
    },
    #[error(
        "converting {function}: the logical-function table is malformed: \
         {source} — the module's Def/RecDef declarations cannot be trusted, \
         not something the specification's own constraints asked for"
    )]
    MalformedFunctionTable {
        function: String,
        #[source]
        source: FunctionTableError,
    },
    #[error(
        "staging under the pass profiles of {first} and {other} froze different wire \
         modules; the wire must not depend on the profile — a defect in a pass, \
         not something the specification asked for"
    )]
    WireDiverged { first: String, other: String },
    #[error("supplied-input validation and execution disagree: {0}")]
    BadInput(String),
    #[error(
        "{var} is set in this process's environment and a harness spawned from \
         it would refuse to dispatch — unset it before running"
    )]
    HostileEnv { var: &'static str },
    #[error("harness {path} cannot be used safely: {message}")]
    HarnessOracleCapability { path: PathBuf, message: String },
    #[error("{0}")]
    Campaign(#[from] CampaignError),
    #[error("stage {index} ({engine}) failed: {source}")]
    StageFailed {
        index: usize,
        engine: String,
        #[source]
        source: Box<CampaignError>,
        stages: Vec<StageOutcome>,
        merged: Option<Box<CampaignReport>>,
    },
}

impl From<GeneratorSourceError> for DriverError {
    fn from(error: GeneratorSourceError) -> Self {
        match error {
            GeneratorSourceError::Read { path, source } => Self::Read { path, source },
            GeneratorSourceError::Spec { path, source } => Self::ParseSpec {
                path,
                message: source.to_string(),
            },
            GeneratorSourceError::Functions { path, source } => convert_error(&path, None, *source),
            GeneratorSourceError::UnknownFunction { path, function } => {
                Self::NoSuchFunction { path, function }
            }
            error => Self::GeneratorSource(error),
        }
    }
}

fn entropy(cfg: &RunConfig) -> Result<(Entropy, EntropySource), DriverError> {
    Ok(match EntropySource::planned(cfg) {
        EntropySource::Seeded(seed) => (Entropy::seeded(seed), EntropySource::Seeded(seed)),
        EntropySource::Replayed { path, .. } => {
            let file = std::fs::read(&path).map_err(|source| DriverError::Read {
                path: path.clone(),
                source,
            })?;
            let inputs =
                recording::unframe(&file).map_err(|source| DriverError::NotARecording {
                    path: path.clone(),
                    source,
                })?;
            let count = inputs.len();
            let bytes: usize = inputs.iter().map(Vec::len).sum();
            let entropy = Entropy::replay_inputs(inputs, cfg.seed);
            cfg.reporter.emit(|| RunEvent::ReplayLoaded {
                path: path.clone(),
                inputs: count,
                bytes,
            });
            (
                entropy,
                EntropySource::Replayed {
                    path,
                    bytes: Some(bytes),
                },
            )
        }
    })
}

pub struct StagedFunction {
    pub target_id: u8,
    pub name: String,
    pub generator: Sym,
    pub post: Post,
    pub post_defaults: crate::gen_dsl::DefaultTypes,
}

pub struct StagedGenerator {
    pub functions: Vec<StagedFunction>,
    pub wire_module: GenModule,
    pub production_modules: ProductionModules,
    pub generator_path: PathBuf,
}

impl StagedGenerator {
    pub fn function(&self) -> Option<&str> {
        match self.functions.as_slice() {
            [only] => Some(&only.name),
            _ => None,
        }
    }
}

fn convert_error(path: &Path, selected: Option<&str>, e: ConvertError) -> DriverError {
    let function = selected.unwrap_or("module").to_string();
    match e {
        ConvertError::NoFunctions => DriverError::NoFunctions {
            path: path.to_path_buf(),
        },
        ConvertError::DuplicateFunction { name } => DriverError::DuplicateFunction {
            path: path.to_path_buf(),
            name,
        },
        ConvertError::TooManyFunctions { count, max } => DriverError::TooManyFunctions {
            path: path.to_path_buf(),
            count,
            max,
        },
        ConvertError::NotWellScoped { name, error } => DriverError::NotWellScoped {
            function,
            generator: name,
            source: error,
        },
        ConvertError::MalformedFunctionTable { error } => DriverError::MalformedFunctionTable {
            function,
            source: error,
        },
        ConvertError::UnsupportedParam(_)
        | ConvertError::DuplicateGlobalAddress { .. }
        | ConvertError::DuplicateGlobalName { .. }
        | ConvertError::DuplicateInternalGlobal { .. }
        | ConvertError::EmptyGlobalName { .. }
        | ConvertError::EmptyGlobalOwner { .. }
        | ConvertError::ImmutableGlobal { .. }
        | ConvertError::UnsupportedGlobalType { .. }
        | ConvertError::UnknownPredicate { .. }
        | ConvertError::UninterpretedPredicate { .. }
        | ConvertError::NoClauses { .. }
        | ConvertError::GuardedLastClause { .. }
        | ConvertError::DuplicateGenerator { .. }
        | ConvertError::NotAMapBaseType { .. }
        | ConvertError::IteBranchMismatch { .. }
        | ConvertError::DuplicateRecordMember { .. }
        | ConvertError::Transform(_)
        | ConvertError::InvalidGeneratorModule { .. }
        | ConvertError::UnsupportedField { .. }
        | ConvertError::UnknownFunction { .. }
        | ConvertError::UnknownTargetId { .. } => DriverError::Convert {
            function,
            message: format!("{e:?}"),
        },
    }
}

pub fn stage_generator(
    spec: &Path,
    function: Option<&str>,
    work: &Path,
    dsl_log_dir: Option<&Path>,
    profiles: &[(PassProfile, String)],
    pass_options: PassOptions,
) -> Result<StagedGenerator, DriverError> {
    let text = std::fs::read_to_string(spec).map_err(|source| DriverError::Read {
        path: spec.to_path_buf(),
        source,
    })?;
    stage_generator_text(
        spec,
        &text,
        function,
        work,
        dsl_log_dir,
        profiles,
        pass_options,
    )
}

fn stage_generator_text(
    spec: &Path,
    text: &str,
    function: Option<&str>,
    work: &Path,
    dsl_log_dir: Option<&Path>,
    profiles: &[(PassProfile, String)],
    pass_options: PassOptions,
) -> Result<StagedGenerator, DriverError> {
    assert!(
        !profiles.is_empty(),
        "stage_generator needs at least one pass profile to stage under"
    );
    let module = SpecModule::from_json(text).map_err(|e| DriverError::ParseSpec {
        path: spec.to_path_buf(),
        message: e.to_string(),
    })?;
    crate::spec::validate_function_table(&module)
        .map_err(|error| convert_error(spec, function, error))?;
    let selected_target_id = function
        .map(|want| {
            module
                .functions
                .iter()
                .position(|candidate| candidate.name == want)
                .map(|target_id| target_id as u8)
                .ok_or_else(|| DriverError::NoSuchFunction {
                    path: spec.to_path_buf(),
                    function: want.to_string(),
                })
        })
        .transpose()?;
    let mut first: Option<(
        GenModule,
        Vec<StagedFunction>,
        crate::spec::SpecModule,
        &str,
    )> = None;
    let mut production_modules = ProductionModules::default();
    for (profile, label) in profiles {
        let mut snapshots: BTreeMap<&'static str, Vec<GenDef>> = BTreeMap::new();
        let staged_conversion = crate::spec::transform_staged_observed_with_options(
            module.clone(),
            selected_target_id,
            *profile,
            pass_options,
            &mut |stage, _, def| {
                if dsl_log_dir.is_some() {
                    snapshots.entry(stage).or_default().push(def.clone());
                }
            },
        )
        .map_err(|e| convert_error(spec, function, e))?;
        let crate::spec::StagedConversion {
            conversion,
            spec: oracle_spec,
        } = staged_conversion;
        if let Some(dir) = dsl_log_dir {
            if profiles.len() == 1 {
                write_dsl_logs(dir, snapshots)?;
            } else {
                write_dsl_logs(&dir.join(label), snapshots)?;
            }
        }
        let functions: Vec<StagedFunction> = conversion
            .functions
            .into_iter()
            .map(|converted| StagedFunction {
                target_id: converted.target_id,
                name: converted.name,
                generator: converted.generator,
                post: converted.post,
                post_defaults: converted.post_defaults,
            })
            .collect();
        match &first {
            None => first = Some((conversion.wire, functions, oracle_spec, label)),
            Some((wire, _, _, first_label)) => {
                if *wire != conversion.wire {
                    return Err(DriverError::WireDiverged {
                        first: first_label.to_string(),
                        other: label.clone(),
                    });
                }
            }
        }
        production_modules.insert(*profile, Arc::new(conversion.production));
    }
    let (wire_module, functions, oracle_spec, _) = first.expect("at least one profile was staged");

    std::fs::create_dir_all(work).map_err(|source| DriverError::Write {
        path: work.to_path_buf(),
        source,
    })?;
    let generator_path = work.join("generator.json");
    let harness_module = crate::wire::HarnessModuleRef {
        functions: functions
            .iter()
            .map(|function| crate::wire::HarnessFunctionRef {
                target_id: function.target_id,
                name: &function.name,
                generator: &function.generator,
                post: &function.post,
                post_defaults: &function.post_defaults,
            })
            .collect(),
        generator_module: &wire_module,
        spec: &oracle_spec,
    };
    std::fs::write(
        &generator_path,
        serde_json::to_vec(&harness_module).expect("harness module serializes"),
    )
    .map_err(|source| DriverError::Write {
        path: generator_path.clone(),
        source,
    })?;
    Ok(StagedGenerator {
        functions,
        wire_module,
        production_modules,
        generator_path,
    })
}

fn write_dsl_logs(
    dir: &Path,
    mut snapshots: BTreeMap<&'static str, Vec<GenDef>>,
) -> Result<(), DriverError> {
    std::fs::create_dir_all(dir).map_err(|source| DriverError::Write {
        path: dir.to_path_buf(),
        source,
    })?;

    let formatter = DslFormatter::default();
    for stage in crate::spec::OBSERVED_STAGES {
        let defs = snapshots
            .remove(stage)
            .expect("transform_observed emits every observed stage");
        let path = dir.join(format!("{stage}.log"));
        std::fs::write(&path, formatter.render_defs(&defs)).map_err(|source| {
            DriverError::Write {
                path: path.clone(),
                source,
            }
        })?;
    }
    assert!(
        snapshots.is_empty(),
        "OBSERVED_STAGES is missing emitted stage(s): {:?}",
        snapshots.keys().collect::<Vec<_>>()
    );
    Ok(())
}

fn require_harness_oracle_capability(path: &Path) -> Result<(), DriverError> {
    match object::mentions_symbol(path, crate::wire::ORACLE_CAPABILITY_SYMBOL) {
        Ok(true) => Ok(()),
        Ok(false) => Err(DriverError::HarnessOracleCapability {
            path: path.to_path_buf(),
            message: format!(
                "it does not carry `{}`; rebuild it with this Austen runtime before running inputs",
                crate::wire::ORACLE_CAPABILITY_SYMBOL,
            ),
        }),
        Err(error) => Err(DriverError::HarnessOracleCapability {
            path: path.to_path_buf(),
            message: format!(
                "its `{}` capability stamp could not be inspected ({error})",
                crate::wire::ORACLE_CAPABILITY_SYMBOL,
            ),
        }),
    }
}

pub fn uses_tree_generators(schedule: &Schedule) -> bool {
    schedule.descriptor().producer_options
}

pub fn run(cfg: &RunConfig) -> Result<Report, DriverError> {
    let selected = cfg.schedule.as_ref();
    if cfg.input.is_none() && selected.is_none() {
        return Err(DriverError::BadInput(
            "generation requires an engine".into(),
        ));
    }
    let single_austen = selected.is_some_and(|schedule| {
        schedule.is_single() && schedule.descriptor().loop_owner == crate::LoopOwner::Austen
    });
    if cfg.exit_fast
        && (cfg.build_only || cfg.input.is_some() || cfg.replay.is_some() || !single_austen)
    {
        return Err(DriverError::BadInput(
            "--exit-fast requires a single-stage Austen-driven live campaign".into(),
        ));
    }
    if cfg.strict_duration
        && (cfg.build_only
            || cfg.input.is_some()
            || cfg.replay.is_some()
            || !single_austen
            || !selected.is_some_and(|schedule| matches!(
                schedule::total_budget(schedule, cfg.build_only, cfg.replay.is_some(), cfg.duration_secs),
                Budget::Seconds(seconds) if seconds > 0
            )))
    {
        return Err(DriverError::BadInput(
            "--strict-duration requires a single-stage Austen-driven live campaign with an explicit nonzero duration"
                .into(),
        ));
    }
    if cfg.tyche.is_some() {
        if cfg.build_only {
            return Err(DriverError::BadInput(
                "--tyche requires a running test".into(),
            ));
        }
        if cfg.input.is_none()
            && selected.is_some_and(|schedule| {
                schedule.descriptor().loop_owner == crate::LoopOwner::DelegatedTool
            })
        {
            return Err(DriverError::BadInput(
                "--tyche requires an Austen-driven campaign or a supplied input".into(),
            ));
        }
        if cfg.input.is_none() && selected.is_some_and(|schedule| !schedule.is_single()) {
            return Err(DriverError::BadInput(
                "--tyche writes one campaign's attempts; a multi-stage --engine runs several"
                    .into(),
            ));
        }
    }
    let custom_generators = if cfg.input.is_none()
        && selected.is_some_and(uses_tree_generators)
        && !cfg.ignore_custom_generators
    {
        Some(match &cfg.custom_generators {
            Some(sources) => {
                sources.validate_context(&cfg.spec, cfg.function.as_deref())?;
                Cow::Borrowed(sources)
            }
            None => Cow::Owned(LoadedGeneratorSources::load(
                &cfg.spec,
                cfg.function.as_deref(),
            )?),
        })
    } else {
        None
    };
    let mut staged = {
        let _phase = cfg.reporter.phase(Phase::Convert);
        let text = match &custom_generators {
            Some(sources) => Cow::Borrowed(sources.spec_text()),
            None => Cow::Owned(std::fs::read_to_string(&cfg.spec).map_err(|source| {
                DriverError::Read {
                    path: cfg.spec.clone(),
                    source,
                }
            })?),
        };
        stage_generator_text(
            &cfg.spec,
            &text,
            if cfg.input.is_none() {
                cfg.function.as_deref()
            } else {
                None
            },
            &cfg.work.join("gen"),
            cfg.dsl_log_dir.as_deref(),
            &match selected {
                Some(schedule) if cfg.input.is_none() => schedule.labelled_profiles(),
                _ => vec![(PassProfile::NONE, "none".to_string())],
            },
            cfg.pass_options,
        )?
    };
    if let Some(sources) = &custom_generators {
        sources.attach(&mut staged.production_modules);
        for (_, producer) in staged.production_modules.custom_iter() {
            producer
                .validate(&staged.wire_module)
                .map_err(CampaignError::from)?;
        }
    }

    if let Some(var) = crate::wire::inherited_refusal() {
        return Err(DriverError::HostileEnv { var });
    }

    if cfg.input.is_some() {
        require_harness_oracle_capability(&cfg.harness)?;
        let supplied_journal_offset = std::fs::metadata(cfg.journal())
            .map(|meta| meta.len())
            .unwrap_or(0);
        let campaign = run_supplied_input(cfg, &staged)?;
        let function = cfg
            .input
            .as_ref()
            .and_then(|input| selected_function(&staged, input))
            .map(|function| function.name.clone());
        let coverage = coverage_outcome(
            cfg,
            &staged.generator_path,
            Ok(()),
            campaign.corpus_snapshot_interval,
            true,
            &schedule::ScheduleClock::single(supplied_journal_offset),
        );
        return Ok(Report {
            function,
            entropy: None,
            budget: None,
            campaign,
            stages: Vec::new(),
            shrink: None,
            suite_minimization: None,
            coverage,
        });
    }

    let schedule = selected.expect("generation has a selected engine");

    let (entropy, entropy_source) = entropy(cfg)?;

    require_harness_oracle_capability(&cfg.harness)?;
    for facts in cfg.stage_harnesses.iter().flatten() {
        if facts.harness != cfg.harness {
            require_harness_oracle_capability(&facts.harness)?;
        }
    }
    let budget = schedule::total_budget(
        schedule,
        cfg.build_only,
        cfg.replay.is_some(),
        cfg.duration_secs,
    );
    let corpus_snapshot_interval =
        resolve_corpus_snapshot_interval(cfg.corpus_snapshot_interval, budget);

    let generator_path = staged.generator_path.clone();
    let selected_function = staged.function().map(str::to_owned);
    let validator: Arc<dyn crate::findings::ArtifactValidator> =
        Arc::new(checked_validator(cfg, &generator_path));
    let functions: Vec<CampaignFunction> = staged
        .functions
        .iter()
        .map(|function| CampaignFunction {
            post_defaults: function.post_defaults.clone(),
            target_id: function.target_id,
            name: function.name.clone(),
            generator: function.generator.clone(),
        })
        .collect();

    let schedule_started = std::time::Instant::now();
    let mut clock = schedule::ScheduleClock::default();
    let mut seeds = schedule::StageSeeds::new(cfg.seed);
    let mut stages: Vec<StageOutcome> = Vec::new();
    let mut replay_entropy = Some(entropy);
    let count = schedule.len();
    for (index, stage) in schedule.stages().iter().enumerate() {
        if index > 0 && cfg.cancel.is_cancelled() {
            break;
        }
        let stage_budget = schedule::stage_budget(
            stage.budget,
            index + 1 == count,
            count == 1,
            cfg.build_only,
            cfg.replay.is_some(),
            cfg.duration_secs,
        );
        let seed = stage.engine.descriptor().generic_seed.then(|| seeds.next());
        let stage_entropy = match replay_entropy.take() {
            Some(entropy) if cfg.replay.is_some() => entropy,
            _ => Entropy::seeded(seed.unwrap_or(cfg.seed)),
        };
        let facts = cfg.stage_harness(index);
        clock.journal_offsets.push(
            std::fs::metadata(cfg.journal())
                .map(|meta| meta.len())
                .unwrap_or(0),
        );
        clock
            .stage_offsets_ms
            .push(u64::try_from(schedule_started.elapsed().as_millis()).unwrap_or(u64::MAX));
        if count > 1 {
            let engine = stage.engine.name().to_string();
            cfg.reporter.emit(|| RunEvent::StageStarted {
                index,
                count,
                engine,
                budget_secs: stage_budget.seconds(),
                seed,
            });
        }
        let stage_started = std::time::Instant::now();
        let outcome = {
            let _phase = cfg.reporter.phase(Phase::Campaign);
            stage.engine.campaign(CampaignConfig {
                functions: functions
                    .iter()
                    .map(|function| CampaignFunction {
                        post_defaults: function.post_defaults.clone(),
                        target_id: function.target_id,
                        name: function.name.clone(),
                        generator: function.generator.clone(),
                    })
                    .collect(),
                wire_module: staged.wire_module.clone(),
                production_modules: staged.production_modules.clone(),
                generator_path: generator_path.clone(),
                harness: facts.harness.clone(),
                work: cfg.work.clone(),
                crashes: cfg.crashes.clone(),
                corpus: cfg.corpus.clone(),
                heap_bytes: cfg.heap_bytes,
                allow_unstable_addresses: cfg.allow_unstable_addresses,
                max_array_length: cfg.pass_options.max_array_length,
                generator_timeout: cfg.generator_timeout,
                chain_discard_limit: cfg.chain_discard_limit,
                max_test_case_size: cfg.max_test_case_size,
                function_selector: cfg.function_selector,
                size_selector: cfg.size_selector,
                budget: stage_budget,
                exit_fast: cfg.exit_fast,
                strict_duration: cfg.strict_duration,
                corpus_snapshot_interval,
                journal: cfg.journal(),
                caller_protected_paths: {
                    let mut paths = cfg.campaign_protected_paths();
                    if let Some(sources) = &custom_generators {
                        paths.extend(sources.protected_paths());
                    }
                    paths
                },
                validator: Arc::clone(&validator),
                instrumented: facts.instrumented,
                instrumentation: facts.instrumentation,
                coverage_guards: facts.coverage_guards,
                seed: seed.unwrap_or(cfg.seed),
                entropy: stage_entropy,
                record: (count == 1).then(|| cfg.record.clone()).flatten(),
                tyche: cfg.tyche.clone(),
                debug_child: cfg.debug_child,
                tool_output: cfg.tool_output,
                reporter: cfg.reporter.clone(),
                cancel: cfg.cancel.clone(),
            })
        };
        let elapsed = stage_started.elapsed();
        let engine = stage.engine.name().to_string();
        match outcome {
            Ok(campaign) => stages.push(StageOutcome {
                index,
                engine,
                budget: stage_budget,
                seed,
                harness: facts.harness.clone(),
                elapsed,
                campaign: Some(campaign),
                failed: false,
            }),
            Err(error) if count == 1 => return Err(error.into()),
            Err(error) => {
                let (source, campaign) = match error {
                    CampaignError::WithReport { source, report } => (*source, Some(*report)),
                    other => (other, None),
                };
                stages.push(StageOutcome {
                    index,
                    engine: engine.clone(),
                    budget: stage_budget,
                    seed,
                    harness: facts.harness.clone(),
                    elapsed,
                    campaign,
                    failed: true,
                });
                let merged = schedule::merge_campaigns(
                    schedule,
                    corpus_snapshot_interval,
                    stages
                        .iter()
                        .filter_map(|stage| stage.campaign.as_ref())
                        .collect(),
                )
                .map(Box::new);
                return Err(DriverError::StageFailed {
                    index,
                    engine,
                    source: Box::new(source),
                    stages,
                    merged,
                });
            }
        }
        if cfg.cancel.is_cancelled() {
            break;
        }
    }
    let campaign = schedule::merge_campaigns(
        schedule,
        corpus_snapshot_interval,
        stages
            .iter()
            .filter_map(|stage| stage.campaign.as_ref())
            .collect(),
    )
    .expect("a schedule has at least one stage, and a failed one returned above");

    let uninstrumented_stage = stages.iter().find(|stage| {
        stage
            .campaign
            .as_ref()
            .is_some_and(|campaign| campaign.internal.is_some())
            && !cfg.stage_harness(stage.index).instrumented
    });
    let coverage_population = if cfg.build_only {
        Err("build-only ran no inputs, so there is no coverage population".to_string())
    } else if let Some(stage) = uninstrumented_stage {
        Err(format!(
            "{} carries no compatible coverage instrumentation, so the Austen-driven campaign cannot record a lossless replay population",
            stage.harness.display()
        ))
    } else {
        Ok(())
    };
    let observation_complete = stages
        .iter()
        .filter_map(|stage| stage.campaign.as_ref())
        .all(coverage_observation_complete);
    let coverage = coverage_outcome(
        cfg,
        &generator_path,
        coverage_population,
        campaign.corpus_snapshot_interval,
        observation_complete,
        &clock,
    );

    let mut shrink = None;
    let mut suite_minimization = None;
    if !cfg.build_only {
        match (&cfg.suite_minimizer, &cfg.shrink) {
            (Some(tool), maybe_shrinker) => {
                let binary = cfg
                    .cov_harness
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(EvaluationBinary::observed);
                let binary = binary
                    .as_ref()
                    .map(|binary| *binary)
                    .map_err(String::as_str);
                let first = {
                    let _phase = cfg.reporter.phase(Phase::MinimizeSuite);
                    suite::run_pass(
                        cfg,
                        tool,
                        binary,
                        &generator_path,
                        observe::SuitePass::Initial,
                        &campaign.retained,
                        maybe_shrinker.is_none(),
                    )
                };

                let mut final_pass = None;
                if let Some(shrinker) = maybe_shrinker {
                    let reductions = {
                        let _phase = cfg.reporter.phase(Phase::Shrink);
                        let run = shrink::run(
                            cfg,
                            shrinker,
                            &generator_path,
                            campaign.findings.filed.len(),
                            &first.retained,
                        );
                        shrink = Some(run.summary);
                        run.reductions
                    };
                    let union = suite::with_reductions(first.retained, reductions);
                    let second = {
                        let _phase = cfg.reporter.phase(Phase::ReminimizeSuite);
                        suite::run_pass(
                            cfg,
                            tool,
                            binary,
                            &generator_path,
                            observe::SuitePass::Final,
                            &union,
                            true,
                        )
                    };
                    final_pass = Some(second.summary);
                }

                suite_minimization = Some(SuiteMinimizationSummary {
                    tool: SuiteTool::name(tool),
                    harness: binary
                        .map(|binary| binary.harness.clone())
                        .unwrap_or_default(),
                    initial: first.summary,
                    final_pass,
                });
            }
            (None, Some(shrinker)) => {
                let _phase = cfg.reporter.phase(Phase::Shrink);
                shrink = Some(
                    shrink::run(
                        cfg,
                        shrinker,
                        &generator_path,
                        campaign.findings.filed.len(),
                        &campaign.retained,
                    )
                    .summary,
                );
            }
            (None, None) => {}
        }
    }

    Ok(Report {
        function: selected_function,
        entropy: schedule.descriptor().generic_seed.then_some(entropy_source),
        budget: Some(budget),
        campaign,
        stages,
        shrink,
        suite_minimization,
        coverage,
    })
}

fn coverage_outcome(
    cfg: &RunConfig,
    generator: &Path,
    population: Result<(), String>,
    snapshot_interval: Duration,
    observation_complete: bool,
    clock: &schedule::ScheduleClock,
) -> CoverageOutcome {
    if let Err(why) = population {
        return CoverageOutcome::Skipped(why);
    }
    match cfg
        .cov_harness
        .as_ref()
        .map_err(Clone::clone)
        .and_then(EvaluationBinary::observed)
    {
        Ok(binary) => coverage::run(
            cfg,
            binary,
            generator,
            snapshot_interval,
            observation_complete,
            clock,
        )
        .map_or_else(CoverageOutcome::Skipped, CoverageOutcome::Measured),
        Err(why) => CoverageOutcome::Skipped(why),
    }
}

fn coverage_observation_complete(campaign: &CampaignReport) -> bool {
    campaign
        .internal
        .as_ref()
        .is_none_or(|stats| stats.retention_complete)
        && !campaign
            .external
            .as_ref()
            .and_then(|outcome| outcome.artifacts.as_ref())
            .is_some_and(|artifacts| artifacts.capture_error.is_some())
}

fn checked_validator(cfg: &RunConfig, generator_path: &Path) -> HarnessValidator {
    let validation_tally = cfg.work.join("validation-tally");
    let env = crate::external::harness_env_for(
        generator_path,
        Trust::Checked,
        cfg.heap_bytes,
        &validation_tally,
        cfg.allow_unstable_addresses,
    );
    HarnessValidator::new(&cfg.harness, env)
}

fn selected_function<'a>(
    staged: &'a StagedGenerator,
    input: &crate::wire::AustenInput,
) -> Option<&'a StagedFunction> {
    let id = input.parts().ok()?.function_id;
    staged
        .functions
        .iter()
        .find(|function| function.target_id == id)
}

fn run_supplied_input(
    cfg: &RunConfig,
    staged: &StagedGenerator,
) -> Result<CampaignReport, DriverError> {
    let input = cfg.input.as_ref().expect("a supplied input");
    let mut harness = target::Harness::spawn(&target::Spawn {
        harness: &cfg.harness,
        generator_path: &staged.generator_path,
        work: &cfg.work,
        heap_bytes: cfg.heap_bytes,
        allow_unstable_addresses: cfg.allow_unstable_addresses,
        debug_child: cfg.debug_child,
        trust: Trust::Checked,
        coverage: false,
        coverage_guards: cfg.coverage_guards,
        reporter: &cfg.reporter,
        generator_module: &staged.wire_module,
    })?;
    let incoming = harness.incoming();
    let region = incoming.heap();
    let bytes = input.as_bytes().to_vec();
    let validator = checked_validator(cfg, &staged.generator_path);
    let mut journal = Journal::open(
        &cfg.journal(),
        "supplied-input",
        std::time::SystemTime::now(),
    );
    let mut validation = ValidationSession::new(&validator);
    let artifact = validation
        .validate("--input", bytes, None, &mut journal)
        .map_err(CampaignError::from)?;
    let mut tyche = crate::tyche::TycheWriter::open(
        cfg.tyche.as_deref(),
        std::time::SystemTime::now(),
        &cfg.reporter,
    );
    let execute_started = std::time::Instant::now();
    let execution = harness.run(input);
    let execute_elapsed = execute_started.elapsed();
    let crashed = match execution {
        Ok(crashed) => crashed,
        Err(error) => {
            tyche.finish(&cfg.reporter);
            return Err(error.into());
        }
    };
    let oracle = harness.oracle_status();
    let oracle_unsupported =
        oracle.is_some_and(|status| status.pre_unsupported > 0 || status.post_unsupported > 0);

    let post_failed = usize::from(crashed && harness.tally().is_some_and(|t| t.post_failed > 0));
    let dispatched = harness.tally().map_or(0, |tally| tally.dispatched);
    let mut execute = crate::gen_dsl::TimingStats::default();
    execute.observe(execute_elapsed);
    let function = selected_function(staged, input);
    if tyche.enabled() {
        use crate::tyche::{Execution, TycheCase, TycheStatus};
        let (status, reason) = TycheStatus::classify(Execution {
            killed: crashed,
            post_failed: post_failed > 0,
            pre_failed: oracle.is_some_and(|status| status.pre_failed > 0),
            parse_rejected: oracle.is_some_and(|status| status.parse_rejected > 0),
            oracle_unsupported,
            dispatched: dispatched > 0,
        });
        tyche.write(
            TycheCase {
                property: function.map_or("unknown function", |function| function.name.as_str()),
                status,
                status_reason: reason,
                input: Some(input),
                backtracks: None,
                owned_bytes: None,
                timing: std::collections::BTreeMap::from([("execute", execute.last_secs())]),
            },
            &cfg.reporter,
        );
    }
    tyche.finish(&cfg.reporter);
    let (filed, kept, retained) = match artifact.filter(|_| !oracle_unsupported) {
        Some(artifact) => {
            let measured = Measured {
                verdict: if crashed {
                    Verdict::Crash
                } else if artifact.evidence().precondition == PreconditionValidation::Failed {
                    Verdict::NotDispatched
                } else {
                    Verdict::Survived
                },
                cover: None,
                post: post_failed > 0,
            };
            let classified = artifact.classify(&measured).ok_or_else(|| {
                DriverError::BadInput(
                    "execution produced an outcome that validation did not retain".to_string(),
                )
            })?;
            let outcome = classified.outcome();
            let crash_finding = outcome.is_crash_finding();
            let path = Store::promote_outcome(
                &ArtifactFolders::new(&cfg.corpus, &cfg.crashes),
                &classified,
                &mut journal,
                Some(0),
                Provenance::Seed,
                Timing::Observed,
            )
            .map_err(CampaignError::from)?;
            let retained =
                RetainedArtifacts::from_iter([RetainedArtifact::new(outcome, path.clone())]);
            if crash_finding {
                (BTreeSet::from([path]), 0, retained)
            } else {
                (BTreeSet::new(), 1, retained)
            }
        }
        None => (BTreeSet::new(), 0, RetainedArtifacts::default()),
    };
    crate::warn_oracle_unsupported(&cfg.reporter, oracle);

    let timing = crate::gen_dsl::Timings::from([("execute", execute)]);
    let functions = function.map(|function| {
        let mut stats = crate::input::FunctionStats {
            inputs: 1,
            dispatched,
            crashes: u64::from(crashed),
            post_failed: post_failed as u64,
            pre_failed: oracle.map_or(0, |status| status.pre_failed),
            parse_rejected: oracle.map_or(0, |status| status.parse_rejected),
            timing: timing.clone(),
            ..crate::input::FunctionStats::new(function.target_id, function.name.clone())
        };
        if let Ok(parts) = input.parts() {
            stats
                .sizes
                .test_case_size
                .insert(parts.test_case_size.get(), 1);
        }
        vec![stats]
    });

    Ok(CampaignReport {
        engine: None,
        corpus_snapshot_interval: resolve_corpus_snapshot_interval(
            cfg.corpus_snapshot_interval,
            Budget::Unbounded,
        ),
        findings: Crashes::observed(usize::from(crashed), filed, post_failed),
        corpus: Some(Corpus {
            artifacts: kept,
            unique: kept,
            dir: cfg.corpus.clone(),
        }),
        retained,
        tally: harness.tally(),
        oracle,
        internal: Some(LoopStats {
            inputs: 1,
            rejected: 0,
            heap: region,
            retention_complete: true,
            rejections: None,
            functions,
            timing,
        }),
        external: None,
    })
}
