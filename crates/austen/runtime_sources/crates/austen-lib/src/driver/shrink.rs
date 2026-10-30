use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::Verdict;
use crate::observe::{Reporter, RunEvent, Warning};
use crate::shrink::{AnyShrinker, Outcome, ShrinkRequest, Shrinker, Shrunk};
use crate::{RetainedArtifact, RetainedArtifacts};

use super::RunConfig;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShrinkSummary {
    pub tool: &'static str,
    pub harness: PathBuf,
    pub stored: usize,
    pub eligible: usize,
    pub attempted: usize,
    pub shrunk: usize,
    rejected_candidates: usize,
    unchanged_candidates: usize,
    pub bytes_before: usize,
    pub bytes_after: usize,
    pub verdicts: BTreeMap<Verdict, usize>,
    pub out: PathBuf,
}

impl ShrinkSummary {
    pub fn rejected(&self) -> usize {
        self.rejected_candidates
    }

    pub fn unchanged(&self) -> usize {
        self.unchanged_candidates
    }

    pub fn crashed(&self) -> usize {
        self.verdicts.get(&Verdict::Crash).copied().unwrap_or(0)
    }

    pub fn replayed(&self) -> usize {
        self.verdicts.values().sum()
    }

    pub fn capped(&self) -> bool {
        false
    }
}

pub(super) struct ShrinkRun {
    pub summary: ShrinkSummary,
    pub reductions: RetainedArtifacts,
}

pub(super) fn run(
    cfg: &RunConfig,
    shrinker: &AnyShrinker,
    generator: &Path,
    stored: usize,
    candidates: &RetainedArtifacts,
) -> ShrinkRun {
    let out = cfg.work.join("shrunk");
    let harness = cfg.shrink_harness.as_deref().unwrap_or(&cfg.harness);
    let taking = crash_candidates(candidates);
    let total = taking.len();
    let mut summary = ShrinkSummary {
        tool: Shrinker::name(shrinker),
        harness: harness.to_path_buf(),
        stored,
        eligible: total,
        out: out.clone(),
        ..ShrinkSummary::default()
    };
    let mut reductions = RetainedArtifacts::default();

    let work = cfg.work.join("shrink");
    let tally = cfg.work.join("shrink-tally");
    let journal = cfg.journal();
    for candidate in taking {
        let input = &candidate.path;
        let req = ShrinkRequest {
            harness,
            generator,
            heap_bytes: cfg.heap_bytes,
            allow_unstable_addresses: cfg.allow_unstable_addresses,
            input,
            out: &out,
            work: &work,
            tally: &tally,
            journal: &journal,
            budget_secs: cfg.duration_secs,
        };
        summary.attempted += 1;
        match shrinker.shrink(&req) {
            Ok(shrunk) => {
                if let Some(reduced) = record(&mut summary, &shrunk, &cfg.reporter) {
                    reductions.insert(reduced);
                }
            }
            Err(e) => {
                let (input, message) = (input.clone(), e.to_string());
                cfg.reporter
                    .emit(|| RunEvent::Warning(Warning::ShrinkFailed { input, message }));
            }
        }
        let shrunk = summary.shrunk;
        cfg.reporter
            .emit(|| RunEvent::ShrinkProgress { shrunk, total });
    }
    ShrinkRun {
        summary,
        reductions,
    }
}

fn crash_candidates(candidates: &RetainedArtifacts) -> Vec<&RetainedArtifact> {
    candidates
        .iter()
        .filter(|artifact| artifact.outcome.is_crash_finding())
        .collect()
}

fn record(
    summary: &mut ShrinkSummary,
    shrunk: &Shrunk,
    reporter: &Reporter,
) -> Option<RetainedArtifact> {
    *summary.verdicts.entry(shrunk.verdict).or_default() += 1;
    summary.bytes_before += shrunk.before;
    summary.bytes_after += shrunk.after;
    match shrunk.outcome() {
        Outcome::Shrunk => {
            summary.shrunk += 1;
            Some(RetainedArtifact::new(
                shrunk
                    .outcome
                    .expect("an accepted crashing reduction has a classified outcome"),
                shrunk.kept.clone(),
            ))
        }
        Outcome::Unchanged => {
            summary.unchanged_candidates += 1;
            None
        }
        Outcome::Rejected => {
            summary.rejected_candidates += 1;
            let (input, verdict) = (shrunk.original.clone(), shrunk.verdict.token().to_string());
            reporter.emit(|| RunEvent::Warning(Warning::ShrinkRejected { input, verdict }));
            None
        }
    }
}
