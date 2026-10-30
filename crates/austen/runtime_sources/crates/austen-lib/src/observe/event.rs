use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, strum::EnumIter)]
pub enum Phase {
    Runtime,
    Shim,
    CompileTarget,
    CompileHarness,
    Convert,
    Spawn,
    Campaign,
    ValidateSeeds,
    ImportFindings,
    Coverage,
    MinimizeSuite,
    Shrink,
    ReminimizeSuite,
}

impl serde::Serialize for Phase {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.token())
    }
}

impl Phase {
    pub fn token(self) -> &'static str {
        match self {
            Phase::Runtime => "runtime",
            Phase::Shim => "shim",
            Phase::CompileTarget => "compile-target",
            Phase::CompileHarness => "compile-harness",
            Phase::Convert => "convert",
            Phase::Spawn => "spawn",
            Phase::Campaign => "campaign",
            Phase::ValidateSeeds => "validate-seeds",
            Phase::ImportFindings => "import-findings",
            Phase::Coverage => "coverage",
            Phase::MinimizeSuite => "minimize-suite",
            Phase::Shrink => "shrink",
            Phase::ReminimizeSuite => "reminimize-suite",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Phase::Runtime => "locating the runtime",
            Phase::Shim => "generating the shim",
            Phase::CompileTarget => "compiling the target",
            Phase::CompileHarness => "linking the harness",
            Phase::Convert => "converting the specification",
            Phase::Spawn => "spawning the harness",
            Phase::Campaign => "testing",
            Phase::ValidateSeeds => "validating launch seeds",
            Phase::ImportFindings => "importing findings",
            Phase::Coverage => "measuring coverage",
            Phase::MinimizeSuite => "minimizing suites",
            Phase::Shrink => "shrinking findings",
            Phase::ReminimizeSuite => "reminimizing suites",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, strum::EnumIter)]
pub enum SuitePass {
    Initial,
    Final,
}

impl SuitePass {
    pub const fn token(self) -> &'static str {
        match self {
            SuitePass::Initial => "initial",
            SuitePass::Final => "final",
        }
    }
}

impl Serialize for SuitePass {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.token())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunMode {
    Campaign,
    Replay,
    SingleInput,
    Build,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CampaignKind {
    #[default]
    Internal,
    External,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunStarted {
    pub spec: PathBuf,
    pub target: Option<PathBuf>,
    pub harness: Option<PathBuf>,
    pub work: PathBuf,
    pub engine: Option<String>,
    pub mode: RunMode,
    pub budget_secs: Option<u64>,
}

pub const SNAPSHOT_TICK: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct CampaignSnapshot {
    pub kind: CampaignKind,
    pub elapsed: Duration,
    pub budget: Option<Duration>,

    pub inputs: Option<u64>,
    pub rejected: Option<u64>,
    pub rejections_total: Option<u64>,
    pub replay_left: Option<u64>,

    pub seen: Option<u64>,
    pub selected: Option<u64>,
    pub dispatched: Option<u64>,

    pub execs: Option<u64>,
    pub execs_per_sec: Option<f64>,

    pub crashes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InertCoverage {
    pub carries: String,
    pub usable_by: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "warning", rename_all = "kebab-case")]
pub enum Warning {
    StaleRuntime {
        dir: PathBuf,
        installed: String,
        running: String,
    },
    ForeignRuntime {
        dir: PathBuf,
        staged_from: String,
        running_from: String,
        overridden: bool,
    },
    FindingsCapped {
        unique: usize,
        replaying: usize,
    },
    ReplayFailed {
        message: String,
    },
    HeapBaseMoved {
        base: u64,
        default: u64,
    },
    ReportWriteFailed {
        path: PathBuf,
        message: String,
    },
    TycheWriteFailed {
        path: PathBuf,
        message: String,
    },
    UninstrumentedTarget {
        target: PathBuf,
        engine: String,
        inert: Option<InertCoverage>,
    },
    InstrumentationUnknown {
        target: PathBuf,
        message: String,
    },
    StaticFunctionInPrebuiltArchive {
        function: String,
    },
    ExternalConfigurationFallback {
        engine: String,
        from: String,
        to: String,
        message: String,
    },
    LinkLineUnreadable {
        entry: PathBuf,
        message: String,
    },
    HarnessKindUnknown {
        harness: PathBuf,
        message: String,
    },
    ShrinkFailed {
        input: PathBuf,
        message: String,
    },
    ShrinkRejected {
        input: PathBuf,
        verdict: String,
    },
    ShrinkCapped {
        stored: usize,
        shrinking: usize,
    },
    SuiteMinimizationFailed {
        pass: SuitePass,
        outcome: Option<String>,
        message: String,
    },
    CoverageUnavailable {
        message: String,
    },
    FinalizationFailed {
        during: &'static str,
        message: String,
    },
    CoverageTimingDegraded {
        message: String,
    },
    CoverageCapped {
        entries: usize,
        replaying: usize,
    },
    CoverageAliased {
        sites: u64,
        map_bytes: u64,
    },
    CoverageEntriesLost {
        verdicts: BTreeMap<String, usize>,
    },
    CoverageReplayFailed {
        entry: PathBuf,
        message: String,
    },
    CoverageDenominatorMissing {
        harness: PathBuf,
    },
    CoverageEdgesWriteFailed {
        path: PathBuf,
        message: String,
    },
    OracleUnsupported {
        pre_unsupported: u64,
        post_unsupported: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum RunEvent {
    ToolSelected {
        name: String,
        path: PathBuf,
        version: Option<String>,
    },
    BuildCommand {
        directory: PathBuf,
        role: String,
        program: PathBuf,
        arguments: Vec<String>,
        environment: BTreeMap<String, String>,
        working_directory: Option<PathBuf>,
    },
    RunStarted(RunStarted),

    PhaseStarted {
        phase: Phase,
    },
    PhaseFinished {
        phase: Phase,
        elapsed: Duration,
    },

    StageStarted {
        index: usize,
        count: usize,
        engine: String,
        budget_secs: Option<u64>,
        seed: Option<u64>,
    },

    HarnessReused {
        harness: PathBuf,
    },

    CompilerCoverage {
        engine: String,
        controls: Vec<String>,
        flavors: Vec<String>,
    },

    ReplayLoaded {
        path: PathBuf,
        inputs: usize,
        bytes: usize,
    },

    HeapPublished {
        base: u64,
        size: u64,
    },

    Snapshot(CampaignSnapshot),

    CrashObserved {
        index: u64,
        input_bytes: usize,
    },

    ValidationProgress {
        validated: usize,
        total: usize,
    },

    ImportProgress {
        replayed: usize,
        total: usize,
    },

    ShrinkProgress {
        shrunk: usize,
        total: usize,
    },

    SuiteMinimizationProgress {
        pass: SuitePass,
        outcome: String,
        retained: usize,
        total: usize,
    },

    Warning(Warning),

    RunFinished {
        outcome: Outcome,
        elapsed: Duration,
    },
}

impl RunEvent {
    pub fn tool_selected(name: &str, path: PathBuf) -> Self {
        RunEvent::ToolSelected {
            name: name.to_string(),
            path,
            version: None,
        }
    }
}
