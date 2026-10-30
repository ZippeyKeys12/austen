mod token;

use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use token::stable_tokens;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunReport {
    pub schema_version: u32,
    #[serde(default)]
    pub provenance: Option<RunProvenance>,

    pub spec: PathBuf,
    pub target: Option<PathBuf>,
    pub harness: Option<PathBuf>,
    pub input: Option<String>,
    pub function: Option<String>,
    pub work: Option<PathBuf>,

    pub budget: Option<Budget>,
    pub duration: Option<u64>,
    pub seed: Option<u64>,
    pub replay: Option<ReplayReport>,
    pub engine: Option<String>,
    pub function_selector: Option<FunctionSelector>,
    pub check_heap_ownership: bool,
    #[serde(default)]
    pub effective_check_heap_ownership: Option<bool>,

    pub inputs: Option<usize>,
    pub rejected: Option<usize>,
    pub heap_base: Option<u64>,
    pub heap_size: Option<u64>,
    pub heap_base_is_default: Option<bool>,
    pub rejections: Option<BTreeMap<String, u64>>,
    #[serde(default)]
    pub functions: Option<Vec<FunctionReport>>,
    #[serde(default)]
    pub timing: Option<BTreeMap<String, TimingReport>>,

    pub seen: Option<u64>,
    pub selected: Option<u64>,
    pub dispatched: Option<u64>,
    pub oracle: Option<OracleReport>,

    pub crashes: Option<usize>,
    pub findings: Option<Findings>,
    pub corpus: Option<CorpusReport>,
    pub external: Option<ExternalReport>,
    pub suite_minimization: Option<SuiteMinimizationReport>,
    pub shrink: Option<ShrinkReport>,
    pub coverage: Option<CoverageReport>,
    #[serde(default)]
    pub stages: Option<Vec<StageReport>>,

    pub status: Status,
    pub stage: Stage,
    pub error: Option<ErrorReport>,
    pub exit: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunProvenance {
    pub captured_at_unix_ms: u64,
    pub invocation_directory: Option<PathBuf>,
    pub platform: PlatformProvenance,
    #[serde(default)]
    pub environment: Option<BTreeMap<String, String>>,
    pub configuration: CampaignConfiguration,
    pub tools: Vec<ToolProvenance>,
    #[serde(default)]
    pub libraries: BTreeMap<String, String>,
    pub builds: Vec<BuildCommandProvenance>,
    pub reused_harnesses: Vec<PathBuf>,
    pub runtime: Option<RuntimeProvenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformProvenance {
    pub os: String,
    pub architecture: String,
    pub kernel_release: Option<String>,
    pub available_parallelism: Option<usize>,
    pub austen_build_host: String,
    pub austen_build_profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeProvenance {
    pub directory: PathBuf,
    pub version: String,
    pub host: String,
    pub profile: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolProvenance {
    pub role: String,
    pub program: PathBuf,
    pub resolved_path: Option<PathBuf>,
    pub version: Option<String>,
    pub version_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildCommandProvenance {
    pub directory: PathBuf,
    pub role: String,
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub working_directory: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CampaignConfiguration {
    pub mode: String,
    pub engine: Option<String>,
    pub external_mode: Option<ExternalMode>,
    pub function_selector: Option<FunctionSelector>,
    #[serde(default)]
    pub max_generator_size: Option<u64>,
    #[serde(default)]
    pub chain_discard_limit: Option<u64>,
    pub duration_seconds: Option<u64>,
    pub seed: Option<u64>,
    pub replay: Option<PathBuf>,
    pub record: Option<PathBuf>,
    pub work: Option<PathBuf>,
    pub crashes: Option<PathBuf>,
    pub corpus: Option<PathBuf>,
    pub target_kind: String,
    pub coverage: Vec<String>,
    pub cflags: Vec<String>,
    pub corpus_snapshot_interval_ms: Option<u64>,
    pub resolved_snapshot_interval_ms: Option<u64>,
    pub production: ProductionConfiguration,
    pub check_heap_ownership: bool,
    #[serde(default)]
    pub allow_unstable_addresses: bool,
    pub shrink: Option<String>,
    pub resolved_shrinker: Option<String>,
    pub minimize_suite: bool,
    #[serde(default)]
    pub exit_fast: bool,
    #[serde(default)]
    pub strict_duration: bool,
    pub fail_on_crash: bool,
    pub clean: bool,
    pub debug_child: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductionConfiguration {
    pub path_pruning: bool,
    pub smt_pruning: bool,
    pub inline: String,
    pub max_unfolds: Option<usize>,
    pub experimental_product_arg_destruction: bool,
    pub experimental_return_pruning: bool,
    pub experimental_arg_pruning: bool,
    pub sink_args: bool,
    pub max_array_length: usize,
    pub lift_constraints: bool,
    pub each_fusion: bool,
    pub apply_array_max_length: bool,
    #[serde(default)]
    pub specialize_guarded_choices: bool,
    #[serde(default)]
    pub specialize_guarded_choices_elected: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionReport {
    pub id: u8,
    pub name: String,
    pub inputs: u64,
    pub rejected: u64,
    pub rejections: BTreeMap<String, u64>,
    pub dispatched: u64,
    pub crashes: u64,
    pub post_failed: u64,
    pub pre_failed: u64,
    pub parse_rejected: u64,
    pub gave_up: bool,
    pub timing: BTreeMap<String, TimingReport>,
    pub sites: Vec<SiteReport>,
    pub sizes: SizeReport,
    #[serde(default)]
    pub members: Option<Vec<MemberReport>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemberReport {
    pub engine: String,
    pub inputs: u64,
    pub rejected: u64,
    pub rejections: BTreeMap<String, u64>,
    pub retired: bool,
    pub sites: Vec<SiteReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageReport {
    pub index: usize,
    pub engine: String,
    pub budget: Budget,
    pub duration: Option<u64>,
    pub seed: Option<u64>,
    pub harness: PathBuf,
    pub elapsed: f64,
    pub status: Status,
    pub inputs: Option<usize>,
    pub rejected: Option<usize>,
    pub seen: Option<u64>,
    pub selected: Option<u64>,
    pub dispatched: Option<u64>,
    pub crashes: Option<usize>,
    pub findings: Option<Findings>,
    pub corpus: Option<CorpusReport>,
    pub seeds: Option<usize>,
    pub oracle: Option<OracleReport>,
    pub external: Option<ExternalReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TimingReport {
    pub count: u64,
    pub min: f64,
    pub max: f64,
    pub total: f64,
    pub avg: f64,
    pub last: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteReport {
    pub def: String,
    pub ordinal: u64,
    pub failed: u64,
    pub passed: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SizeReport {
    pub owned_bytes: BTreeMap<u64, u64>,
    pub test_case_size: BTreeMap<u64, u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayReport {
    pub path: PathBuf,
    pub bytes: Option<usize>,
}

stable_tokens! {
    pub enum FunctionSelector {
        Random => "random",
        RoundRobin => "round-robin",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OracleReport {
    pub parse_rejected: u64,
    pub pre_passed: u64,
    pub pre_failed: u64,
    pub pre_unsupported: u64,
    pub pre_internal_error: u64,
    pub post_unsupported: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorReport {
    pub kind: ErrorKind,
    pub blame: Blame,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalReport {
    pub mode: Option<ExternalMode>,
    pub duration: Option<u64>,
    #[serde(default)]
    pub snapshot_interval_ms: Option<u64>,
    #[serde(default)]
    pub snapshot_complete: Option<bool>,
    pub instrumented: bool,
    pub instrumentation: Instrumentation,
    pub execs: Option<u64>,
    pub execs_per_sec: Option<f64>,
    pub out: PathBuf,
    pub crashes: CrashesReport,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrashesReport {
    pub artifacts: usize,
    pub unique: usize,
    pub replayed: usize,
    pub confirmed: usize,
    pub verdicts: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CorpusReport {
    pub artifacts: usize,
    pub unique: usize,
    pub out: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuiteMinimizationReport {
    pub tool: String,
    pub harness: PathBuf,
    pub initial: SuiteMinimizationPassReport,
    #[serde(rename = "final")]
    pub final_pass: Option<SuiteMinimizationPassReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuiteMinimizationPassReport {
    pub eligible: usize,
    pub retained: usize,
    pub outcomes: BTreeMap<String, SuiteOutcomeMinimizationReport>,
    pub out: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuiteOutcomeMinimizationReport {
    pub eligible: usize,
    pub retained: usize,
    pub fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShrinkReport {
    pub tool: String,
    pub harness: PathBuf,
    pub stored: usize,
    pub eligible: usize,
    pub attempted: usize,
    pub shrunk: usize,
    pub unchanged: usize,
    pub rejected: usize,
    pub bytes_before: usize,
    pub bytes_after: usize,
    pub capped: bool,
    pub verdicts: BTreeMap<String, u64>,
    pub out: PathBuf,
}

#[derive(Deserialize)]
struct ShrinkReportFields {
    tool: String,
    harness: PathBuf,
    stored: usize,
    #[serde(default)]
    eligible: Option<usize>,
    attempted: usize,
    shrunk: usize,
    unchanged: usize,
    rejected: usize,
    bytes_before: usize,
    bytes_after: usize,
    capped: bool,
    verdicts: BTreeMap<String, u64>,
    out: PathBuf,
}

impl<'de> Deserialize<'de> for ShrinkReport {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let fields = ShrinkReportFields::deserialize(deserializer)?;
        Ok(Self {
            tool: fields.tool,
            harness: fields.harness,
            stored: fields.stored,
            eligible: fields.eligible.unwrap_or(fields.stored),
            attempted: fields.attempted,
            shrunk: fields.shrunk,
            unchanged: fields.unchanged,
            rejected: fields.rejected,
            bytes_before: fields.bytes_before,
            bytes_after: fields.bytes_after,
            capped: fields.capped,
            verdicts: fields.verdicts,
            out: fields.out,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoverageReport {
    pub harness: PathBuf,
    pub harness_digest: String,
    pub instrumentation: Instrumentation,
    pub sites: Option<u64>,
    pub covered: Option<usize>,
    pub collision_free: Option<bool>,
    pub map_bytes: Option<u64>,
    #[serde(default)]
    pub snapshot_interval_ms: Option<u64>,
    pub denominator: CoverageDenominator,
    pub corpus: CoverageCorpus,
    pub verdicts: BTreeMap<String, u64>,
    pub trajectory: Vec<CoveragePoint>,
    pub timing: CoverageTiming,
    pub edges: Option<PathBuf>,
    pub skipped: Option<String>,
}

impl CoverageReport {
    pub fn skipped(why: impl Into<String>) -> CoverageReport {
        CoverageReport {
            harness: PathBuf::new(),
            harness_digest: String::new(),
            instrumentation: Instrumentation::Unknown,
            sites: None,
            covered: None,
            collision_free: None,
            map_bytes: None,
            snapshot_interval_ms: None,
            denominator: CoverageDenominator::InstrumentedSites,
            corpus: CoverageCorpus {
                entries: 0,
                replayed: 0,
                capped: false,
                provenance: None,
            },
            verdicts: BTreeMap::new(),
            trajectory: Vec::new(),
            timing: CoverageTiming::Unknown,
            edges: None,
            skipped: Some(why.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoverageCorpus {
    pub entries: usize,
    pub replayed: usize,
    pub capped: bool,
    pub provenance: Option<CoverageProvenance>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoveragePoint {
    pub elapsed_ms: Option<u64>,
    pub covered: usize,
}

stable_tokens! {
    pub enum CoverageDenominator {
        InstrumentedSites => "instrumented-sites",
    }
}

stable_tokens! {
    pub enum CoverageProvenance {
        Recorded => "recorded",
        EngineRetained => "engine-retained",
    }
}

stable_tokens! {
    pub enum CoverageTiming {
        Observed => "observed",
        Mtime => "mtime",
        Unknown => "unknown",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Findings {
    pub unit: FindingsUnit,
    pub confirmed: usize,
    pub capped: Option<bool>,
    pub post_failed: usize,
}

stable_tokens! {
    pub enum FindingsUnit {
        CrashingExecutions => "crashing-executions",
        DistinctConfirmedInputs => "distinct-confirmed-inputs",
    }
}

stable_tokens! {
    pub enum ExternalMode {
        HonggfuzzAuto => "auto",
        HonggfuzzPersistent => "persistent",
        HonggfuzzStdin => "stdin",
        LibFuzzerDirect => "direct",
        LibFuzzerFork => "fork",
    }
}

stable_tokens! {
    pub enum Instrumentation {
        Compiled => "compiled",
        Detected => "detected",
        Absent => "absent",
        Unknown => "unknown",
    }
}

stable_tokens! {
    pub enum Budget {
        Seconds => "seconds",
        Unbounded => "unbounded",
        Tape => "tape",
        Build => "build",
        None => "none",
    }
}

stable_tokens! {
    pub enum Status {
        Ok => "ok",
        Error => "error",
    }
}

stable_tokens! {
    pub enum Stage {
        Args => "args",
        Runtime => "runtime",
        SpecParse => "spec-parse",
        Shim => "shim",
        CompileTarget => "compile-target",
        CompileHarness => "compile-harness",
        Convert => "convert",
        Spawn => "spawn",
        Generate => "generate",
        Dispatch => "dispatch",
        Complete => "complete",
    }
}

stable_tokens! {
    pub enum Blame {
        Spec => "spec",
        Austen => "austen",
        Environment => "environment",
    }
}

stable_tokens! {
    pub enum ErrorKind {
        Usage => "usage",
        Runtime => "runtime",
        Io => "io",
        BuildIo => "build.io",
        BuildTemplate => "build.template",
        BuildParseSpec => "build.parse-spec",
        BuildShim => "build.shim",
        BuildCc => "build.cc",
        BuildObjcopy => "build.objcopy",
        BuildRustc => "build.rustc",
        BuildForeignInstrumentation => "build.foreign-instrumentation",
        DriverIo => "driver.io",
        DriverNotARecording => "driver.not-a-recording",
        DriverParseSpec => "driver.parse-spec",
        DriverNoFunctions => "driver.no-functions",
        DriverNoSuchFunction => "driver.no-such-function",
        DriverFunctionNotFirst => "driver.function-not-first",
        DriverConvert => "driver.convert",
        DriverNotWellScoped => "driver.not-well-scoped",
        DriverMalformedFunctionTable => "driver.malformed-function-table",
        DriverWireDiverged => "driver.wire-diverged",
        DriverBadInput => "driver.bad-input",
        DriverHostileEnv => "driver.hostile-env",
        DriverHeapTimeout => "driver.heap-timeout",
        DriverHeapMalformed => "driver.heap-malformed",
        DriverUnrunnable => "driver.unrunnable",
        DriverLibAfl => "driver.libafl",
        DriverFindings => "driver.findings",
        ExternalNotFound => "external.not-found",
        ShrinkNotFound => "shrink.not-found",
        ExternalSpawn => "external.spawn",
        ExternalFailed => "external.failed",
        ExternalUnrunnable => "external.unrunnable",
        ExternalIo => "external.io",
    }
}
