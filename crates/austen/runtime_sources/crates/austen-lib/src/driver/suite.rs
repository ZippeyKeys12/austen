use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::observe::{Reporter, RunEvent, SuitePass, Warning};
use crate::shrink::{SuiteRequest, SuiteTool, SuiteToolHandle};
use crate::wire::{Trust, remove_mode_vars};
use crate::{
    ArtifactOutcome, Replay, RetainedArtifact, RetainedArtifacts, external::harness_env_for,
};
use strum::IntoEnumIterator;

use super::{EvaluationBinary, RunConfig};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SuiteOutcomeSummary {
    pub eligible: usize,
    pub retained: usize,
    pub fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SuitePassSummary {
    pub eligible: usize,
    pub retained: usize,
    pub outcomes: BTreeMap<ArtifactOutcome, SuiteOutcomeSummary>,
    pub out: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuiteMinimizationSummary {
    pub tool: &'static str,
    pub harness: PathBuf,
    pub initial: SuitePassSummary,
    pub final_pass: Option<SuitePassSummary>,
}

pub(super) struct SuitePassResult {
    pub summary: SuitePassSummary,
    pub retained: RetainedArtifacts,
}

fn minimizes(pass: SuitePass, outcome: ArtifactOutcome) -> bool {
    match pass {
        SuitePass::Initial => true,
        SuitePass::Final => outcome.is_crash_finding(),
    }
}

struct Layout {
    scratch: PathBuf,
    materialized: PathBuf,
}

impl Layout {
    fn published(cfg: &RunConfig) -> PathBuf {
        cfg.work.join("minimized-suite")
    }

    fn of(cfg: &RunConfig, pass: SuitePass, publish: bool) -> Layout {
        let scratch = cfg.work.join("suite-minimize").join(pass.token());
        let materialized = if publish {
            Layout::published(cfg)
        } else {
            scratch.join("suite")
        };
        Layout {
            scratch,
            materialized,
        }
    }
}

pub(super) fn with_reductions(
    mut originals: RetainedArtifacts,
    reductions: RetainedArtifacts,
) -> RetainedArtifacts {
    originals.extend(reductions);
    originals
}

pub(super) fn run_pass(
    cfg: &RunConfig,
    tool: &SuiteToolHandle,
    binary: Result<&EvaluationBinary, &str>,
    generator: &Path,
    pass: SuitePass,
    candidates: &RetainedArtifacts,
    publish: bool,
) -> SuitePassResult {
    if pass == SuitePass::Initial && !publish {
        let previous = Layout::published(cfg);
        if let Err(message) = clear_dir(&previous) {
            warn_failure(
                &cfg.reporter,
                pass,
                None,
                format!(
                    "could not clear the previous published suite at {}: {message}",
                    previous.display()
                ),
            );
        }
    }

    let Layout {
        scratch,
        materialized,
    } = Layout::of(cfg, pass, publish);
    let give_up = |message: String| fallback_pass(cfg, pass, candidates, &materialized, message);

    let binary = match binary {
        Ok(binary) => binary,
        Err(message) => return give_up(message.to_string()),
    };
    if let Err(error) = reset_dir(&scratch) {
        return give_up(error);
    }
    if publish && let Err(error) = reset_dir(&materialized) {
        return give_up(error);
    }
    let ctx = match (|| {
        Ok::<_, String>(PassCtx {
            cfg,
            tool,
            binary,
            program: SuiteTool::preflight(tool).map_err(|error| error.to_string())?,
            harness: absolute_path(&binary.harness)?,
            generator: absolute_path(generator)?,
            scratch: absolute_path(&scratch)?,
        })
    })() {
        Ok(ctx) => ctx,
        Err(message) => return give_up(message),
    };

    let mut selected = RetainedArtifacts::default();
    let mut summary = SuitePassSummary {
        eligible: candidates.len(),
        out: materialized.clone(),
        ..SuitePassSummary::default()
    };

    for outcome in ArtifactOutcome::iter() {
        let group: Vec<&RetainedArtifact> = candidates.for_outcome(outcome).collect();
        let total = group.len();
        let (kept, fallback) = if !minimizes(pass, outcome) || total <= 1 {
            (group.into_iter().cloned().collect(), false)
        } else {
            match ctx.minimize_group(outcome, &group) {
                Ok(kept) => (kept, false),
                Err(message) => {
                    warn_failure(&cfg.reporter, pass, Some(outcome), message);
                    (group.into_iter().cloned().collect(), true)
                }
            }
        };

        let retained = kept.len();
        summary.outcomes.insert(
            outcome,
            SuiteOutcomeSummary {
                eligible: total,
                retained,
                fallback,
            },
        );
        summary.retained += retained;
        selected.extend(kept);
        cfg.reporter.emit(|| RunEvent::SuiteMinimizationProgress {
            pass,
            outcome: outcome.token().to_string(),
            retained,
            total,
        });
    }

    if let Err(message) = materialize(&materialized, &selected) {
        warn_failure(
            &cfg.reporter,
            pass,
            None,
            format!(
                "could not materialize {}: {message}",
                materialized.display()
            ),
        );
    }

    SuitePassResult {
        summary,
        retained: selected,
    }
}

struct PassCtx<'a> {
    cfg: &'a RunConfig,
    tool: &'a SuiteToolHandle,
    binary: &'a EvaluationBinary,
    program: PathBuf,
    harness: PathBuf,
    generator: PathBuf,
    scratch: PathBuf,
}

impl PassCtx<'_> {
    fn minimize_group(
        &self,
        outcome: ArtifactOutcome,
        candidates: &[&RetainedArtifact],
    ) -> Result<Vec<RetainedArtifact>, String> {
        let group = self.scratch.join(outcome.token());
        let input_dir = group.join("in");
        let output_dir = group.join("out");
        std::fs::create_dir_all(&input_dir)
            .map_err(|error| format!("create {}: {error}", input_dir.display()))?;

        let tally = group.join("tally");
        let env = harness_env_for(
            &self.generator,
            Trust::Checked,
            self.cfg.heap_bytes,
            &tally,
            self.cfg.allow_unstable_addresses,
        );
        let replay = Replay {
            harness: &self.binary.harness,
            env: &env,
            tally: &tally,
            cover: None,
        };
        let mut by_bytes: BTreeMap<Vec<u8>, &RetainedArtifact> = BTreeMap::new();
        for (index, candidate) in candidates.iter().enumerate() {
            let bytes = std::fs::read(&candidate.path)
                .map_err(|error| format!("read {}: {error}", candidate.path.display()))?;
            let measured = replay
                .measure(&bytes)
                .map_err(|error| format!("replay {}: {error}", candidate.path.display()))?;
            if !outcome.agrees_with(&measured) {
                return Err(format!(
                    "{} replayed as {}{} in the common evaluation harness, not {}",
                    candidate.path.display(),
                    measured.verdict.token(),
                    if measured.post {
                        " (postcondition)"
                    } else {
                        ""
                    },
                    outcome.token(),
                ));
            }
            let staged = input_dir.join(format!("id-{index:08}"));
            std::fs::write(&staged, &bytes)
                .map_err(|error| format!("write {}: {error}", staged.display()))?;
            by_bytes.entry(bytes).or_insert(candidate);
        }

        let request = SuiteRequest {
            checked_coverage_harness: &self.harness,
            generator: &self.generator,
            heap_bytes: self.cfg.heap_bytes,
            allow_unstable_addresses: self.cfg.allow_unstable_addresses,
            input_dir: &input_dir,
            output_dir: &output_dir,
            tally: &tally,
            crashes_only: outcome.is_crash_finding(),
        };
        let plan = SuiteTool::suite_plan(self.tool, &request).map_err(|error| error.to_string())?;
        let program = &self.program;
        let status = remove_mode_vars(&mut Command::new(program))
            .args(&plan.argv)
            .envs(plan.env.iter().map(|(key, value)| (key, value)))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("run {}: {error}", program.display()))?;
        if !status.success() {
            return Err(format!("{} exited with {status}", program.display()));
        }

        let entries = std::fs::read_dir(&output_dir)
            .map_err(|error| format!("read {}: {error}", output_dir.display()))?;
        let mut kept = BTreeSet::new();
        for entry in entries {
            let entry = entry.map_err(|error| format!("read cmin output entry: {error}"))?;
            let ty = entry
                .file_type()
                .map_err(|error| format!("inspect {}: {error}", entry.path().display()))?;
            if !ty.is_file() {
                continue;
            }
            let bytes = std::fs::read(entry.path())
                .map_err(|error| format!("read {}: {error}", entry.path().display()))?;
            let Some(candidate) = by_bytes.get(&bytes) else {
                return Err(format!(
                    "{} produced bytes that were not in the staged {} group",
                    program.display(),
                    outcome.token(),
                ));
            };
            kept.insert((*candidate).clone());
        }
        if kept.is_empty() {
            return Err(format!(
                "{} retained no inputs from the non-empty {} group",
                program.display(),
                outcome.token(),
            ));
        }

        Ok(kept.into_iter().collect())
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    std::path::absolute(path).map_err(|error| {
        format!(
            "resolve {} from the current directory: {error}",
            path.display()
        )
    })
}

fn fallback_pass(
    cfg: &RunConfig,
    pass: SuitePass,
    candidates: &RetainedArtifacts,
    out: &Path,
    message: String,
) -> SuitePassResult {
    let mut outcomes = BTreeMap::new();
    for outcome in ArtifactOutcome::iter() {
        let eligible = candidates.for_outcome(outcome).count();
        let fallback = minimizes(pass, outcome) && eligible > 1;
        outcomes.insert(
            outcome,
            SuiteOutcomeSummary {
                eligible,
                retained: eligible,
                fallback,
            },
        );
        if fallback {
            warn_failure(&cfg.reporter, pass, Some(outcome), message.clone());
        }
    }
    if let Err(error) = reset_dir(out).and_then(|()| materialize(out, candidates)) {
        warn_failure(
            &cfg.reporter,
            pass,
            None,
            format!(
                "could not materialize the unreduced fallback at {}: {error}",
                out.display()
            ),
        );
    }
    SuitePassResult {
        summary: SuitePassSummary {
            eligible: candidates.len(),
            retained: candidates.len(),
            outcomes,
            out: out.to_path_buf(),
        },
        retained: candidates.clone(),
    }
}

fn warn_failure(
    reporter: &Reporter,
    pass: SuitePass,
    outcome: Option<ArtifactOutcome>,
    message: String,
) {
    let outcome = outcome.map(|outcome| outcome.token().to_string());
    reporter.emit(|| {
        RunEvent::Warning(Warning::SuiteMinimizationFailed {
            pass,
            outcome,
            message,
        })
    });
}

fn materialize(out: &Path, candidates: &RetainedArtifacts) -> Result<(), String> {
    for candidate in candidates {
        let dir = out.join(candidate.outcome.token());
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("create {}: {error}", dir.display()))?;
        let name = candidate
            .path
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("input"));
        let destination = dir.join(name);
        std::fs::copy(&candidate.path, &destination).map_err(|error| {
            format!(
                "copy {} to {}: {error}",
                candidate.path.display(),
                destination.display()
            )
        })?;
    }
    Ok(())
}

fn reset_dir(path: &Path) -> Result<(), String> {
    clear_dir(path)?;
    std::fs::create_dir_all(path).map_err(|error| format!("create {}: {error}", path.display()))
}

fn clear_dir(path: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("clear {}: {error}", path.display())),
    }
    Ok(())
}
