use std::num::NonZeroU64;
use std::path::PathBuf;
use std::time::Duration;

use crate::gen_dsl::passes::PassProfile;
use crate::gen_dsl::{Entropy, GenModule, Region, Sym, TestCaseSize};
use crate::observe::{Cancel, Reporter, RunEvent, Warning};

use crate::external::ToolOutput;
use crate::findings::{Corpus, Crashes, FindingsError};
use crate::instrumentation::Instrumentation;
use crate::wire::{OracleStatus, Tally};
use enum_dispatch::enum_dispatch;

use crate::Rejections;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedPath {
    pub role: &'static str,
    pub path: PathBuf,
}

impl ProtectedPath {
    pub fn new(role: &'static str, path: impl Into<PathBuf>) -> Self {
        Self {
            role,
            path: path.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Budget {
    Seconds(u64),
    Unbounded,
    Tape,
    Build,
}

impl Budget {
    pub fn seconds(self) -> Option<u64> {
        match self {
            Budget::Seconds(secs) => Some(secs),
            Budget::Tape | Budget::Unbounded | Budget::Build => None,
        }
    }
}

pub const DEFAULT_CORPUS_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(15 * 60);

const MIN_CORPUS_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(1);
const MAX_CORPUS_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(15 * 60);
const AUTO_CORPUS_SNAPSHOTS: u128 = 100;

pub fn resolve_corpus_snapshot_interval(explicit: Option<Duration>, budget: Budget) -> Duration {
    if let Some(interval) = explicit {
        return interval;
    }

    let Budget::Seconds(seconds) = budget else {
        return DEFAULT_CORPUS_SNAPSHOT_INTERVAL;
    };
    if seconds == 0 {
        return DEFAULT_CORPUS_SNAPSHOT_INTERVAL;
    }

    let duration_ms = u128::from(seconds) * 1_000;
    let interval_ms = duration_ms.div_ceil(AUTO_CORPUS_SNAPSHOTS).clamp(
        MIN_CORPUS_SNAPSHOT_INTERVAL.as_millis(),
        MAX_CORPUS_SNAPSHOT_INTERVAL.as_millis(),
    );
    Duration::from_millis(
        u64::try_from(interval_ms).expect("the clamped snapshot interval fits in u64"),
    )
}

pub struct CampaignFunction {
    pub post_defaults: crate::gen_dsl::DefaultTypes,
    pub target_id: u8,
    pub name: String,
    pub generator: Sym,
}

pub struct CampaignConfig {
    pub functions: Vec<CampaignFunction>,
    pub function_selector: crate::FunctionSelectorKind,
    pub size_selector: crate::SizeSelectorKind,
    pub max_test_case_size: TestCaseSize,
    pub wire_module: GenModule,
    pub production_modules: crate::internal::ProductionModules,
    pub generator_path: PathBuf,
    pub harness: PathBuf,
    pub work: PathBuf,
    pub crashes: PathBuf,
    pub corpus: PathBuf,
    pub journal: PathBuf,
    pub caller_protected_paths: Vec<ProtectedPath>,
    pub validator: std::sync::Arc<dyn crate::findings::ArtifactValidator>,
    pub heap_bytes: u64,
    pub allow_unstable_addresses: bool,
    pub max_array_length: usize,
    pub generator_timeout: Option<std::time::Duration>,
    pub chain_discard_limit: Option<NonZeroU64>,
    pub budget: Budget,
    pub exit_fast: bool,
    pub strict_duration: bool,
    pub corpus_snapshot_interval: Duration,
    pub instrumented: bool,
    pub instrumentation: Instrumentation,
    pub coverage_guards: Option<u64>,
    pub seed: u64,
    pub entropy: Entropy,
    pub record: Option<PathBuf>,
    pub tyche: Option<PathBuf>,
    pub debug_child: bool,
    pub tool_output: ToolOutput,
    pub reporter: Reporter,
    pub cancel: Cancel,
}

impl CampaignConfig {
    pub fn output_paths<'a>(
        &'a self,
        folders: &'a crate::findings::ArtifactFolders,
    ) -> Vec<(&'static str, &'a std::path::Path)> {
        use crate::findings::ArtifactOutcome;

        let mut paths = vec![
            ("harness", self.harness.as_path()),
            ("generator", self.generator_path.as_path()),
            ("crash archive", self.crashes.as_path()),
            ("corpus archive", self.corpus.as_path()),
            (
                "precondition-failing archive",
                folders.path(ArtifactOutcome::PreconditionFailing),
            ),
            ("passing archive", folders.path(ArtifactOutcome::Passing)),
            ("crash findings", folders.path(ArtifactOutcome::Crash)),
            (
                "postcondition-failing findings",
                folders.path(ArtifactOutcome::PostconditionFailing),
            ),
            ("journal", self.journal.as_path()),
        ];
        if let Some(record) = self.record.as_deref() {
            paths.push(("recording", record));
        }
        if let Some(tyche) = self.tyche.as_deref() {
            paths.push(("Tyche output", tyche));
        }
        paths.extend(
            self.caller_protected_paths
                .iter()
                .map(|protected| (protected.role, protected.path.as_path())),
        );
        paths
    }
}

pub(crate) fn validate_functions(functions: &[CampaignFunction]) -> Result<(), CampaignError> {
    if functions.is_empty() {
        return Err(CampaignError::Unrunnable(
            "the campaign has no active functions".to_string(),
        ));
    }
    let mut ids = std::collections::BTreeSet::new();
    for function in functions {
        if !ids.insert(function.target_id) {
            return Err(CampaignError::Unrunnable(format!(
                "function dispatch ID {} is active more than once",
                function.target_id
            )));
        }
    }
    Ok(())
}

pub(crate) fn validate_generator_modules(
    functions: &[CampaignFunction],
    wire: &GenModule,
    production: &crate::internal::ProductionModules,
) -> Result<(), CampaignError> {
    wire.validate()
        .map_err(crate::internal::EngineOpenError::Wire)?;
    if production.is_empty() {
        return Err(CampaignError::Unrunnable(
            "no production module was staged for this campaign".into(),
        ));
    }
    for (_, custom) in production.custom_iter() {
        custom.validate(wire)?;
    }
    for (_, production) in production.iter() {
        production
            .validate()
            .map_err(crate::internal::EngineOpenError::Production)?;
        for function in functions {
            crate::internal::validate_generator_pair(&function.generator, wire, production)?;
        }
        crate::internal::validate_module_fork(wire, production)?;
    }
    Ok(())
}

pub fn warn_oracle_unsupported(reporter: &Reporter, status: Option<OracleStatus>) {
    if let Some(status) = status
        && status.pre_unsupported + status.post_unsupported > 0
    {
        reporter.emit(|| {
            RunEvent::Warning(Warning::OracleUnsupported {
                pre_unsupported: status.pre_unsupported,
                post_unsupported: status.post_unsupported,
            })
        });
    }
}

#[derive(Clone, Debug)]
pub struct CampaignReport {
    pub engine: Option<&'static str>,
    pub corpus_snapshot_interval: Duration,
    pub findings: Crashes,
    pub corpus: Option<Corpus>,
    pub retained: crate::findings::RetainedArtifacts,
    pub tally: Option<Tally>,
    pub oracle: Option<OracleStatus>,
    pub internal: Option<LoopStats>,
    pub external: Option<crate::external::ExternalOutcome>,
}

#[derive(Clone, Debug)]
pub struct LoopStats {
    pub functions: Option<Vec<crate::FunctionStats>>,
    pub timing: crate::gen_dsl::Timings,
    pub inputs: usize,
    pub rejected: usize,
    pub heap: Region,
    pub retention_complete: bool,
    pub rejections: Option<Rejections>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratorPhase {
    Prepare,
    Open,
    Live,
}

impl std::fmt::Display for GeneratorPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Prepare => "preparation",
            Self::Open => "opening",
            Self::Live => "live operation",
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CampaignError {
    #[error("{source}")]
    WithReport {
        #[source]
        source: Box<CampaignError>,
        report: Box<CampaignReport>,
    },
    #[error("could not open generator modules: {0}")]
    EngineOpen(#[from] crate::internal::EngineOpenError),
    #[error("write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("internal retained-input spool {spool} overlaps configured {role} path {path}")]
    SpoolPathConflict {
        spool: PathBuf,
        role: &'static str,
        path: PathBuf,
    },
    #[error(
        "resolve configured path {path} before opening the internal retained-input spool: {source}"
    )]
    SpoolPathResolution {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the generator is structurally unrunnable: {0}")]
    Unrunnable(String),
    #[error("generator {generator} failed during {phase}: {message}")]
    Generator {
        generator: &'static str,
        phase: GeneratorPhase,
        message: String,
    },
    #[error(
        "generator {generator} prepared with {prepared:?} trust but opened with {live:?}{cleanup}"
    )]
    GeneratorTrustMismatch {
        generator: &'static str,
        prepared: crate::wire::Trust,
        live: crate::wire::Trust,
        cleanup: String,
    },
    #[error("the specification oracle failed internally: {0}")]
    OracleInternal(String),
    #[error("the harness did not publish incoming information to {path} within {timeout:?}")]
    HeapTimeout {
        path: PathBuf,
        timeout: std::time::Duration,
    },
    #[error("the harness published invalid incoming information ({reason}): {line:?}")]
    HeapMalformed { line: String, reason: String },
    #[error("{during}: {message}")]
    LibAfl { during: LibAflStep, message: String },
    #[error("{0}")]
    External(#[from] crate::external::ExternalError),
    #[error("{0}")]
    Findings(#[from] FindingsError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibAflStep {
    ShmemProvider,
    CoverageShmem,
    ExportShmId,
    State,
    BuildExecutor,
    RunTarget,
}

impl LibAflStep {
    pub fn is_run(self) -> bool {
        matches!(self, LibAflStep::RunTarget)
    }

    pub fn is_resource(self) -> bool {
        match self {
            LibAflStep::ShmemProvider | LibAflStep::CoverageShmem | LibAflStep::ExportShmId => true,
            LibAflStep::State | LibAflStep::BuildExecutor | LibAflStep::RunTarget => false,
        }
    }
}

impl std::fmt::Display for LibAflStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LibAflStep::ShmemProvider => "shmem provider",
            LibAflStep::CoverageShmem => "coverage shmem",
            LibAflStep::ExportShmId => "export shm id",
            LibAflStep::State => "state",
            LibAflStep::BuildExecutor => "build forkserver executor",
            LibAflStep::RunTarget => "run target",
        })
    }
}

#[enum_dispatch]
pub trait Engine {
    fn name(&self) -> &'static str;

    fn display_name(&self) -> &'static str;

    fn profiles(&self) -> Vec<PassProfile>;

    fn campaign(&self, cfg: CampaignConfig) -> Result<CampaignReport, CampaignError>;
}
