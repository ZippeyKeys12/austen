mod registry;
pub use registry::{AnyDelegatedEngine, AnyFuzzer, DelegatedEngineHandle};

use crate::observe;
pub use crate::tools::discovery::{
    Discovery, NotFound, Tier, discover_tool, is_executable, on_path,
};
use crate::tools::process::{CampaignChild, GRACE, configure_campaign_process_group};
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::instrumentation::{CompileRequest, Coverage, Instrumentation};
use crate::observe::{
    CampaignKind, CampaignSnapshot, Cancel, Reporter, RunEvent, SNAPSHOT_TICK, Warning,
};
use crate::tools::launch_seeds::{self, LaunchSeedError};
use crate::tools::path_safety::{Overlap, PathTraversal};
use crate::wire::{OracleStatus, Tally, TallyReader, Trust};

pub use crate::tools::launch_seeds::{
    CandidateGate, CandidateGateError, SeedCandidate, StagedSeed,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    SharedMemory,
    HfPersistent,
    HfAuto,
    Stdin,
    InProcess,
}

pub const HARNESS_TRANSPORTS: &[Transport] = &[
    Transport::SharedMemory,
    Transport::HfPersistent,
    Transport::HfAuto,
    Transport::Stdin,
    Transport::InProcess,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    pub transport: Transport,
    pub austen_coverage: bool,
    pub blind: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessLink {
    pub linker: Option<PathBuf>,
    pub link_args: Vec<LinkArg>,
    pub driver_symbol: Option<&'static str>,
    pub persistent_hooks: Option<PersistentHookSymbols>,
    pub binary_marker: Option<BinaryMarker>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BinaryMarker {
    pub bytes: &'static [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistentHookSymbols {
    pub clear_new_coverage: &'static str,
    pub reset_local_coverage: &'static str,
    pub reset_stack_depth: &'static str,
    pub count_counters: &'static str,
    pub check_stack_depth: &'static str,
}

impl PersistentHookSymbols {
    pub fn symbols(&self) -> [&'static str; 5] {
        let Self {
            clear_new_coverage,
            reset_local_coverage,
            reset_stack_depth,
            count_counters,
            check_stack_depth,
        } = *self;
        [
            clear_new_coverage,
            reset_local_coverage,
            reset_stack_depth,
            count_counters,
            check_stack_depth,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkArg {
    Path(PathBuf),
    Flag(String),
}

impl LinkArg {
    pub fn path(&self) -> Option<&Path> {
        match self {
            LinkArg::Path(p) => Some(p),
            LinkArg::Flag(_) => None,
        }
    }
}

impl std::fmt::Display for LinkArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkArg::Path(p) => write!(f, "{}", p.display()),
            LinkArg::Flag(flag) => f.write_str(flag),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetCompiler {
    pub program: PathBuf,
    pub request: CompileRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOutput {
    Silence,
    Inherit,
}

pub struct RunRequest<'a> {
    pub harness: &'a Path,
    pub seeds_dir: &'a Path,
    pub out_dir: &'a Path,
    pub duration_secs: Option<u64>,
    pub generator: &'a Path,
    pub heap_bytes: u64,
    pub allow_unstable_addresses: bool,
    pub tally: &'a Path,
    pub oracle_status: &'a Path,
    pub instrumented: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPlan {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub transport_env: Vec<(String, String)>,
    pub mode_fallback: Option<ModeFallback>,
    pub dirs: Vec<PathBuf>,
    pub crashes_dir: PathBuf,
    pub corpus_dir: Option<PathBuf>,
    pub not_findings: &'static [&'static str],
    pub stats: StatsSource,
}

fn launch_env<'a>(
    env: &'a [(String, String)],
    transport_env: &'a [(String, String)],
) -> impl Iterator<Item = &'a (String, String)> {
    env.iter().chain(transport_env)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatsSource {
    ToolFile(PathBuf),
    Captured {
        path: PathBuf,
        exit_codes: &'static [i32],
    },
}

impl StatsSource {
    pub fn path(&self) -> &Path {
        match self {
            StatsSource::ToolFile(path) => path,
            StatsSource::Captured { path, .. } => path,
        }
    }

    fn ran(&self, status: &std::process::ExitStatus) -> bool {
        match self {
            StatsSource::ToolFile(path) => status.success() || path.exists(),
            StatsSource::Captured { exit_codes, .. } => {
                status.code().is_some_and(|code| exit_codes.contains(&code))
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    pub execs: Option<u64>,
    pub execs_per_sec: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumIter)]
pub enum ExternalMode {
    HonggfuzzAuto,
    HonggfuzzPersistent,
    HonggfuzzStdin,
    LibFuzzerDirect,
    LibFuzzerFork,
}

impl ExternalMode {
    pub const fn token(self) -> &'static str {
        match self {
            ExternalMode::HonggfuzzAuto => "auto",
            ExternalMode::HonggfuzzPersistent => "persistent",
            ExternalMode::HonggfuzzStdin => "stdin",
            ExternalMode::LibFuzzerDirect => "direct",
            ExternalMode::LibFuzzerFork => "fork",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeFallback {
    pub marker: PathBuf,
    pub from: ExternalMode,
    pub to: ExternalMode,
    pub message: &'static str,
}

pub trait Fuzzer {
    fn name(&self) -> &'static str;

    fn mode(&self) -> Option<ExternalMode>;

    fn display_name(&self) -> &'static str;

    fn slug(&self) -> &'static str;

    fn capabilities(&self) -> Capabilities;

    fn coverage(&self) -> &'static [Coverage];

    fn rejected_coverage(&self) -> &'static [Coverage];

    fn persistent_hooks(&self) -> Option<PersistentHookSymbols>;

    fn driver_symbol(&self) -> Option<&'static str>;

    fn preflight_harness(&self, _harness: &Path) -> Result<(), ExternalError> {
        Ok(())
    }

    fn compile_request(&self) -> CompileRequest;

    fn target_compiler(&self) -> Result<Option<TargetCompiler>, NotFound>;

    fn run_plan(&self, req: &RunRequest<'_>) -> RunPlan;

    fn parse_stats(&self, text: &str) -> Stats;

    fn discovery(&self) -> Discovery;

    fn discover(&self) -> Result<PathBuf, NotFound> {
        self.discovery().locate()
    }

    fn harness_link(&self) -> Result<Option<HarnessLink>, NotFound> {
        Ok(None)
    }
}

pub trait DelegatedEngine {
    fn name(&self) -> &'static str;

    fn mode(&self) -> Option<ExternalMode>;

    fn display_name(&self) -> &'static str;

    fn slug(&self) -> &'static str;

    fn capabilities(&self) -> Capabilities;

    fn compile_request(&self) -> CompileRequest;

    fn target_compiler(&self) -> Result<Option<TargetCompiler>, NotFound>;

    fn coverage(&self) -> &'static [Coverage];

    fn rejected_coverage(&self) -> &'static [Coverage];

    fn harness_link(&self) -> Result<Option<HarnessLink>, NotFound>;

    fn stage(&self, req: &ExternalRequest<'_>) -> Result<StagedCampaign, ExternalError>;

    fn launch(
        &self,
        staged: &StagedCampaign,
        req: &ExternalRequest<'_>,
    ) -> Result<Child, ExternalError> {
        tracing::debug!(
            target: "austen::external",
            program = %staged.program.display(),
            argv = %staged.plan.argv.join(" "),
            "launching the external tool",
        );

        let mut command = Command::new(&staged.program);
        command.args(&staged.plan.argv);
        crate::wire::remove_mode_vars(&mut command);
        for (key, value) in launch_env(&staged.plan.env, &staged.plan.transport_env) {
            command.env(key, value);
        }
        if req.tool_output == ToolOutput::Silence {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }
        if let StatsSource::Captured { path, .. } = &staged.plan.stats {
            let log = std::fs::File::create(path).map_err(|source| ExternalError::Io {
                path: path.clone(),
                source,
            })?;
            command.stderr(Stdio::from(log));
        }
        configure_campaign_process_group(&mut command);
        command.spawn().map_err(|source| ExternalError::Spawn {
            program: staged.program.display().to_string(),
            source,
        })
    }

    fn parse_stats(&self, text: &str) -> Stats;

    fn campaign(&self, req: &ExternalRequest<'_>) -> Result<ExternalOutcome, ExternalError> {
        let snapshot_interval_invalid = req.corpus_snapshot_interval < Duration::from_secs(1)
            || req.corpus_snapshot_interval.as_millis() > u128::from(u64::MAX)
            || !req
                .corpus_snapshot_interval
                .subsec_nanos()
                .is_multiple_of(1_000_000)
            || Instant::now()
                .checked_add(req.corpus_snapshot_interval)
                .is_none();
        if snapshot_interval_invalid {
            return Err(ExternalError::Unrunnable {
                engine: self.name(),
                display_name: self.display_name(),
                why: "the corpus snapshot interval must be non-zero, at least one second, \
                      schedulable, and fit exactly in the report's millisecond field"
                    .to_string(),
            });
        }
        let staged = self.stage(req)?;

        for dir in &staged.plan.dirs {
            mkdir(dir)?;
        }

        let (artifacts, stats, counts, oracle_status) = if req.build_only {
            (None, Stats::default(), None, None)
        } else {
            let capture = prepare_capture(self, req, &staged);
            let tally = TallyReader::open(staged.tally.clone());
            let child = self.launch(&staged, req)?;
            let (status, captured, capture_error) =
                supervise(self, child, req, &staged, &tally, capture)?;

            if !req.cancel.is_cancelled() && !staged.plan.stats.ran(&status) {
                return Err(ExternalError::Failed {
                    program: staged.program.display().to_string(),
                    status,
                });
            }

            let artifacts = Artifacts {
                crashes_dir: staged.plan.crashes_dir.clone(),
                corpus_dir: staged.plan.corpus_dir.clone(),
                not_findings: staged.plan.not_findings,
                captured,
                capture_error,
            };
            let stats = std::fs::read_to_string(staged.plan.stats.path())
                .ok()
                .map(|text| self.parse_stats(&text))
                .unwrap_or_default();
            (
                Some(artifacts),
                stats,
                tally.read(),
                OracleStatus::read(&staged.oracle_status),
            )
        };

        let StagedCampaign {
            program,
            plan,
            out_dir,
            seeds,
            ..
        } = staged;
        Ok(ExternalOutcome {
            engine: self.name(),
            mode: self.mode(),
            tally: counts,
            oracle_status,
            artifacts,
            execs: stats.execs,
            execs_per_sec: stats.execs_per_sec,
            duration_secs: req.duration_secs,
            instrumented: req.instrumented,
            instrumentation: req.instrumentation,
            seeds,
            out_dir,
            program,
            argv: plan.argv,
            env: plan.env,
            transport_env: plan.transport_env,
        })
    }
}

#[derive(Debug, Clone)]
pub struct StagedCampaign {
    pub program: PathBuf,
    pub plan: RunPlan,
    pub tally: PathBuf,
    pub oracle_status: PathBuf,
    pub out_dir: PathBuf,
    pub seeds: Vec<StagedSeed>,
}

pub struct ExternalRequest<'a> {
    pub harness: &'a Path,
    pub generator: &'a Path,
    pub work: &'a Path,
    pub function_ids: &'a [u8],
    pub seed_sources: &'a [PathBuf],
    pub protected_paths: &'a [(&'static str, &'a Path)],
    pub candidate_gate: &'a dyn CandidateGate,
    pub heap_bytes: u64,
    pub allow_unstable_addresses: bool,
    pub duration_secs: Option<u64>,
    pub corpus_snapshot_interval: Duration,
    pub build_only: bool,
    pub instrumented: bool,
    pub instrumentation: Instrumentation,
    pub tool_output: ToolOutput,
    pub reporter: &'a Reporter,
    pub cancel: &'a Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturedArtifactKind {
    Corpus,
    Crash,
}

impl CapturedArtifactKind {
    fn token(self) -> &'static str {
        match self {
            CapturedArtifactKind::Corpus => "corpus",
            CapturedArtifactKind::Crash => "crash",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedArtifact {
    pub capture_path: PathBuf,
    pub source_path: PathBuf,
    pub elapsed_ms: u64,
    pub sequence: u64,
    pub kind: CapturedArtifactKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifacts {
    pub crashes_dir: PathBuf,
    pub corpus_dir: Option<PathBuf>,
    pub not_findings: &'static [&'static str],
    pub captured: Vec<CapturedArtifact>,
    pub capture_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExternalOutcome {
    pub engine: &'static str,
    pub mode: Option<ExternalMode>,
    pub tally: Option<Tally>,
    pub oracle_status: Option<OracleStatus>,
    pub artifacts: Option<Artifacts>,
    pub execs: Option<u64>,
    pub execs_per_sec: Option<f64>,
    pub duration_secs: Option<u64>,
    pub instrumented: bool,
    pub instrumentation: Instrumentation,
    pub seeds: Vec<StagedSeed>,
    pub out_dir: PathBuf,
    pub program: PathBuf,
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub transport_env: Vec<(String, String)>,
}

impl ExternalOutcome {
    pub fn launch_env(&self) -> impl Iterator<Item = &(String, String)> {
        launch_env(&self.env, &self.transport_env)
    }
}

pub fn engine_and_token(display_name: &str, token: &str) -> String {
    format!("{display_name} (--engine={token})")
}

#[derive(Debug, thiserror::Error)]
pub enum ExternalError {
    #[error("{0}")]
    NotFound(#[from] NotFound),
    #[error("could not run `{program}`: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`{program}` could not start a campaign ({status})")]
    Failed {
        program: String,
        status: std::process::ExitStatus,
    },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("external seed validation failed: {0}")]
    CandidateGate(#[from] CandidateGateError),
    #[error("{display_name} (--engine={engine}) cannot run this target: {why}")]
    Unrunnable {
        engine: &'static str,
        display_name: &'static str,
        why: String,
    },
}

impl From<LaunchSeedError> for ExternalError {
    fn from(error: LaunchSeedError) -> Self {
        match error {
            LaunchSeedError::Io { path, source } => ExternalError::Io { path, source },
            LaunchSeedError::CandidateGate(error) => ExternalError::CandidateGate(error),
        }
    }
}

pub const TRUST: Trust = Trust::Checked;

fn promote_launch_seeds(
    dir: &Path,
    candidates: &[SeedCandidate],
    canonical: &[usize],
    gate: &dyn CandidateGate,
) -> Result<Vec<StagedSeed>, ExternalError> {
    let staged = launch_seeds::write_launch_seeds(dir, candidates, canonical)?;
    if let Err(error) = gate.promoted(&staged) {
        let _ = std::fs::remove_dir_all(dir);
        return Err(error.into());
    }
    Ok(staged)
}

impl<T: Fuzzer> DelegatedEngine for T {
    fn name(&self) -> &'static str {
        Fuzzer::name(self)
    }

    fn mode(&self) -> Option<ExternalMode> {
        Fuzzer::mode(self)
    }

    fn display_name(&self) -> &'static str {
        Fuzzer::display_name(self)
    }

    fn slug(&self) -> &'static str {
        Fuzzer::slug(self)
    }

    fn capabilities(&self) -> Capabilities {
        Fuzzer::capabilities(self)
    }

    fn compile_request(&self) -> CompileRequest {
        Fuzzer::compile_request(self)
    }

    fn target_compiler(&self) -> Result<Option<TargetCompiler>, NotFound> {
        Fuzzer::target_compiler(self)
    }

    fn coverage(&self) -> &'static [Coverage] {
        Fuzzer::coverage(self)
    }

    fn rejected_coverage(&self) -> &'static [Coverage] {
        Fuzzer::rejected_coverage(self)
    }

    fn harness_link(&self) -> Result<Option<HarnessLink>, NotFound> {
        Fuzzer::harness_link(self)
    }

    fn parse_stats(&self, text: &str) -> Stats {
        Fuzzer::parse_stats(self, text)
    }

    fn stage(&self, req: &ExternalRequest<'_>) -> Result<StagedCampaign, ExternalError> {
        let unique_ids: std::collections::BTreeSet<u8> = req.function_ids.iter().copied().collect();
        if unique_ids.is_empty() {
            return Err(ExternalError::Unrunnable {
                engine: Fuzzer::name(self),
                display_name: Fuzzer::display_name(self),
                why: "the campaign has no active functions".to_string(),
            });
        }
        if unique_ids.len() != req.function_ids.len() {
            return Err(ExternalError::Unrunnable {
                engine: Fuzzer::name(self),
                display_name: Fuzzer::display_name(self),
                why: "an original function dispatch ID is active more than once".to_string(),
            });
        }

        Fuzzer::preflight_harness(self, req.harness)?;
        let discovered = self.discover()?;
        req.reporter
            .emit(|| observe::RunEvent::tool_selected(Fuzzer::name(self), discovered.clone()));

        if !req.instrumented && !Fuzzer::capabilities(self).blind {
            return Err(ExternalError::Unrunnable {
                engine: Fuzzer::name(self),
                display_name: Fuzzer::display_name(self),
                why: "it steers by coverage counters the target does not carry — \
                      give it a `.c` source it can instrument, or a prebuilt \
                      object already carrying this engine's own instrumentation"
                    .to_string(),
            });
        }

        let out = req.work.join(format!("{}-out", Fuzzer::slug(self)));
        let launch_seeds = req
            .work
            .join(format!("{}-launch-seeds", Fuzzer::slug(self)));
        let tally = req.work.join("tally");
        let oracle_status_path = req.work.join("oracle-status");

        let managed: [(&'static str, &Path, bool); 4] = [
            ("launch-seed directory", launch_seeds.as_path(), true),
            ("tool output tree", out.as_path(), !req.build_only),
            ("tally", tally.as_path(), true),
            ("oracle status", oracle_status_path.as_path(), true),
        ];
        for (i, &(role, dir, validate)) in managed.iter().enumerate() {
            if !validate {
                continue;
            }
            let others: Vec<(&'static str, &Path)> = managed
                .iter()
                .enumerate()
                .filter(|&(j, _)| j != i)
                .map(|(_, &(other_role, other_dir, _))| (other_role, other_dir))
                .chain(std::iter::once(("tool program", discovered.as_path())))
                .collect();
            validate_managed_path(self, role, dir, req, &others)?;
        }

        launch_seeds::clear_managed_dir(&launch_seeds)?;
        let seed_candidates =
            launch_seeds::collect_seed_candidates(req.work, req.function_ids, req.seed_sources)?;
        let canonical = req.candidate_gate.validate(&seed_candidates)?;
        let staged_seeds = promote_launch_seeds(
            &launch_seeds,
            &seed_candidates,
            &canonical,
            req.candidate_gate,
        )?;
        if !req.build_only {
            launch_seeds::clear_managed_dir(&out)?;
        }
        Tally::stage(&tally).map_err(|source| ExternalError::Io {
            path: tally.clone(),
            source,
        })?;
        OracleStatus::stage(&oracle_status_path).map_err(|source| ExternalError::Io {
            path: oracle_status_path.clone(),
            source,
        })?;

        let plan = self.run_plan(&RunRequest {
            harness: req.harness,
            seeds_dir: &launch_seeds,
            out_dir: &out,
            duration_secs: req.duration_secs,
            generator: req.generator,
            heap_bytes: req.heap_bytes,
            allow_unstable_addresses: req.allow_unstable_addresses,
            tally: &tally,
            oracle_status: &oracle_status_path,
            instrumented: req.instrumented,
        });

        if let Some(fallback) = &plan.mode_fallback {
            validate_managed_path(self, "mode-fallback marker", &fallback.marker, req, &[])?;
            if let Err(source) = std::fs::remove_file(&fallback.marker)
                && source.kind() != std::io::ErrorKind::NotFound
            {
                return Err(ExternalError::Io {
                    path: fallback.marker.clone(),
                    source,
                });
            }
        }

        let program = match Fuzzer::capabilities(self).transport {
            Transport::InProcess => req.harness.to_path_buf(),
            Transport::SharedMemory
            | Transport::HfPersistent
            | Transport::HfAuto
            | Transport::Stdin => discovered,
        };

        Ok(StagedCampaign {
            program,
            plan,
            tally,
            oracle_status: oracle_status_path,
            out_dir: out,
            seeds: staged_seeds,
        })
    }
}

const MAX_CAPTURE_BYTES: usize = crate::wire::EXTERNAL_INPUT_BYTES;

const CAPTURE_READ_CHUNK: usize = 16 * 1024;

struct FileTail {
    offset: u64,
    text: String,
}

impl FileTail {
    fn new() -> Self {
        FileTail {
            offset: 0,
            text: String::new(),
        }
    }

    fn refresh(&mut self, path: &Path) -> &str {
        let _ = self.read_appended(path);
        &self.text
    }

    fn read_appended(&mut self, path: &Path) -> std::io::Result<()> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        if len < self.offset {
            self.offset = 0;
            self.text.clear();
        }
        if len == self.offset {
            return Ok(());
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut appended = String::new();
        let read = file.take(len - self.offset).read_to_string(&mut appended)?;
        self.offset += read as u64;
        self.text.push_str(&appended);
        Ok(())
    }
}

fn report_mode_fallback(
    engine: &(impl DelegatedEngine + ?Sized),
    req: &ExternalRequest<'_>,
    staged: &StagedCampaign,
    reported: &mut bool,
) -> Result<(), ExternalError> {
    if *reported || req.reporter.is_silent() {
        return Ok(());
    }
    let Some(fallback) = &staged.plan.mode_fallback else {
        return Ok(());
    };
    let metadata = match std::fs::symlink_metadata(&fallback.marker) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(ExternalError::Io {
                path: fallback.marker.clone(),
                source,
            });
        }
    };
    if !metadata.file_type().is_file() {
        return Err(ExternalError::Io {
            path: fallback.marker.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the mode-fallback marker is not a regular file",
            ),
        });
    }

    *reported = true;
    req.reporter.emit(|| {
        RunEvent::Warning(Warning::ExternalConfigurationFallback {
            engine: engine.name().to_string(),
            from: fallback.from.token().to_string(),
            to: fallback.to.token().to_string(),
            message: fallback.message.to_string(),
        })
    });
    Ok(())
}

const CAPTURE_MANIFEST: &str = "manifest.tsv";
const CAPTURE_INDEX_WIDTH: usize = 20;

struct CaptureSpool {
    dir: PathBuf,
    manifest: std::fs::File,
    captured: Vec<CapturedArtifact>,
    seen_by_source: HashMap<PathBuf, Vec<CapturedVersion>>,
    next_sequence: u64,
}

struct CapturedVersion {
    len: u64,
    fingerprint: u64,
    capture_path: PathBuf,
}

fn capture_fingerprint(bytes: &[u8]) -> u64 {
    use std::hash::{DefaultHasher, Hasher};
    let mut hasher = DefaultHasher::new();
    hasher.write(bytes);
    hasher.finish()
}

#[derive(Debug)]
struct CaptureLayout {
    dir: PathBuf,
}

struct CaptureChannel {
    spool: Option<CaptureSpool>,
    captured: Vec<CapturedArtifact>,
    error: Option<String>,
}

impl CaptureChannel {
    fn snapshot(&mut self, plan: &RunPlan, elapsed: Duration, final_scan: bool, cancel: &Cancel) {
        let Some(spool) = self.spool.as_mut() else {
            return;
        };
        let result = if final_scan {
            spool.snapshot_final(plan, elapsed)
        } else {
            spool.snapshot(plan, elapsed, cancel)
        };
        if let Err(error) = result {
            let interrupted_for_cancel = !final_scan
                && cancel.is_cancelled()
                && matches!(
                    &error,
                    ExternalError::Io { source, .. }
                        if source.kind() == std::io::ErrorKind::Interrupted
                );
            if self.error.is_none() {
                self.error = Some(error.to_string());
            }
            if interrupted_for_cancel {
                return;
            }
            self.finish();
        }
    }

    fn finish(&mut self) {
        if let Some(spool) = self.spool.take() {
            self.captured.extend(spool.into_captured());
        }
    }

    fn next_deadline(
        &mut self,
        previous: Instant,
        observed_at: Instant,
        interval: Duration,
    ) -> Option<Instant> {
        self.spool.as_ref()?;
        match anchored_deadline_after(previous, observed_at, interval) {
            Some(deadline) => Some(deadline),
            None => {
                self.error = Some(
                    "the next corpus snapshot deadline overflowed the monotonic clock".to_string(),
                );
                self.finish();
                None
            }
        }
    }

    fn into_parts(mut self) -> (Vec<CapturedArtifact>, Option<String>) {
        self.finish();
        (self.captured, self.error)
    }
}

impl CaptureSpool {
    fn open(layout: CaptureLayout) -> Result<CaptureSpool, ExternalError> {
        let dir = layout.dir;
        launch_seeds::clear_managed_dir(&dir)?;
        std::fs::create_dir(&dir).map_err(|source| ExternalError::Io {
            path: dir.clone(),
            source,
        })?;
        let manifest_path = dir.join(CAPTURE_MANIFEST);
        let mut manifest = std::fs::OpenOptions::new()
            .append(true)
            .create_new(true)
            .open(&manifest_path)
            .map_err(|source| ExternalError::Io {
                path: manifest_path.clone(),
                source,
            })?;
        manifest
            .write_all(b"sequence\telapsed_ms\tkind\tcapture\tsource\n")
            .map_err(|source| ExternalError::Io {
                path: manifest_path,
                source,
            })?;
        Ok(CaptureSpool {
            dir,
            manifest,
            captured: Vec::new(),
            seen_by_source: HashMap::new(),
            next_sequence: 0,
        })
    }

    fn snapshot(
        &mut self,
        plan: &RunPlan,
        elapsed: Duration,
        cancel: &Cancel,
    ) -> Result<(), ExternalError> {
        self.snapshot_with_policy(plan, elapsed, false, cancel)
    }

    fn snapshot_final(&mut self, plan: &RunPlan, elapsed: Duration) -> Result<(), ExternalError> {
        self.snapshot_with_policy(plan, elapsed, true, &Cancel::never())
    }

    fn snapshot_with_policy(
        &mut self,
        plan: &RunPlan,
        elapsed: Duration,
        final_scan: bool,
        cancel: &Cancel,
    ) -> Result<(), ExternalError> {
        let mut seen_paths = std::collections::HashSet::new();
        if let Some(corpus) = plan.corpus_dir.as_deref() {
            for source in capture_candidate_paths(corpus, &[], final_scan, cancel)? {
                if seen_paths.insert(source.clone()) {
                    self.consider(
                        source,
                        CapturedArtifactKind::Corpus,
                        elapsed,
                        final_scan,
                        cancel,
                    )?;
                }
            }
        }
        for source in
            capture_candidate_paths(&plan.crashes_dir, plan.not_findings, final_scan, cancel)?
        {
            if seen_paths.insert(source.clone()) {
                self.consider(
                    source,
                    CapturedArtifactKind::Crash,
                    elapsed,
                    final_scan,
                    cancel,
                )?;
            }
        }
        Ok(())
    }

    fn consider(
        &mut self,
        source_path: PathBuf,
        kind: CapturedArtifactKind,
        elapsed: Duration,
        final_scan: bool,
        cancel: &Cancel,
    ) -> Result<(), ExternalError> {
        let Some(bytes) = read_capture_source(&source_path, final_scan, cancel)? else {
            return Ok(());
        };
        let fingerprint = capture_fingerprint(&bytes);
        if let Some(previous) = self.seen_by_source.get(&source_path) {
            for version in previous {
                if version.len != bytes.len() as u64 || version.fingerprint != fingerprint {
                    continue;
                }
                capture_not_cancelled(&source_path, cancel)?;
                if capture_holds(&version.capture_path, &bytes, cancel)? {
                    return Ok(());
                }
            }
        }

        let sequence = self.next_sequence;
        let elapsed_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64;
        let capture_path = self
            .dir
            .join(format!("capture-{sequence:0CAPTURE_INDEX_WIDTH$}.input"));
        let write_result = (|| -> std::io::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&capture_path)?;
            file.write_all(&bytes)?;
            file.flush()
        })();
        if let Err(source) = write_result {
            let cleanup = std::fs::remove_file(&capture_path);
            let source = match cleanup {
                Ok(()) => source,
                Err(cleanup) if cleanup.kind() == std::io::ErrorKind::NotFound => source,
                Err(cleanup) => std::io::Error::new(
                    source.kind(),
                    format!(
                        "{source}; could not remove possibly partial capture {}: {cleanup}",
                        capture_path.display()
                    ),
                ),
            };
            return Err(ExternalError::Io {
                path: capture_path,
                source,
            });
        }

        let manifest_path = self.dir.join(CAPTURE_MANIFEST);
        let manifest_row = format!(
            "{sequence}\t{elapsed_ms}\t{}\t{}\t{:?}\n",
            kind.token(),
            capture_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            source_path,
        );
        if let Err(source) = self
            .manifest
            .write_all(manifest_row.as_bytes())
            .and_then(|()| self.manifest.flush())
        {
            let cleanup = std::fs::remove_file(&capture_path);
            let source = match cleanup {
                Ok(()) => source,
                Err(cleanup) => std::io::Error::new(
                    source.kind(),
                    format!(
                        "{source}; could not remove unmanifested capture {}: {cleanup}",
                        capture_path.display()
                    ),
                ),
            };
            return Err(ExternalError::Io {
                path: manifest_path,
                source,
            });
        }

        self.seen_by_source
            .entry(source_path.clone())
            .or_default()
            .push(CapturedVersion {
                len: bytes.len() as u64,
                fingerprint,
                capture_path: capture_path.clone(),
            });
        self.captured.push(CapturedArtifact {
            capture_path,
            source_path,
            elapsed_ms,
            sequence,
            kind,
        });
        self.next_sequence =
            self.next_sequence
                .checked_add(1)
                .ok_or_else(|| ExternalError::Io {
                    path: self.dir.clone(),
                    source: std::io::Error::other("the capture sequence overflowed u64"),
                })?;
        Ok(())
    }

    fn into_captured(self) -> Vec<CapturedArtifact> {
        self.captured
    }
}

fn capture_candidate_paths(
    root: &Path,
    skip: &[&str],
    final_scan: bool,
    cancel: &Cancel,
) -> Result<Vec<PathBuf>, ExternalError> {
    capture_not_cancelled(root, cancel)?;
    let root_metadata = match std::fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(source) if !final_scan && source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(source) => {
            return Err(ExternalError::Io {
                path: root.to_path_buf(),
                source,
            });
        }
    };
    if !root_metadata.file_type().is_dir() {
        return Err(ExternalError::Io {
            path: root.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the external artifact root is not a directory",
            ),
        });
    }

    let mut files = Vec::new();
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(source) if !final_scan && source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(files);
        }
        Err(source) => {
            return Err(ExternalError::Io {
                path: root.to_path_buf(),
                source,
            });
        }
    };
    for entry in entries {
        capture_not_cancelled(root, cancel)?;
        let entry = match entry {
            Ok(entry) => entry,
            Err(source) if !final_scan && source.kind() == std::io::ErrorKind::NotFound => {
                continue;
            }
            Err(source) => {
                return Err(ExternalError::Io {
                    path: root.to_path_buf(),
                    source,
                });
            }
        };
        if skip.iter().any(|name| entry.file_name() == *name) {
            continue;
        }
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(source) if !final_scan && source.kind() == std::io::ErrorKind::NotFound => {
                continue;
            }
            Err(source) => {
                return Err(ExternalError::Io { path, source });
            }
        };
        if file_type.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn capture_not_cancelled(path: &Path, cancel: &Cancel) -> Result<(), ExternalError> {
    if cancel.is_cancelled() {
        Err(ExternalError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "corpus snapshot interrupted by campaign cancellation; capture is degraded and final-directory fallback is required",
            ),
        })
    } else {
        Ok(())
    }
}

fn capture_too_large(path: &Path, bytes: u64) -> ExternalError {
    ExternalError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "capture candidate is {bytes} bytes, above the supported {MAX_CAPTURE_BYTES}-byte testcase ceiling; capture is degraded and final-directory fallback is required"
            ),
        ),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum CaptureRead {
    Absent,
    Stable(Vec<u8>),
    Unstable,
}

fn read_capture_source_once(path: &Path, cancel: &Cancel) -> Result<CaptureRead, ExternalError> {
    capture_not_cancelled(path, cancel)?;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CaptureRead::Absent);
        }
        Err(source) => {
            return Err(ExternalError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if !metadata.file_type().is_file() {
        return Ok(CaptureRead::Absent);
    }
    if metadata.len() > MAX_CAPTURE_BYTES as u64 {
        return Err(capture_too_large(path, metadata.len()));
    }

    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CaptureRead::Absent);
        }
        #[cfg(unix)]
        Err(source) if source.raw_os_error() == Some(libc::ELOOP) => {
            return Ok(CaptureRead::Absent);
        }
        Err(source) => {
            return Err(ExternalError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let before = file.metadata().map_err(|source| ExternalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !before.file_type().is_file() {
        return Ok(CaptureRead::Absent);
    }
    if before.len() > MAX_CAPTURE_BYTES as u64 {
        return Err(capture_too_large(path, before.len()));
    }
    let before_modified = before.modified().map_err(|source| ExternalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut bytes = Vec::with_capacity(before.len() as usize);
    let mut chunk = [0u8; CAPTURE_READ_CHUNK];
    loop {
        capture_not_cancelled(path, cancel)?;
        let remaining = MAX_CAPTURE_BYTES + 1 - bytes.len();
        let read_len = remaining.min(chunk.len());
        let n = file
            .read(&mut chunk[..read_len])
            .map_err(|source| ExternalError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if bytes.len() > MAX_CAPTURE_BYTES {
            return Err(capture_too_large(path, bytes.len() as u64));
        }
    }
    let after = file.metadata().map_err(|source| ExternalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let after_modified = after.modified().map_err(|source| ExternalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if before.len() != after.len()
        || after.len() != bytes.len() as u64
        || before_modified != after_modified
    {
        return Ok(CaptureRead::Unstable);
    }
    Ok(CaptureRead::Stable(bytes))
}

fn read_capture_source_with(
    path: &Path,
    final_scan: bool,
    mut read: impl FnMut(&Path) -> Result<CaptureRead, ExternalError>,
) -> Result<Option<Vec<u8>>, ExternalError> {
    let vanished = || ExternalError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "source disappeared or ceased to be a regular file during the mandatory final \
             corpus snapshot",
        ),
    };
    match read(path)? {
        CaptureRead::Stable(bytes) => Ok(Some(bytes)),
        CaptureRead::Absent | CaptureRead::Unstable if !final_scan => Ok(None),
        CaptureRead::Absent => Err(vanished()),
        CaptureRead::Unstable => match read(path)? {
            CaptureRead::Stable(bytes) => Ok(Some(bytes)),
            CaptureRead::Absent => Err(vanished()),
            CaptureRead::Unstable => Err(ExternalError::Io {
                path: path.to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "source remained unstable during the mandatory final corpus snapshot",
                ),
            }),
        },
    }
}

fn read_capture_source(
    path: &Path,
    final_scan: bool,
    cancel: &Cancel,
) -> Result<Option<Vec<u8>>, ExternalError> {
    read_capture_source_with(path, final_scan, |path| {
        read_capture_source_once(path, cancel)
    })
}

fn capture_holds(path: &Path, bytes: &[u8], cancel: &Cancel) -> Result<bool, ExternalError> {
    capture_not_cancelled(path, cancel)?;
    let mut file = std::fs::File::open(path).map_err(|source| ExternalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let metadata = file.metadata().map_err(|source| ExternalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() != bytes.len() as u64 {
        return Ok(false);
    }
    let mut offset = 0usize;
    let mut chunk = [0u8; 8192];
    loop {
        capture_not_cancelled(path, cancel)?;
        let read = file.read(&mut chunk).map_err(|source| ExternalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if read == 0 {
            return Ok(offset == bytes.len());
        }
        if bytes.get(offset..offset + read) != Some(&chunk[..read]) {
            return Ok(false);
        }
        offset += read;
    }
}

fn capture_component(slug: &str) -> String {
    format!("{slug}-capture")
}

fn path_traversal(path: &Path) -> Result<PathTraversal, ExternalError> {
    PathTraversal::inspect(path).map_err(|source| ExternalError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn request_protected<'a>(
    req: &'a ExternalRequest<'_>,
) -> impl Iterator<Item = (&'static str, Cow<'a, Path>)> {
    req.protected_paths
        .iter()
        .map(|&(role, path)| (role, Cow::Borrowed(path)))
        .chain([
            ("harness", Cow::Borrowed(req.harness)),
            ("generator", Cow::Borrowed(req.generator)),
        ])
        .chain(launch_seeds::seed_input_paths(req.work, req.seed_sources))
}

fn validate_managed_path(
    engine: &(impl DelegatedEngine + ?Sized),
    managed_role: &'static str,
    dir: &Path,
    req: &ExternalRequest<'_>,
    additional: &[(&'static str, &Path)],
) -> Result<(), ExternalError> {
    let managed = path_traversal(dir)?;
    let protected = request_protected(req).chain(
        additional
            .iter()
            .map(|&(role, path)| (role, Cow::Borrowed(path))),
    );
    managed
        .disjoint_from(protected)
        .map_err(|overlap| match overlap {
            Overlap::Unreadable { path, source } => ExternalError::Io { path, source },
            Overlap::Conflict { role, path } => ExternalError::Unrunnable {
                engine: engine.name(),
                display_name: engine.display_name(),
                why: format!(
                    "the managed {managed_role} {} overlaps the configured {role} {}",
                    dir.display(),
                    path.display(),
                ),
            },
        })
}

fn capture_layout(
    engine: &(impl DelegatedEngine + ?Sized),
    req: &ExternalRequest<'_>,
    staged: &StagedCampaign,
) -> Result<CaptureLayout, ExternalError> {
    let dir = req.work.join(capture_component(engine.slug()));
    let capture = path_traversal(&dir)?;

    let conflict = |role: &'static str, path: &Path| -> Result<(), ExternalError> {
        let protected = path_traversal(path)?;
        if protected.crosses(&capture) {
            return Err(ExternalError::Unrunnable {
                engine: engine.name(),
                display_name: engine.display_name(),
                why: format!(
                    "the managed capture directory {} would recursively clear the configured \
                     {role} {}",
                    dir.display(),
                    path.display(),
                ),
            });
        }
        if capture.crosses(&protected) {
            return Err(ExternalError::Unrunnable {
                engine: engine.name(),
                display_name: engine.display_name(),
                why: format!(
                    "the managed capture directory {} overlaps the configured {role} {}",
                    dir.display(),
                    path.display(),
                ),
            });
        }
        Ok(())
    };

    for (role, path) in request_protected(req) {
        conflict(role, &path)?;
    }
    conflict("tool program", &staged.program)?;
    conflict("tool output tree", &staged.out_dir)?;
    conflict("tally", &staged.tally)?;
    conflict("oracle status", &staged.oracle_status)?;
    let mut seed_parents: Vec<&Path> = Vec::new();
    for seed in &staged.seeds {
        conflict("staged launch seed", &seed.path)?;
        if let Some(parent) = seed.path.parent()
            && !seed_parents.contains(&parent)
        {
            seed_parents.push(parent);
            conflict("staged launch-seed directory", parent)?;
        }
    }
    for path in &staged.plan.dirs {
        conflict("tool output directory", path)?;
    }
    conflict("tool crash directory", &staged.plan.crashes_dir)?;
    if let Some(path) = staged.plan.corpus_dir.as_deref() {
        conflict("tool corpus directory", path)?;
    }
    conflict("tool statistics", staged.plan.stats.path())?;
    if let Some(fallback) = &staged.plan.mode_fallback {
        conflict("mode-fallback marker", &fallback.marker)?;
    }
    Ok(CaptureLayout { dir })
}

fn prepare_capture(
    engine: &(impl DelegatedEngine + ?Sized),
    req: &ExternalRequest<'_>,
    staged: &StagedCampaign,
) -> CaptureChannel {
    let (spool, error) = match capture_layout(engine, req, staged).and_then(CaptureSpool::open) {
        Ok(spool) => (Some(spool), None),
        Err(error) => (None, Some(error.to_string())),
    };
    CaptureChannel {
        spool,
        captured: Vec::new(),
        error,
    }
}

fn campaign_process_error(staged: &StagedCampaign, source: std::io::Error) -> ExternalError {
    ExternalError::Spawn {
        program: staged.program.display().to_string(),
        source,
    }
}

fn stop_after_supervisor_error(
    child: &mut CampaignChild,
    staged: &StagedCampaign,
) -> Result<(), ExternalError> {
    child
        .stop_and_quiesce()
        .map_err(|source| campaign_process_error(staged, source))
}

fn anchored_deadline_after(
    previous: Instant,
    observed_at: Instant,
    interval: Duration,
) -> Option<Instant> {
    let interval_nanos = interval.as_nanos();
    let elapsed_nanos = observed_at
        .checked_duration_since(previous)
        .unwrap_or_default()
        .as_nanos();
    let steps = elapsed_nanos
        .checked_div(interval_nanos)
        .and_then(|whole| whole.checked_add(1));
    let advance = steps
        .and_then(|steps| interval_nanos.checked_mul(steps))
        .and_then(|nanos| {
            let secs = u64::try_from(nanos / 1_000_000_000).ok()?;
            let subsec_nanos = u32::try_from(nanos % 1_000_000_000).ok()?;
            Some(Duration::new(secs, subsec_nanos))
        })?;
    previous.checked_add(advance)
}

fn supervisor_bail(
    child: &mut CampaignChild,
    staged: &StagedCampaign,
    capture: &mut CaptureChannel,
    elapsed: Duration,
    cancel: &Cancel,
    error: ExternalError,
) -> ExternalError {
    if let Err(stop) = stop_after_supervisor_error(child, staged) {
        return stop;
    }
    capture.snapshot(&staged.plan, elapsed, true, cancel);
    error
}

fn supervise(
    engine: &(impl DelegatedEngine + ?Sized),
    child: Child,
    req: &ExternalRequest<'_>,
    staged: &StagedCampaign,
    tally: &TallyReader,
    mut capture: CaptureChannel,
) -> Result<(ExitStatus, Vec<CapturedArtifact>, Option<String>), ExternalError> {
    let mut child =
        CampaignChild::new(child).map_err(|source| campaign_process_error(staged, source))?;
    let watched = !req.reporter.is_silent();
    let mut fallback_reported = false;
    let mut stats_tail = FileTail::new();
    let started = Instant::now();
    capture.snapshot(&staged.plan, Duration::ZERO, false, req.cancel);
    let mut next_capture =
        capture.next_deadline(started, Instant::now(), req.corpus_snapshot_interval);
    let mut grace_ends: Option<Instant> = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                child
                    .quiesce_after_exit()
                    .map_err(|source| campaign_process_error(staged, source))?;
                capture.snapshot(&staged.plan, started.elapsed(), true, req.cancel);
                report_mode_fallback(engine, req, staged, &mut fallback_reported)?;
                let (captured, capture_error) = capture.into_parts();
                return Ok((status, captured, capture_error));
            }
            Ok(None) => {}
            Err(source) => {
                return Err(supervisor_bail(
                    &mut child,
                    staged,
                    &mut capture,
                    started.elapsed(),
                    req.cancel,
                    campaign_process_error(staged, source),
                ));
            }
        }

        if let Err(error) = report_mode_fallback(engine, req, staged, &mut fallback_reported) {
            return Err(supervisor_bail(
                &mut child,
                staged,
                &mut capture,
                started.elapsed(),
                req.cancel,
                error,
            ));
        }

        let now = Instant::now();
        if req.cancel.is_cancelled() {
            match grace_ends {
                None => {
                    if let Err(source) = child.interrupt() {
                        return Err(supervisor_bail(
                            &mut child,
                            staged,
                            &mut capture,
                            started.elapsed(),
                            req.cancel,
                            campaign_process_error(staged, source),
                        ));
                    }
                    grace_ends = Some(now + GRACE);
                }
                Some(ends) if now >= ends => {
                    if let Err(source) = child.force_terminate() {
                        return Err(supervisor_bail(
                            &mut child,
                            staged,
                            &mut capture,
                            started.elapsed(),
                            req.cancel,
                            campaign_process_error(staged, source),
                        ));
                    }
                }
                Some(_) => {}
            }
        }

        if next_capture.is_some_and(|deadline| now >= deadline) {
            capture.snapshot(&staged.plan, now.duration_since(started), false, req.cancel);
            let scan_finished = Instant::now();
            next_capture = next_capture.and_then(|deadline| {
                capture.next_deadline(deadline, scan_finished, req.corpus_snapshot_interval)
            });
        }

        if watched {
            let stats = engine.parse_stats(stats_tail.refresh(staged.plan.stats.path()));
            let counts = tally.read();
            let elapsed = now.duration_since(started);
            req.reporter.emit(|| {
                RunEvent::Snapshot(CampaignSnapshot {
                    kind: CampaignKind::External,
                    elapsed,
                    budget: req.duration_secs.map(Duration::from_secs),
                    inputs: None,
                    rejected: None,
                    rejections_total: None,
                    replay_left: None,
                    seen: counts.map(|t| t.seen),
                    selected: counts.map(|t| t.selected),
                    dispatched: counts.map(|t| t.dispatched),
                    execs: stats.execs,
                    execs_per_sec: stats.execs_per_sec,
                    crashes: 0,
                })
            });
        }
        let sleep_for = next_capture
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .map_or(SNAPSHOT_TICK, |until_capture| {
                SNAPSHOT_TICK.min(until_capture)
            });
        std::thread::sleep(sleep_for);
    }
}

pub fn parse_csv_stats(text: &str, execs: &str, execs_per_sec: &str) -> Stats {
    let mut header = "";
    let mut last: Option<&str> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match line.strip_prefix('#') {
            Some(names) => header = names,
            None => last = Some(line),
        }
    }
    let Some(last) = last else {
        return Stats::default();
    };
    let names: Vec<&str> = header.split(',').map(str::trim).collect();
    let row: Vec<&str> = last.split(',').map(str::trim).collect();
    let column = |name: &str| {
        names
            .iter()
            .position(|h| *h == name)
            .and_then(|i| row.get(i))
            .copied()
    };
    Stats {
        execs: column(execs).and_then(|v| v.parse().ok()),
        execs_per_sec: column(execs_per_sec).and_then(|v| v.parse().ok()),
    }
}

pub fn harness_env(req: &RunRequest<'_>) -> Vec<(String, String)> {
    let mut env = harness_env_for(
        req.generator,
        TRUST,
        req.heap_bytes,
        req.tally,
        req.allow_unstable_addresses,
    );
    env.push((
        crate::wire::ORACLE_STATUS_ENV.to_string(),
        req.oracle_status.display().to_string(),
    ));
    env
}

pub fn harness_env_for(
    generator: &Path,
    trust: Trust,
    heap_bytes: u64,
    tally: &Path,
    allow_unstable_addresses: bool,
) -> Vec<(String, String)> {
    vec![
        (
            crate::wire::GENERATOR_ENV.to_string(),
            generator.display().to_string(),
        ),
        (crate::wire::TRUST_ENV.to_string(), trust.name().to_string()),
        (
            crate::wire::HEAP_BYTES_ENV.to_string(),
            heap_bytes.to_string(),
        ),
        (
            crate::wire::ALLOW_UNSTABLE_ADDRESSES_ENV.to_string(),
            crate::wire::unstable_addresses_token(allow_unstable_addresses).to_string(),
        ),
        (
            crate::wire::TALLY_ENV.to_string(),
            tally.display().to_string(),
        ),
    ]
}
fn mkdir(path: &Path) -> Result<(), ExternalError> {
    std::fs::create_dir_all(path).map_err(|source| ExternalError::Io {
        path: path.to_path_buf(),
        source,
    })
}
