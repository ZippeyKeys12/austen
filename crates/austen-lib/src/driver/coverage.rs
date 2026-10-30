use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::coverage::{Entry, Provenance, Timing, read_runs_from};
use crate::findings::{digest, file_holds};
use crate::instrumentation::Instrumentation;
use crate::observe::{RunEvent, Warning};
use crate::wire::{CoverPage, EdgeSet};
use crate::{Measured, Replay, Verdict};

use super::RunConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationBinary {
    pub harness: PathBuf,
    pub object: Option<PathBuf>,
    pub instrumentation: Instrumentation,
}

impl EvaluationBinary {
    pub fn observed(&self) -> Result<&Self, String> {
        match self.instrumentation {
            Instrumentation::Compiled | Instrumentation::Detected => Ok(self),
            Instrumentation::Absent => Err(format!(
                "{} carries no coverage instrumentation",
                self.harness.display()
            )),
            Instrumentation::Unknown => Err(format!(
                "coverage instrumentation in {} could not be established",
                self.harness.display()
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CoverageOutcome {
    Measured(CoverageSummary),
    Skipped(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub elapsed_ms: Option<u64>,
    pub covered: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CoverageSummary {
    pub harness: PathBuf,
    pub harness_digest: String,
    pub instrumentation: Instrumentation,
    pub page: Option<CoverPage>,
    pub provenance: Option<Provenance>,
    pub entries: usize,
    pub replayed: usize,
    pub verdicts: BTreeMap<Verdict, usize>,
    pub covered: EdgeSet,
    pub trajectory: Vec<Point>,
    pub timing: Timing,
    pub edges_out: Option<PathBuf>,
}

impl CoverageSummary {
    pub fn covered(&self) -> usize {
        self.covered.len()
    }

    pub fn capped(&self) -> bool {
        self.replayed < self.entries
    }

    pub fn sites(&self) -> Option<u64> {
        self.page.as_ref().and_then(|p| p.sites)
    }

    pub fn collision_free(&self) -> Option<bool> {
        self.page.as_ref().and_then(CoverPage::collision_free)
    }

    pub fn map_bytes(&self) -> Option<u64> {
        self.page.as_ref().map(|p| p.map_bytes)
    }
}

pub(super) fn run(
    cfg: &RunConfig,
    binary: &EvaluationBinary,
    generator: &Path,
    snapshot_interval: Duration,
    observation_complete: bool,
    clock: &super::schedule::ScheduleClock,
) -> Result<CoverageSummary, String> {
    let _phase = cfg.reporter.phase(crate::observe::Phase::Coverage);
    let cov_harness = binary.harness.as_path();
    let journal = cfg.journal();
    let Some(sections) = read_runs_from(
        &journal,
        clock.journal_offsets.first().copied().unwrap_or(0),
    ) else {
        return Err(format!(
            "{} is missing or is not a corpus journal, so there is nothing to \
             replay",
            journal.display()
        ));
    };
    let entries: Vec<Entry> = sections
        .into_iter()
        .flat_map(|section| {
            let shift = clock.shift_for(section.offset);
            section.entries.into_iter().map(move |mut entry| {
                entry.elapsed_ms = entry.elapsed_ms.map(|ms| ms.saturating_add(shift));
                entry
            })
        })
        .collect();
    let digest = match binary.object.as_deref().map(digest_of) {
        Some(Ok(digest)) => digest,
        Some(Err(why)) => return Err(why),
        None => String::new(),
    };

    let env = crate::external::harness_env_for(
        generator,
        crate::shrink::TRUST,
        cfg.heap_bytes,
        &cfg.work.join("coverage-tally"),
        cfg.allow_unstable_addresses,
    );
    let replay = Replay {
        harness: cov_harness,
        env: &env,
        tally: &cfg.work.join("coverage-tally"),
        cover: Some(&cfg.work.join("coverage-page")),
    };
    let interval_ms = snapshot_interval_ms(snapshot_interval)?;

    let mut summary = CoverageSummary {
        harness: cov_harness.to_path_buf(),
        harness_digest: digest,
        instrumentation: binary.instrumentation,
        page: None,
        provenance: provenance_of(&entries),
        entries: 0,
        replayed: 0,
        verdicts: BTreeMap::new(),
        covered: EdgeSet::default(),
        trajectory: Vec::new(),
        timing: Timing::Unknown,
        edges_out: None,
    };
    let mut seen = SeenInputs::default();
    let mut measured_entries: Vec<&Entry> = Vec::new();
    let mut trajectory = TrajectoryBuckets::new(interval_ms);
    let mut page_less_verdicts: BTreeMap<Verdict, usize> = BTreeMap::new();
    for entry in &entries {
        let Ok(bytes) = std::fs::read(&entry.path) else {
            summary.entries += 1;
            measured_entries.push(entry);
            continue;
        };
        if !seen.insert(&entry.path, &bytes) {
            continue;
        }
        summary.entries += 1;
        measured_entries.push(entry);
        match replay.measure(&bytes) {
            Ok(Measured { verdict, cover, .. }) => {
                summary.replayed += 1;
                *summary.verdicts.entry(verdict).or_default() += 1;
                if cover.is_none() {
                    *page_less_verdicts.entry(verdict).or_default() += 1;
                }
                trajectory.observe(entry.elapsed_ms, cover.as_ref().map(|page| &page.edges));
                if let Some(page) = cover {
                    summary.covered.absorb(&page.edges);
                    summary.page = Some(page);
                }
            }
            Err(e) => {
                let path = entry.path.clone();
                let message = e.to_string();
                cfg.reporter.emit(|| {
                    RunEvent::Warning(Warning::CoverageReplayFailed {
                        entry: path,
                        message,
                    })
                });
            }
        }
    }
    summary.trajectory = trajectory.finish();
    summary.timing = timing_of(measured_entries.iter().copied(), observation_complete);

    if let Some((message, warning)) = unavailable_reason(&summary) {
        cfg.reporter.emit(|| RunEvent::Warning(warning));
        return Err(message);
    }
    warn_about(cfg, &summary, &page_less_verdicts);
    summary.edges_out = write_edges(cfg, &summary);
    Ok(summary)
}

fn snapshot_interval_ms(interval: Duration) -> Result<u64, String> {
    let millis = u64::try_from(interval.as_millis())
        .map_err(|_| "the corpus snapshot interval is too large to report in milliseconds")?;
    if millis == 0 || Duration::from_millis(millis) != interval {
        return Err(
            "the corpus snapshot interval must be a positive whole number of milliseconds"
                .to_string(),
        );
    }
    Ok(millis)
}

struct TrajectoryBuckets {
    interval_ms: u64,
    edges: BTreeMap<u64, EdgeSet>,
}

impl TrajectoryBuckets {
    fn new(interval_ms: u64) -> Self {
        debug_assert!(interval_ms > 0);
        Self {
            interval_ms,
            edges: BTreeMap::new(),
        }
    }

    fn observe(&mut self, elapsed_ms: Option<u64>, edges: Option<&EdgeSet>) {
        let Some(checkpoint) =
            elapsed_ms.and_then(|elapsed_ms| right_edge_checkpoint(elapsed_ms, self.interval_ms))
        else {
            return;
        };
        let bucket = self.edges.entry(checkpoint).or_default();
        if let Some(edges) = edges {
            bucket.absorb(edges);
        }
    }

    fn finish(self) -> Vec<Point> {
        let mut covered = EdgeSet::default();
        self.edges
            .into_iter()
            .map(|(elapsed_ms, edges)| {
                covered.absorb(&edges);
                Point {
                    elapsed_ms: Some(elapsed_ms),
                    covered: covered.len(),
                }
            })
            .collect()
    }
}

fn right_edge_checkpoint(elapsed_ms: u64, interval_ms: u64) -> Option<u64> {
    if interval_ms == 0 {
        return None;
    }
    if elapsed_ms == 0 || elapsed_ms.is_multiple_of(interval_ms) {
        return Some(elapsed_ms);
    }
    elapsed_ms
        .checked_div(interval_ms)?
        .checked_add(1)?
        .checked_mul(interval_ms)
}

#[derive(Default)]
struct SeenInputs {
    buckets: HashMap<u64, Vec<PathBuf>>,
}

impl SeenInputs {
    fn insert(&mut self, path: &Path, bytes: &[u8]) -> bool {
        let bucket = self.buckets.entry(digest(bytes)).or_default();
        if bucket
            .iter()
            .any(|previous| file_holds(previous, bytes).unwrap_or(false))
        {
            return false;
        }
        bucket.push(path.to_path_buf());
        true
    }
}

fn unavailable_reason(summary: &CoverageSummary) -> Option<(String, Warning)> {
    summary.sites().is_none().then(|| {
        if summary.page.is_some() {
            (
                format!(
                    "{} published no instrumented-site denominator",
                    summary.harness.display()
                ),
                Warning::CoverageDenominatorMissing {
                    harness: summary.harness.clone(),
                },
            )
        } else {
            let message = format!(
                "{} published no coverage page for the recorded population",
                summary.harness.display()
            );
            (message.clone(), Warning::CoverageUnavailable { message })
        }
    })
}

fn warn_about(
    cfg: &RunConfig,
    summary: &CoverageSummary,
    page_less_verdicts: &BTreeMap<Verdict, usize>,
) {
    if summary.collision_free() == Some(false)
        && let (Some(sites), Some(map_bytes)) = (summary.sites(), summary.map_bytes())
    {
        cfg.reporter
            .emit(|| RunEvent::Warning(Warning::CoverageAliased { sites, map_bytes }));
    }
    let lost: BTreeMap<String, usize> = page_less_verdicts
        .iter()
        .map(|(v, n)| (v.token().to_string(), *n))
        .collect();
    if !lost.is_empty() {
        cfg.reporter
            .emit(|| RunEvent::Warning(Warning::CoverageEntriesLost { verdicts: lost }));
    }
}

fn timing_of<'a>(
    entries: impl IntoIterator<Item = &'a Entry>,
    observation_complete: bool,
) -> Timing {
    if !observation_complete {
        return Timing::Unknown;
    }
    let mut seen: Option<Timing> = None;
    for entry in entries.into_iter().filter(|e| e.elapsed_ms.is_some()) {
        match seen {
            None => seen = Some(entry.timing),
            Some(t) if t == entry.timing => {}
            Some(_) => return Timing::Unknown,
        }
    }
    seen.unwrap_or(Timing::Unknown)
}

fn provenance_of(entries: &[Entry]) -> Option<Provenance> {
    let dominant = |a: Provenance| entries.iter().any(|entry| entry.why == a);
    if entries.is_empty() {
        return None;
    }
    Some(
        if dominant(Provenance::Tool) || dominant(Provenance::Calibration) {
            Provenance::Tool
        } else if dominant(Provenance::Novel) {
            Provenance::Novel
        } else if dominant(Provenance::Recorded) {
            Provenance::Recorded
        } else {
            Provenance::Seed
        },
    )
}

fn write_edges(cfg: &RunConfig, summary: &CoverageSummary) -> Option<PathBuf> {
    use std::fmt::Write as _;
    let path = cfg.work.join("coverage.edges");
    let mut text = format!(
        "austen-edges 1 {} {}\n",
        summary.map_bytes().unwrap_or(0),
        summary.covered.len()
    );
    for id in summary.covered.iter() {
        let _ = writeln!(text, "{id}");
    }
    match std::fs::write(&path, text) {
        Ok(()) => Some(path),
        Err(e) => {
            let message = e.to_string();
            cfg.reporter
                .emit(|| RunEvent::Warning(Warning::CoverageEdgesWriteFailed { path, message }));
            None
        }
    }
}

fn digest_of(path: &Path) -> Result<String, String> {
    std::fs::read(path)
        .map(|bytes| digest(&bytes))
        .map(|d| format!("{d:016x}"))
        .map_err(|e| format!("reading {} to digest it: {e}", path.display()))
}
