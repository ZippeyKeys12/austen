use std::path::{Path, PathBuf};
use std::process::Command;

use crate::external::{AnyFuzzer, Discovery, NotFound, harness_env_for};
use crate::wire::{Trust, remove_mode_vars};
use crate::{
    Journal,
    findings::{
        ArtifactOutcome, FindingsError, HarnessValidator, Replay, Store, ValidationSession, Verdict,
    },
};
use enum_dispatch::enum_dispatch;

pub const TRUST: Trust = Trust::Checked;

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumIter)]
pub enum HarnessKind {
    Forkserver,
    InProcess,
}

impl HarnessKind {
    pub fn token(self) -> &'static str {
        match self {
            HarnessKind::Forkserver => "forkserver",
            HarnessKind::InProcess => "in-process",
        }
    }

    pub fn of_link(link: Option<&crate::external::HarnessLink>) -> HarnessKind {
        match link {
            Some(link) if link.driver_symbol.is_some() => HarnessKind::InProcess,
            _ => HarnessKind::Forkserver,
        }
    }

    pub fn of_binary(
        bin: &Path,
        registry: &crate::engine::EngineRegistry,
    ) -> Result<HarnessKind, crate::object::ProbeError> {
        Ok(match registry.driver_in(bin)? {
            Some(_) => HarnessKind::InProcess,
            None => HarnessKind::Forkserver,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program {
    Tool,
    Harness,
}

pub struct ShrinkRequest<'a> {
    pub harness: &'a Path,
    pub generator: &'a Path,
    pub heap_bytes: u64,
    pub allow_unstable_addresses: bool,
    pub input: &'a Path,
    pub out: &'a Path,
    pub work: &'a Path,
    pub tally: &'a Path,
    pub journal: &'a Path,
    pub budget_secs: Option<u64>,
}

pub fn harness_env(req: &ShrinkRequest<'_>) -> Vec<(String, String)> {
    harness_env_for(
        req.generator,
        TRUST,
        req.heap_bytes,
        req.tally,
        req.allow_unstable_addresses,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShrinkPlan {
    pub program: Program,
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub out: PathBuf,
}

pub struct SuiteRequest<'a> {
    pub checked_coverage_harness: &'a Path,
    pub generator: &'a Path,
    pub heap_bytes: u64,
    pub allow_unstable_addresses: bool,
    pub input_dir: &'a Path,
    pub output_dir: &'a Path,
    pub tally: &'a Path,
    pub crashes_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuitePlan {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SuitePlanError {
    #[error("suite coverage harness must be an absolute path: {path}")]
    HarnessNotAbsolute { path: PathBuf },
}

pub trait SuiteTool {
    fn name(&self) -> &'static str;

    fn suite_plan(&self, req: &SuiteRequest<'_>) -> Result<SuitePlan, SuitePlanError>;

    fn discovery(&self) -> Discovery;

    fn discover(&self) -> Result<PathBuf, NotFound> {
        self.discovery().locate()
    }

    fn preflight(&self) -> Result<PathBuf, NotFound> {
        self.discover()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shrunk {
    pub original: PathBuf,
    pub kept: PathBuf,
    pub before: usize,
    pub after: usize,
    pub verdict: Verdict,
    pub outcome: Option<ArtifactOutcome>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Shrunk,
    Unchanged,
    Rejected,
}

impl Shrunk {
    pub fn outcome(&self) -> Outcome {
        match (
            self.outcome.is_some_and(ArtifactOutcome::is_crash_finding),
            self.after < self.before,
        ) {
            (true, true) => Outcome::Shrunk,
            (true, false) => Outcome::Unchanged,
            _ => Outcome::Rejected,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ShrinkError {
    #[error("{0}")]
    NotFound(#[from] NotFound),
    #[error("failed to run {program}: {source}")]
    Spawn {
        program: String,
        source: std::io::Error,
    },
    #[error("{program} exited with {status}")]
    Failed { program: String, status: String },
    #[error("{path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("could not verify the reduced input: {0}")]
    Verify(String),
    #[error("{0}")]
    Findings(#[from] FindingsError),
}

#[enum_dispatch]
pub trait ShrinkTool {
    fn name(&self) -> &'static str;

    fn drives(&self) -> HarnessKind;

    fn builds_with(&self) -> Option<AnyFuzzer>;

    fn shrink_plan(&self, req: &ShrinkRequest<'_>) -> ShrinkPlan;

    fn discovery(&self) -> Discovery;

    fn discover(&self) -> Result<PathBuf, NotFound> {
        self.discovery().locate()
    }
}

#[derive(Clone, Copy)]
pub struct AnyShrinkTool(&'static (dyn ShrinkTool + Sync));
impl AnyShrinkTool {
    pub const fn new(tool: &'static (dyn ShrinkTool + Sync)) -> Self {
        Self(tool)
    }
}
impl std::fmt::Debug for AnyShrinkTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("AnyShrinkTool")
            .field(&self.0.name())
            .finish()
    }
}
impl PartialEq for AnyShrinkTool {
    fn eq(&self, other: &Self) -> bool {
        self.0.name() == other.0.name()
    }
}
impl Eq for AnyShrinkTool {}
impl ShrinkTool for AnyShrinkTool {
    fn name(&self) -> &'static str {
        self.0.name()
    }
    fn drives(&self) -> HarnessKind {
        self.0.drives()
    }
    fn builds_with(&self) -> Option<AnyFuzzer> {
        self.0.builds_with()
    }
    fn shrink_plan(&self, req: &ShrinkRequest<'_>) -> ShrinkPlan {
        self.0.shrink_plan(req)
    }
    fn discovery(&self) -> Discovery {
        self.0.discovery()
    }
    fn discover(&self) -> Result<PathBuf, NotFound> {
        self.0.discover()
    }
}

#[derive(Clone, Copy)]
pub struct SuiteToolHandle(&'static (dyn SuiteTool + Sync));
impl SuiteToolHandle {
    pub const fn new(tool: &'static (dyn SuiteTool + Sync)) -> Self {
        Self(tool)
    }
}
impl std::fmt::Debug for SuiteToolHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SuiteToolHandle")
            .field(&self.0.name())
            .finish()
    }
}
impl PartialEq for SuiteToolHandle {
    fn eq(&self, other: &Self) -> bool {
        self.0.name() == other.0.name()
    }
}
impl Eq for SuiteToolHandle {}
impl SuiteTool for SuiteToolHandle {
    fn name(&self) -> &'static str {
        self.0.name()
    }
    fn suite_plan(&self, req: &SuiteRequest<'_>) -> Result<SuitePlan, SuitePlanError> {
        self.0.suite_plan(req)
    }
    fn discovery(&self) -> Discovery {
        self.0.discovery()
    }
    fn discover(&self) -> Result<PathBuf, NotFound> {
        self.0.discover()
    }
    fn preflight(&self) -> Result<PathBuf, NotFound> {
        self.0.preflight()
    }
}

#[enum_dispatch]
pub trait Shrinker {
    fn name(&self) -> &'static str;

    fn drives(&self) -> HarnessKind;

    fn builds_with(&self) -> Option<AnyFuzzer>;

    fn preflight(&self) -> Result<PathBuf, NotFound>;

    fn shrink(&self, req: &ShrinkRequest<'_>) -> Result<Shrunk, ShrinkError>;
}

#[enum_dispatch(Shrinker)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnyShrinker {
    Tool(AnyShrinkTool),
}

impl Shrinker for AnyShrinkTool {
    fn name(&self) -> &'static str {
        ShrinkTool::name(self)
    }

    fn drives(&self) -> HarnessKind {
        ShrinkTool::drives(self)
    }

    fn builds_with(&self) -> Option<AnyFuzzer> {
        ShrinkTool::builds_with(self)
    }

    fn preflight(&self) -> Result<PathBuf, NotFound> {
        ShrinkTool::discover(self)
    }

    fn shrink(&self, req: &ShrinkRequest<'_>) -> Result<Shrunk, ShrinkError> {
        let before = size_of_file(req.input)?;
        let plan = ShrinkTool::shrink_plan(self, req);
        let program = match plan.program {
            Program::Tool => ShrinkTool::discover(self)?,
            Program::Harness => req.harness.to_path_buf(),
        };
        if let Some(scratch) = plan.out.parent() {
            mkdir(scratch)?;
        }
        let _ = std::fs::remove_file(&plan.out);

        let status = remove_mode_vars(&mut Command::new(&program))
            .args(&plan.argv)
            .envs(plan.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|source| ShrinkError::Spawn {
                program: program.display().to_string(),
                source,
            })?;

        tracing::debug!(
            target: "austen::shrink",
            program = %program.display(),
            status = %status,
            "shrink tool finished"
        );

        let Ok(candidate) = std::fs::read(&plan.out) else {
            return Ok(Shrunk {
                original: req.input.to_path_buf(),
                kept: req.input.to_path_buf(),
                before,
                after: before,
                verdict: Verdict::HarnessError,
                outcome: None,
            });
        };

        let env = harness_env(req);
        let validator = HarnessValidator::new(req.harness, env.clone());
        let mut journal = Journal::open(req.journal, "shrink", std::time::SystemTime::now());
        let mut validation = ValidationSession::new(&validator);
        let Some(artifact) = validation.validate(
            plan.out.clone(),
            candidate,
            std::fs::metadata(&plan.out)
                .and_then(|metadata| metadata.modified())
                .ok(),
            &mut journal,
        )?
        else {
            return Ok(Shrunk {
                original: req.input.to_path_buf(),
                kept: req.input.to_path_buf(),
                before,
                after: before,
                verdict: Verdict::NotDispatched,
                outcome: None,
            });
        };

        let replay = Replay {
            harness: req.harness,
            env: &env,
            tally: req.tally,
            cover: None,
        };
        let measured = replay
            .measure(artifact.bytes())
            .map_err(|e| ShrinkError::Verify(e.to_string()))?;
        let classified_outcome = artifact
            .classify(&measured)
            .map(|classified| classified.outcome());

        let shrunk = Shrunk {
            original: req.input.to_path_buf(),
            kept: req.input.to_path_buf(),
            before,
            after: artifact.bytes().len(),
            verdict: measured.verdict,
            outcome: classified_outcome,
        };
        if shrunk.outcome() == Outcome::Shrunk {
            let kept = Store::new(req.out).file(&artifact)?;
            return Ok(Shrunk { kept, ..shrunk });
        }
        Ok(Shrunk {
            after: before,
            ..shrunk
        })
    }
}

fn size_of_file(path: &Path) -> Result<usize, ShrinkError> {
    std::fs::metadata(path)
        .map(|m| m.len() as usize)
        .map_err(|source| ShrinkError::Io {
            path: path.display().to_string(),
            source,
        })
}

fn mkdir(path: &Path) -> Result<(), ShrinkError> {
    std::fs::create_dir_all(path).map_err(|source| ShrinkError::Io {
        path: path.display().to_string(),
        source,
    })
}
