mod replay;
mod store;
mod validation;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::observe::{Reporter, RunEvent, Warning};

pub use replay::{Measured, Replay};
pub use store::Store;
pub use store::{digest, file_holds};
pub use validation::{
    ArtifactValidator, ByteUse, ClassifiedArtifact, HarnessValidator, LaunchCandidate,
    LaunchValidationSession, ParseValidation, PreconditionValidation, ValidatedArtifact,
    ValidationEvidence, ValidationSession,
};

pub(crate) use validation::{
    DiskValidationSession, VALIDATION_CHUNK, retain_captured_candidate, validate_all_disk_backed,
    validate_in_chunks, validate_readable_fallback,
};

#[derive(Debug, thiserror::Error)]
pub enum FindingsError {
    #[error("could not run `{program}`: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not clean up incomplete archive state at {path} after {prior}: {source}")]
    Cleanup {
        path: PathBuf,
        prior: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not append required validation evidence to {path}: {message}")]
    Journal { path: PathBuf, message: String },
    #[error("artifact validation failed: {message}")]
    Validation { message: String },
}

impl FindingsError {
    pub(crate) fn is_storage_or_journal_failure(&self) -> bool {
        matches!(
            self,
            FindingsError::Io { .. } | FindingsError::Journal { .. }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, strum::EnumIter)]
pub enum Verdict {
    Crash,
    Survived,
    NotDispatched,
    NoSuchFunction,
    Timeout,
    HarnessError,
}

impl Verdict {
    pub fn token(self) -> &'static str {
        match self {
            Verdict::Crash => "crash",
            Verdict::Survived => "survived",
            Verdict::NotDispatched => "not-dispatched",
            Verdict::NoSuchFunction => "no-such-function",
            Verdict::Timeout => "timeout",
            Verdict::HarnessError => "harness-error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, strum::EnumIter)]
pub enum ArtifactOutcome {
    PreconditionFailing,
    Passing,
    Crash,
    PostconditionFailing,
}

impl ArtifactOutcome {
    pub const fn token(self) -> &'static str {
        match self {
            ArtifactOutcome::PreconditionFailing => "precondition-failing",
            ArtifactOutcome::Passing => "passing",
            ArtifactOutcome::Crash => "crash",
            ArtifactOutcome::PostconditionFailing => "postcondition-failing",
        }
    }

    pub fn classify(evidence: &ValidationEvidence, measured: &Measured) -> Option<ArtifactOutcome> {
        if !evidence.eligible() {
            return None;
        }
        match (evidence.precondition, measured.verdict, measured.post) {
            (PreconditionValidation::Failed, Verdict::NotDispatched, false) => {
                Some(ArtifactOutcome::PreconditionFailing)
            }
            (PreconditionValidation::Passed, Verdict::Survived, false) => {
                Some(ArtifactOutcome::Passing)
            }
            (PreconditionValidation::Passed, Verdict::Crash, false) => Some(ArtifactOutcome::Crash),
            (PreconditionValidation::Passed, Verdict::Crash, true) => {
                Some(ArtifactOutcome::PostconditionFailing)
            }
            _ => None,
        }
    }

    pub const fn is_crash_finding(self) -> bool {
        matches!(
            self,
            ArtifactOutcome::Crash | ArtifactOutcome::PostconditionFailing
        )
    }

    pub const fn verdict(self) -> Verdict {
        match self {
            ArtifactOutcome::PreconditionFailing => Verdict::NotDispatched,
            ArtifactOutcome::Passing => Verdict::Survived,
            ArtifactOutcome::Crash | ArtifactOutcome::PostconditionFailing => Verdict::Crash,
        }
    }

    pub fn agrees_with(self, measured: &Measured) -> bool {
        measured.verdict == self.verdict()
            && measured.post == matches!(self, ArtifactOutcome::PostconditionFailing)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RetainedArtifact {
    pub outcome: ArtifactOutcome,
    pub path: PathBuf,
}

impl RetainedArtifact {
    pub fn new(outcome: ArtifactOutcome, path: impl Into<PathBuf>) -> Self {
        Self {
            outcome,
            path: path.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetainedArtifacts {
    artifacts: BTreeSet<RetainedArtifact>,
}

impl RetainedArtifacts {
    pub fn insert(&mut self, artifact: RetainedArtifact) -> bool {
        self.artifacts.insert(artifact)
    }

    pub fn len(&self) -> usize {
        self.artifacts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.artifacts.is_empty()
    }

    pub fn iter(&self) -> std::collections::btree_set::Iter<'_, RetainedArtifact> {
        self.artifacts.iter()
    }

    pub fn for_outcome(&self, outcome: ArtifactOutcome) -> impl Iterator<Item = &RetainedArtifact> {
        self.artifacts
            .iter()
            .filter(move |artifact| artifact.outcome == outcome)
    }
}

impl FromIterator<RetainedArtifact> for RetainedArtifacts {
    fn from_iter<T: IntoIterator<Item = RetainedArtifact>>(iter: T) -> Self {
        Self {
            artifacts: iter.into_iter().collect(),
        }
    }
}

impl Extend<RetainedArtifact> for RetainedArtifacts {
    fn extend<T: IntoIterator<Item = RetainedArtifact>>(&mut self, iter: T) {
        self.artifacts.extend(iter);
    }
}

impl IntoIterator for RetainedArtifacts {
    type Item = RetainedArtifact;
    type IntoIter = std::collections::btree_set::IntoIter<RetainedArtifact>;

    fn into_iter(self) -> Self::IntoIter {
        self.artifacts.into_iter()
    }
}

impl<'a> IntoIterator for &'a RetainedArtifacts {
    type Item = &'a RetainedArtifact;
    type IntoIter = std::collections::btree_set::Iter<'a, RetainedArtifact>;

    fn into_iter(self) -> Self::IntoIter {
        self.artifacts.iter()
    }
}

fn mtime_timing(
    artifact: &ValidatedArtifact,
    started: std::time::SystemTime,
) -> (Option<u64>, crate::coverage::Timing) {
    let elapsed_ms = artifact.source_mtime().and_then(|at| {
        at.duration_since(started)
            .ok()
            .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
    });
    let timing = if elapsed_ms.is_some() {
        crate::coverage::Timing::Mtime
    } else {
        crate::coverage::Timing::Unknown
    };
    (elapsed_ms, timing)
}

fn retained_timing(
    artifact: &ValidatedArtifact,
    started: std::time::SystemTime,
) -> (Option<u64>, crate::coverage::Timing) {
    artifact.observed_ms().map_or_else(
        || mtime_timing(artifact, started),
        |elapsed_ms| (Some(elapsed_ms), crate::coverage::Timing::Observed),
    )
}

fn validate_captured(
    captured: &[crate::external::CapturedArtifact],
    backing_dir: &Path,
    next_sequence: &mut u64,
    session: &mut DiskValidationSession<'_>,
    reporter: &Reporter,
    journal: &mut crate::coverage::Journal,
) -> Result<(usize, Option<String>), FindingsError> {
    let mut ordered: Vec<&crate::external::CapturedArtifact> = captured.iter().collect();
    ordered.sort_by_key(|artifact| (artifact.elapsed_ms, artifact.sequence));
    let total = ordered.len();
    if total == 0 {
        return Ok((0, None));
    }

    reporter.emit(|| RunEvent::ValidationProgress {
        validated: 0,
        total,
    });
    let mut readable = 0usize;
    let mut capture_losses = Vec::new();
    for chunk in ordered.chunks(VALIDATION_CHUNK) {
        let mut candidates = Vec::with_capacity(chunk.len());
        for artifact in chunk {
            match retain_captured_candidate(
                &artifact.source_path,
                &artifact.capture_path,
                artifact.elapsed_ms,
                backing_dir,
                next_sequence,
            ) {
                Ok(candidate) => candidates.push(candidate),
                Err(error) => {
                    capture_losses.push(error.to_string());
                }
            }
        }
        if !candidates.is_empty() {
            readable += candidates.len();
            session.validate_observed_batch(candidates, journal)?;
            reporter.emit(|| RunEvent::ValidationProgress {
                validated: readable,
                total,
            });
        }
    }
    let capture_loss = match capture_losses.len() {
        0 => None,
        1 => capture_losses.pop(),
        unavailable => Some(format!(
            "{unavailable} managed capture files were unavailable: {}",
            example_list(&capture_losses)
        )),
    };
    Ok((total, capture_loss))
}

fn example_list(errors: &[String]) -> String {
    let mut examples: Vec<String> = errors.iter().take(3).cloned().collect();
    if errors.len() > examples.len() {
        examples.push(format!("and {} more", errors.len() - examples.len()));
    }
    examples.join("; ")
}

fn latch_capture_degradation(degradation: &mut Option<String>, reason: impl Into<String>) {
    let reason = reason.into();
    match degradation {
        Some(existing) => {
            existing.push_str("; ");
            existing.push_str(&reason);
        }
        None => *degradation = Some(reason),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactFolders {
    corpus_root: PathBuf,
    crashes_root: PathBuf,
    precondition_failing: PathBuf,
    passing: PathBuf,
    crash: PathBuf,
    postcondition_failing: PathBuf,
}

impl ArtifactFolders {
    pub fn new(corpus_root: impl AsRef<Path>, crashes_root: impl AsRef<Path>) -> ArtifactFolders {
        let corpus_root = corpus_root.as_ref();
        let crashes_root = crashes_root.as_ref();
        ArtifactFolders {
            corpus_root: corpus_root.to_path_buf(),
            crashes_root: crashes_root.to_path_buf(),
            precondition_failing: corpus_root.join(ArtifactOutcome::PreconditionFailing.token()),
            passing: corpus_root.join(ArtifactOutcome::Passing.token()),
            crash: crashes_root.join(ArtifactOutcome::Crash.token()),
            postcondition_failing: crashes_root.join(ArtifactOutcome::PostconditionFailing.token()),
        }
    }

    pub fn launch_seed_sources(&self) -> [PathBuf; 2] {
        [self.passing.clone(), self.precondition_failing.clone()]
    }

    pub fn path(&self, outcome: ArtifactOutcome) -> &Path {
        match outcome {
            ArtifactOutcome::PreconditionFailing => &self.precondition_failing,
            ArtifactOutcome::Passing => &self.passing,
            ArtifactOutcome::Crash => &self.crash,
            ArtifactOutcome::PostconditionFailing => &self.postcondition_failing,
        }
    }

    pub fn corpus_root(&self) -> &Path {
        &self.corpus_root
    }

    pub fn crashes_root(&self) -> &Path {
        &self.crashes_root
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Crashes {
    pub artifacts: usize,
    pub unique: usize,
    pub verdicts: BTreeMap<Verdict, usize>,
    pub filed: BTreeSet<PathBuf>,
    pub post_failed: usize,
}

impl Crashes {
    pub fn replayed(&self) -> usize {
        self.verdicts.values().sum()
    }

    pub fn confirmed(&self) -> usize {
        self.verdicts.get(&Verdict::Crash).copied().unwrap_or(0)
    }

    pub fn capped(&self) -> bool {
        self.replayed() < self.unique
    }

    pub fn observed(artifacts: usize, filed: BTreeSet<PathBuf>, post_failed: usize) -> Crashes {
        Crashes {
            artifacts,
            unique: filed.len(),
            verdicts: match artifacts {
                0 => BTreeMap::new(),
                n => BTreeMap::from([(Verdict::Crash, n)]),
            },
            filed,
            post_failed,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Corpus {
    pub artifacts: usize,
    pub unique: usize,
    pub dir: PathBuf,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn archive_external(
    crashes_dir: &Path,
    corpus_dir: Option<&Path>,
    captured: &[crate::external::CapturedArtifact],
    capture_error: Option<&str>,
    folders: &ArtifactFolders,
    not_findings: &[&str],
    replay: &Replay<'_>,
    reporter: &Reporter,
    validator: &dyn ArtifactValidator,
    journal: &mut crate::coverage::Journal,
) -> Result<(Crashes, Corpus, RetainedArtifacts, Option<String>), FindingsError> {
    let mut validation = DiskValidationSession::new(validator);
    let mut degradation = capture_error.map(str::to_owned);
    let mut source_errors = Vec::new();
    let import_storage = if !captured.is_empty() || degradation.is_some() {
        match tempfile::Builder::new()
            .prefix("austen-external-import-")
            .tempdir()
        {
            Ok(storage) => Some(storage),
            Err(source) => {
                let error = format!(
                    "could not prepare immutable external-import backing storage: {source}"
                );
                source_errors.push(error.clone());
                latch_capture_degradation(&mut degradation, error);
                None
            }
        }
    } else {
        None
    };
    let mut next_backing = 0u64;
    let captured_artifacts = match import_storage.as_ref() {
        Some(storage) => {
            let (artifacts, capture_read_error) = validate_captured(
                captured,
                storage.path(),
                &mut next_backing,
                &mut validation,
                reporter,
                journal,
            )?;
            if let Some(error) = capture_read_error {
                latch_capture_degradation(&mut degradation, error);
            }
            artifacts
        }
        None => captured.len(),
    };

    let (crash_artifacts, corpus_artifacts) =
        match (degradation.as_deref(), import_storage.as_ref()) {
            (Some(_), Some(storage)) => {
                let (crashes, errors) = validate_readable_fallback(
                    crashes_dir,
                    not_findings,
                    storage.path(),
                    &mut next_backing,
                    &mut validation,
                    reporter,
                    journal,
                )?;
                source_errors.extend(errors);
                let corpus_artifacts = match corpus_dir {
                    Some(dir) => {
                        let (batch, errors) = validate_readable_fallback(
                            dir,
                            &[],
                            storage.path(),
                            &mut next_backing,
                            &mut validation,
                            reporter,
                            journal,
                        )?;
                        source_errors.extend(errors);
                        batch
                    }
                    None => 0,
                };
                (crashes, corpus_artifacts)
            }
            _ => (0, 0),
        };
    let artifacts = captured_artifacts + crash_artifacts + corpus_artifacts;
    let canonical = validation.into_canonical();
    let unique = canonical.len();

    let mut verdicts: BTreeMap<Verdict, usize> = BTreeMap::new();
    let mut filed = BTreeSet::new();
    let mut retained = RetainedArtifacts::default();
    let mut post_failed = 0usize;
    let mut nonbugs = 0usize;
    for (index, artifact) in canonical.into_iter().enumerate() {
        let artifact = match artifact.materialize() {
            Ok(artifact) => artifact,
            Err(error) => {
                let error = format!(
                    "validated external artifact became unavailable before replay: {error}"
                );
                source_errors.push(error.clone());
                latch_capture_degradation(&mut degradation, error);
                reporter.emit(|| RunEvent::ImportProgress {
                    replayed: index + 1,
                    total: unique,
                });
                continue;
            }
        };
        let measured = replay.measure(artifact.bytes()).unwrap_or_else(|error| {
            reporter.emit(|| {
                RunEvent::Warning(Warning::ReplayFailed {
                    message: error.to_string(),
                })
            });
            Measured {
                verdict: Verdict::HarnessError,
                cover: None,
                post: false,
            }
        });
        let Some(classified) = artifact.classify(&measured) else {
            *verdicts.entry(Verdict::HarnessError).or_default() += 1;
            reporter.emit(|| RunEvent::ImportProgress {
                replayed: index + 1,
                total: unique,
            });
            continue;
        };
        *verdicts.entry(classified.outcome().verdict()).or_default() += 1;

        let (elapsed_ms, timing) = retained_timing(&artifact, journal.started());
        let path = Store::promote_outcome(
            folders,
            &classified,
            journal,
            elapsed_ms,
            crate::coverage::Provenance::Tool,
            timing,
        )?;
        let outcome = classified.outcome();
        retained.insert(RetainedArtifact::new(outcome, path.clone()));
        if outcome.is_crash_finding() {
            filed.insert(path);
            if outcome == ArtifactOutcome::PostconditionFailing {
                post_failed += 1;
            }
        } else {
            nonbugs += 1;
        }
        reporter.emit(|| RunEvent::ImportProgress {
            replayed: index + 1,
            total: unique,
        });
    }

    if let Some(error) = degradation.as_deref() {
        let message = if source_errors.is_empty() {
            format!(
                "periodic external corpus capture was incomplete ({error}); readable inputs from \
                 the final tool-owned directories were imported as a fallback, but \
                 coverage-over-time input chronology is incomplete"
            )
        } else {
            format!(
                "periodic external corpus capture was incomplete ({error}); readable inputs from \
                 the final tool-owned directories were imported as a fallback, but some final \
                 sources were unavailable ({}: {}) and coverage-over-time input \
                 chronology is incomplete",
                source_errors.len(),
                example_list(&source_errors)
            )
        };
        reporter.emit(|| RunEvent::Warning(Warning::CoverageTimingDegraded { message }));
    }

    drop(import_storage);
    Ok((
        Crashes {
            artifacts,
            unique,
            verdicts,
            filed,
            post_failed,
        },
        Corpus {
            artifacts: nonbugs,
            unique: nonbugs,
            dir: folders.corpus_root().to_path_buf(),
        },
        retained,
        degradation,
    ))
}

fn mkdir(path: &Path) -> Result<(), FindingsError> {
    std::fs::create_dir_all(path).map_err(|source| FindingsError::Io {
        path: path.to_path_buf(),
        source,
    })
}
