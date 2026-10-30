use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::observe::{Reporter, RunEvent};
use serde::{Deserialize, Serialize};

use crate::coverage::Journal;

use super::{ArtifactOutcome, FindingsError, Measured, digest};

pub(crate) const VALIDATION_CHUNK: usize = 64;

pub(crate) fn validate_in_chunks<C, T>(
    candidates: Vec<C>,
    reporter: &Reporter,
    mut step: impl FnMut(Vec<C>) -> Result<Vec<T>, FindingsError>,
) -> Result<Vec<T>, FindingsError> {
    let total = candidates.len();
    if total == 0 {
        return Ok(Vec::new());
    }
    reporter.emit(|| RunEvent::ValidationProgress {
        validated: 0,
        total,
    });
    let mut source = candidates.into_iter();
    let mut kept = Vec::new();
    let mut validated = 0usize;
    loop {
        let chunk: Vec<C> = source.by_ref().take(VALIDATION_CHUNK).collect();
        if chunk.is_empty() {
            return Ok(kept);
        }
        validated += chunk.len();
        kept.extend(step(chunk)?);
        reporter.emit(|| RunEvent::ValidationProgress { validated, total });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ParseValidation {
    Passed,
    Rejected,
    NoFunction,
    InternalError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PreconditionValidation {
    Passed,
    Failed,
    Unsupported,
    InternalError,
    NotRun,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationEvidence {
    pub parse: ParseValidation,
    pub precondition: PreconditionValidation,
    pub function_id: Option<u8>,
    pub function: Option<String>,
    pub test_case_size: Option<u64>,
    pub trust: String,
    pub pointers: String,
    pub byte_use: ByteUse,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, serde_json::Value>,
}

pub use crate::wire::ByteUse;

#[derive(Debug, Clone)]
pub struct HarnessValidator {
    harness: PathBuf,
    env: Vec<(String, String)>,
}

impl HarnessValidator {
    pub fn new(harness: impl Into<PathBuf>, env: Vec<(String, String)>) -> HarnessValidator {
        HarnessValidator {
            harness: harness.into(),
            env,
        }
    }

    pub fn harness(&self) -> &Path {
        &self.harness
    }

    pub fn env(&self) -> &[(String, String)] {
        &self.env
    }
}

impl ArtifactValidator for HarnessValidator {
    fn validate(&self, _source: &Path, bytes: &[u8]) -> Result<ValidationEvidence, FindingsError> {
        let inputs = [bytes];
        let mut observations = crate::wire::validate_candidates(&self.harness, &self.env, &inputs)
            .map_err(|error| FindingsError::Validation {
                message: error.to_string(),
            })?;
        let observation = observations
            .pop()
            .ok_or_else(|| FindingsError::Validation {
                message: "validation harness returned no observation".to_string(),
            })?;
        Ok(evidence_from_observation(observation))
    }

    fn validate_batch(
        &self,
        candidates: &[(&Path, &[u8])],
    ) -> Result<Vec<ValidationEvidence>, FindingsError> {
        let inputs: Vec<&[u8]> = candidates.iter().map(|(_, bytes)| *bytes).collect();
        crate::wire::validate_candidates(&self.harness, &self.env, &inputs)
            .map_err(|error| FindingsError::Validation {
                message: error.to_string(),
            })
            .map(|observations| {
                observations
                    .into_iter()
                    .map(evidence_from_observation)
                    .collect()
            })
    }
}

fn evidence_from_observation(
    observation: crate::wire::ValidationObservation,
) -> ValidationEvidence {
    use crate::wire::{ValidationParse as Parse, ValidationPrecondition as Precondition};

    let mut details = BTreeMap::new();
    let parse = match observation.parse {
        Parse::Passed => ParseValidation::Passed,
        Parse::Rejected { failure } => {
            details.insert("parse".to_string(), serde_json::json!(failure));
            ParseValidation::Rejected
        }
        Parse::NoFunction => ParseValidation::NoFunction,
        Parse::InternalError { failure } => {
            details.insert("parse".to_string(), serde_json::json!(failure));
            ParseValidation::InternalError
        }
    };
    let precondition = match observation.precondition {
        Precondition::Passed => PreconditionValidation::Passed,
        Precondition::Failed { failure } => {
            details.insert("precondition".to_string(), serde_json::json!(failure));
            PreconditionValidation::Failed
        }
        Precondition::Unsupported { failure } => {
            details.insert("precondition".to_string(), serde_json::json!(failure));
            PreconditionValidation::Unsupported
        }
        Precondition::InternalError { failure } => {
            details.insert("precondition".to_string(), serde_json::json!(failure));
            PreconditionValidation::InternalError
        }
        Precondition::NotRun => PreconditionValidation::NotRun,
    };
    ValidationEvidence {
        parse,
        precondition,
        function_id: observation.function_id,
        function: observation.function,
        test_case_size: observation.test_case_size,
        trust: observation.trust,
        pointers: observation.pointers,
        byte_use: observation.byte_use,
        details,
    }
}

impl ValidationEvidence {
    pub fn passed(&self) -> bool {
        self.parse == ParseValidation::Passed && self.precondition == PreconditionValidation::Passed
    }

    pub fn eligible(&self) -> bool {
        if self.trust != crate::wire::Trust::Checked.name() {
            return false;
        }
        let ordinary = self.parse == ParseValidation::Passed
            && matches!(
                self.precondition,
                PreconditionValidation::Passed | PreconditionValidation::Failed
            );
        let recovered_false_constraint = self.parse == ParseValidation::Rejected
            && self.precondition == PreconditionValidation::Failed
            && self.details.get("parse").is_some_and(|failure| {
                failure.get("kind").and_then(serde_json::Value::as_str)
                    == Some(crate::wire::CONSTRAINT_FALSE_KIND)
            });
        ordinary || recovered_false_constraint
    }

    pub fn launch_eligible(&self) -> bool {
        if self.trust != crate::wire::Trust::Checked.name() {
            return false;
        }
        match self.parse {
            ParseValidation::Passed => matches!(
                self.precondition,
                PreconditionValidation::Passed
                    | PreconditionValidation::Failed
                    | PreconditionValidation::Unsupported
            ),
            ParseValidation::Rejected | ParseValidation::NoFunction => {
                self.precondition != PreconditionValidation::InternalError
            }
            ParseValidation::InternalError => false,
        }
    }
}

pub trait ArtifactValidator: Send + Sync {
    fn validate(&self, source: &Path, bytes: &[u8]) -> Result<ValidationEvidence, FindingsError>;

    fn validate_batch(
        &self,
        candidates: &[(&Path, &[u8])],
    ) -> Result<Vec<ValidationEvidence>, FindingsError> {
        candidates
            .iter()
            .map(|(source, bytes)| self.validate(source, bytes))
            .collect()
    }
}

impl<F> ArtifactValidator for F
where
    F: Fn(&Path, &[u8]) -> Result<ValidationEvidence, FindingsError> + Send + Sync,
{
    fn validate(&self, source: &Path, bytes: &[u8]) -> Result<ValidationEvidence, FindingsError> {
        self(source, bytes)
    }
}

#[derive(Debug)]
pub struct ValidatedArtifact {
    source: PathBuf,
    bytes: Vec<u8>,
    digest: u64,
    evidence: ValidationEvidence,
    source_mtime: Option<std::time::SystemTime>,
    observed_ms: Option<u64>,
}

impl ValidatedArtifact {
    pub fn source(&self) -> &Path {
        &self.source
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn digest(&self) -> u64 {
        self.digest
    }

    pub fn evidence(&self) -> &ValidationEvidence {
        &self.evidence
    }

    pub fn classify<'a>(&'a self, measured: &Measured) -> Option<ClassifiedArtifact<'a>> {
        let outcome = ArtifactOutcome::classify(&self.evidence, measured)?;
        Some(ClassifiedArtifact {
            artifact: self,
            outcome,
        })
    }

    pub(crate) fn source_mtime(&self) -> Option<std::time::SystemTime> {
        self.source_mtime
    }

    pub(crate) fn observed_ms(&self) -> Option<u64> {
        self.observed_ms
    }
}

#[derive(Debug)]
pub struct ClassifiedArtifact<'a> {
    artifact: &'a ValidatedArtifact,
    outcome: ArtifactOutcome,
}

impl ClassifiedArtifact<'_> {
    pub fn source(&self) -> &Path {
        self.artifact.source()
    }

    pub fn bytes(&self) -> &[u8] {
        self.artifact.bytes()
    }

    pub fn digest(&self) -> u64 {
        self.artifact.digest()
    }

    pub fn evidence(&self) -> &ValidationEvidence {
        self.artifact.evidence()
    }

    pub fn outcome(&self) -> ArtifactOutcome {
        self.outcome
    }

    pub(crate) fn validated(&self) -> &ValidatedArtifact {
        self.artifact
    }
}

#[derive(Debug, Serialize)]
struct ValidationRow<'a> {
    source: String,
    digest: String,
    bytes: usize,
    #[serde(flatten)]
    evidence: &'a ValidationEvidence,
}

#[derive(Debug, Serialize)]
struct ValidationDispositionRow {
    source: String,
    digest: String,
    disposition: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    canonical_source: Option<String>,
}

#[derive(Debug)]
struct CanonicalBytes {
    source: PathBuf,
    bytes: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
enum ByteDeduplication {
    Canonical,
    Duplicate,
}

#[derive(Debug, Default)]
struct ByteDeduplicator {
    buckets: HashMap<u64, Vec<CanonicalBytes>>,
}

impl ByteDeduplicator {
    fn consider(
        &mut self,
        source: &Path,
        bytes: &[u8],
        digest: u64,
        canonical_disposition: &'static str,
        journal: &mut Journal,
    ) -> Result<ByteDeduplication, FindingsError> {
        let bucket = self.buckets.entry(digest).or_default();
        if let Some(previous) = bucket.iter().find(|candidate| candidate.bytes == bytes) {
            journal_disposition(source, digest, "duplicate", Some(&previous.source), journal)?;
            return Ok(ByteDeduplication::Duplicate);
        }

        journal_disposition(source, digest, canonical_disposition, None, journal)?;
        bucket.push(CanonicalBytes {
            source: source.to_path_buf(),
            bytes: bytes.to_vec(),
        });
        Ok(ByteDeduplication::Canonical)
    }
}

fn checked_batch(
    validator: &dyn ArtifactValidator,
    candidates: &[(&Path, &[u8])],
) -> Result<Vec<ValidationEvidence>, FindingsError> {
    let evidence = validator.validate_batch(candidates)?;
    if evidence.len() != candidates.len() {
        return Err(FindingsError::Validation {
            message: format!(
                "validator returned {} observations for {} candidates",
                evidence.len(),
                candidates.len()
            ),
        });
    }
    Ok(evidence)
}

fn journal_observation(
    source: &Path,
    bytes: &[u8],
    evidence: &ValidationEvidence,
    journal: &mut Journal,
) -> Result<u64, FindingsError> {
    let digest = digest(bytes);
    journal.required_json(
        "validation",
        &ValidationRow {
            source: source.to_string_lossy().into_owned(),
            digest: format!("{digest:016x}"),
            bytes: bytes.len(),
            evidence,
        },
    )?;
    Ok(digest)
}

fn journal_ineligible(
    source: &Path,
    digest: u64,
    journal: &mut Journal,
) -> Result<(), FindingsError> {
    journal_disposition(source, digest, "ineligible", None, journal)
}

fn journal_disposition(
    source: &Path,
    digest: u64,
    disposition: &'static str,
    canonical_source: Option<&Path>,
    journal: &mut Journal,
) -> Result<(), FindingsError> {
    journal.required_json(
        "validation-disposition",
        &ValidationDispositionRow {
            source: source.to_string_lossy().into_owned(),
            digest: format!("{digest:016x}"),
            disposition,
            canonical_source: canonical_source.map(|p| p.to_string_lossy().into_owned()),
        },
    )
}

fn file_holds_bytes(path: &Path, bytes: &[u8]) -> Result<bool, FindingsError> {
    super::store::file_holds(path, bytes).map_err(|source| FindingsError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[derive(Debug)]
struct DiskCanonicalBytes {
    source: PathBuf,
    storage: PathBuf,
}

#[derive(Debug, Default)]
struct DiskByteDeduplicator {
    buckets: HashMap<u64, Vec<DiskCanonicalBytes>>,
}

impl DiskByteDeduplicator {
    fn consider(
        &mut self,
        source: &Path,
        storage: &Path,
        bytes: &[u8],
        digest: u64,
        journal: &mut Journal,
    ) -> Result<ByteDeduplication, FindingsError> {
        let bucket = self.buckets.entry(digest).or_default();
        for previous in bucket.iter() {
            if file_holds_bytes(&previous.storage, bytes)? {
                journal_disposition(source, digest, "duplicate", Some(&previous.source), journal)?;
                return Ok(ByteDeduplication::Duplicate);
            }
        }

        journal_disposition(source, digest, "canonical", None, journal)?;
        bucket.push(DiskCanonicalBytes {
            source: source.to_path_buf(),
            storage: storage.to_path_buf(),
        });
        Ok(ByteDeduplication::Canonical)
    }
}

pub struct ValidationSession<'a> {
    validator: &'a dyn ArtifactValidator,
    dedup: ByteDeduplicator,
}

impl<'a> ValidationSession<'a> {
    pub fn new(validator: &'a dyn ArtifactValidator) -> ValidationSession<'a> {
        ValidationSession {
            validator,
            dedup: ByteDeduplicator::default(),
        }
    }

    pub fn validate(
        &mut self,
        source: impl Into<PathBuf>,
        bytes: Vec<u8>,
        source_mtime: Option<std::time::SystemTime>,
        journal: &mut Journal,
    ) -> Result<Option<ValidatedArtifact>, FindingsError> {
        let source = source.into();
        let evidence = self.validator.validate(&source, &bytes)?;
        self.finish(source, bytes, source_mtime, None, evidence, journal)
    }

    pub fn validate_batch(
        &mut self,
        candidates: Vec<(PathBuf, Vec<u8>, Option<std::time::SystemTime>)>,
        journal: &mut Journal,
    ) -> Result<Vec<ValidatedArtifact>, FindingsError> {
        let borrowed: Vec<(&Path, &[u8])> = candidates
            .iter()
            .map(|(source, bytes, _)| (source.as_path(), bytes.as_slice()))
            .collect();
        let evidence = checked_batch(self.validator, &borrowed)?;
        let mut canonical = Vec::new();
        for ((source, bytes, source_mtime), evidence) in candidates.into_iter().zip(evidence) {
            if let Some(artifact) =
                self.finish(source, bytes, source_mtime, None, evidence, journal)?
            {
                canonical.push(artifact);
            }
        }
        Ok(canonical)
    }

    fn finish(
        &mut self,
        source: PathBuf,
        bytes: Vec<u8>,
        source_mtime: Option<std::time::SystemTime>,
        observed_ms: Option<u64>,
        evidence: ValidationEvidence,
        journal: &mut Journal,
    ) -> Result<Option<ValidatedArtifact>, FindingsError> {
        let digest = journal_observation(&source, &bytes, &evidence, journal)?;

        if !evidence.eligible() {
            journal_ineligible(&source, digest, journal)?;
            return Ok(None);
        }

        if self
            .dedup
            .consider(&source, &bytes, digest, "canonical", journal)?
            == ByteDeduplication::Duplicate
        {
            return Ok(None);
        }
        Ok(Some(ValidatedArtifact {
            source,
            bytes,
            digest,
            evidence,
            source_mtime,
            observed_ms,
        }))
    }
}

#[derive(Debug)]
pub(crate) struct DiskValidatedArtifact {
    source: PathBuf,
    storage: PathBuf,
    bytes: usize,
    digest: u64,
    evidence: ValidationEvidence,
    source_mtime: Option<std::time::SystemTime>,
    observed_ms: Option<u64>,
}

impl DiskValidatedArtifact {
    pub(crate) fn source(&self) -> &Path {
        &self.source
    }

    fn verified_bytes(&self) -> Result<Vec<u8>, FindingsError> {
        let (_, bytes) = read_candidate_bytes(self.storage.clone())?;
        let observed_digest = digest(&bytes);
        if bytes.len() != self.bytes || observed_digest != self.digest {
            return Err(FindingsError::Validation {
                message: format!(
                    "validated source {} stored at {} changed before replay (expected {} \
                     bytes/{:016x}, found {} bytes/{:016x})",
                    self.source.display(),
                    self.storage.display(),
                    self.bytes,
                    self.digest,
                    bytes.len(),
                    observed_digest,
                ),
            });
        }
        Ok(bytes)
    }

    pub(crate) fn materialize(self) -> Result<ValidatedArtifact, FindingsError> {
        let bytes = self.verified_bytes()?;
        let DiskValidatedArtifact {
            source,
            storage: _,
            bytes: _,
            digest,
            evidence,
            source_mtime,
            observed_ms,
        } = self;
        Ok(ValidatedArtifact {
            source,
            bytes,
            digest,
            evidence,
            source_mtime,
            observed_ms,
        })
    }
}

pub(crate) struct DiskValidationSession<'a> {
    validator: &'a dyn ArtifactValidator,
    dedup: DiskByteDeduplicator,
    canonical: Vec<DiskValidatedArtifact>,
}

struct DiskCandidate {
    source: PathBuf,
    storage: PathBuf,
    bytes: Vec<u8>,
    source_mtime: Option<std::time::SystemTime>,
    observed_ms: Option<u64>,
}

impl<'a> DiskValidationSession<'a> {
    pub(crate) fn new(validator: &'a dyn ArtifactValidator) -> DiskValidationSession<'a> {
        DiskValidationSession {
            validator,
            dedup: DiskByteDeduplicator::default(),
            canonical: Vec::new(),
        }
    }

    pub(crate) fn validate_batch(
        &mut self,
        candidates: Vec<(PathBuf, Vec<u8>, Option<std::time::SystemTime>)>,
        journal: &mut Journal,
    ) -> Result<(), FindingsError> {
        let candidates = candidates
            .into_iter()
            .map(|(source, bytes, source_mtime)| {
                let storage = source.clone();
                DiskCandidate {
                    source,
                    storage,
                    bytes,
                    source_mtime,
                    observed_ms: None,
                }
            })
            .collect();
        self.validate_stored_batch(candidates, journal)
    }

    pub(crate) fn validate_backed_batch(
        &mut self,
        candidates: Vec<(PathBuf, PathBuf, Vec<u8>, Option<std::time::SystemTime>)>,
        journal: &mut Journal,
    ) -> Result<(), FindingsError> {
        let candidates = candidates
            .into_iter()
            .map(|(source, storage, bytes, source_mtime)| DiskCandidate {
                source,
                storage,
                bytes,
                source_mtime,
                observed_ms: None,
            })
            .collect();
        self.validate_stored_batch(candidates, journal)
    }

    pub(crate) fn validate_observed_batch(
        &mut self,
        candidates: Vec<(PathBuf, PathBuf, Vec<u8>, u64)>,
        journal: &mut Journal,
    ) -> Result<(), FindingsError> {
        let candidates = candidates
            .into_iter()
            .map(|(source, storage, bytes, observed_ms)| DiskCandidate {
                source,
                storage,
                bytes,
                source_mtime: None,
                observed_ms: Some(observed_ms),
            })
            .collect();
        self.validate_stored_batch(candidates, journal)
    }

    fn validate_stored_batch(
        &mut self,
        candidates: Vec<DiskCandidate>,
        journal: &mut Journal,
    ) -> Result<(), FindingsError> {
        let borrowed: Vec<(&Path, &[u8])> = candidates
            .iter()
            .map(|candidate| (candidate.source.as_path(), candidate.bytes.as_slice()))
            .collect();
        let evidence = checked_batch(self.validator, &borrowed)?;
        for (candidate, evidence) in candidates.into_iter().zip(evidence) {
            self.finish(candidate, evidence, journal)?;
        }
        Ok(())
    }

    fn finish(
        &mut self,
        candidate: DiskCandidate,
        evidence: ValidationEvidence,
        journal: &mut Journal,
    ) -> Result<(), FindingsError> {
        let DiskCandidate {
            source,
            storage,
            bytes,
            source_mtime,
            observed_ms,
        } = candidate;
        let digest = journal_observation(&source, &bytes, &evidence, journal)?;
        if !evidence.eligible() {
            journal_ineligible(&source, digest, journal)?;
            return Ok(());
        }
        if self
            .dedup
            .consider(&source, &storage, &bytes, digest, journal)?
            == ByteDeduplication::Duplicate
        {
            return Ok(());
        }

        self.canonical.push(DiskValidatedArtifact {
            source,
            storage,
            bytes: bytes.len(),
            digest,
            evidence,
            source_mtime,
            observed_ms,
        });
        Ok(())
    }

    pub(crate) fn into_canonical(self) -> Vec<DiskValidatedArtifact> {
        self.canonical
    }
}

#[derive(Debug)]
pub struct LaunchCandidate {
    source: PathBuf,
}

impl LaunchCandidate {
    pub fn source(&self) -> &Path {
        &self.source
    }
}

pub struct LaunchValidationSession<'a> {
    validator: &'a dyn ArtifactValidator,
    dedup: ByteDeduplicator,
}

impl<'a> LaunchValidationSession<'a> {
    pub fn new(validator: &'a dyn ArtifactValidator) -> LaunchValidationSession<'a> {
        LaunchValidationSession {
            validator,
            dedup: ByteDeduplicator::default(),
        }
    }

    pub fn validate_batch(
        &mut self,
        candidates: Vec<(PathBuf, Vec<u8>)>,
        journal: &mut Journal,
    ) -> Result<Vec<LaunchCandidate>, FindingsError> {
        let borrowed: Vec<(&Path, &[u8])> = candidates
            .iter()
            .map(|(source, bytes)| (source.as_path(), bytes.as_slice()))
            .collect();
        let evidence = checked_batch(self.validator, &borrowed)?;
        let mut canonical = Vec::new();
        for ((source, bytes), evidence) in candidates.into_iter().zip(evidence) {
            if let Some(candidate) = self.finish(source, bytes, evidence, journal)? {
                canonical.push(candidate);
            }
        }
        Ok(canonical)
    }

    fn finish(
        &mut self,
        source: PathBuf,
        bytes: Vec<u8>,
        evidence: ValidationEvidence,
        journal: &mut Journal,
    ) -> Result<Option<LaunchCandidate>, FindingsError> {
        let digest = journal_observation(&source, &bytes, &evidence, journal)?;

        if !evidence.launch_eligible() {
            journal_ineligible(&source, digest, journal)?;
            return Ok(None);
        }

        let disposition = if evidence.eligible() {
            "canonical"
        } else {
            "launch-only"
        };
        match self
            .dedup
            .consider(&source, &bytes, digest, disposition, journal)?
        {
            ByteDeduplication::Canonical => Ok(Some(LaunchCandidate { source })),
            ByteDeduplication::Duplicate => Ok(None),
        }
    }
}

fn candidate_paths(dir: &Path, skip: &[&str]) -> Result<Vec<PathBuf>, FindingsError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(FindingsError::Io {
                path: dir.to_path_buf(),
                source,
            });
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| FindingsError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        if skip.iter().any(|skip| entry.file_name() == **skip) {
            continue;
        }

        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| FindingsError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_file() {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn read_candidate_bytes(path: PathBuf) -> Result<(PathBuf, Vec<u8>), FindingsError> {
    let mut file = std::fs::File::open(&path).map_err(|source| FindingsError::Io {
        path: path.clone(),
        source,
    })?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes).map_err(|source| FindingsError::Io {
        path: path.clone(),
        source,
    })?;
    Ok((path, bytes))
}

pub(crate) fn read_captured_candidate(path: &Path) -> Result<Vec<u8>, FindingsError> {
    let mut file = std::fs::File::open(path).map_err(|source| FindingsError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let metadata = file.metadata().map_err(|source| FindingsError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() > crate::wire::EXTERNAL_INPUT_BYTES as u64 {
        return Err(oversized_input_error(path, "captured"));
    }
    let limit = u64::try_from(crate::wire::EXTERNAL_INPUT_BYTES)
        .expect("the supported testcase size fits in u64")
        + 1;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(&mut file, limit), &mut bytes).map_err(
        |source| FindingsError::Io {
            path: path.to_path_buf(),
            source,
        },
    )?;
    if bytes.len() > crate::wire::EXTERNAL_INPUT_BYTES {
        return Err(oversized_input_error(path, "captured"));
    }
    Ok(bytes)
}

pub(crate) fn retain_captured_candidate(
    source: &Path,
    capture: &Path,
    elapsed_ms: u64,
    backing_dir: &Path,
    next_sequence: &mut u64,
) -> Result<(PathBuf, PathBuf, Vec<u8>, u64), FindingsError> {
    let bytes = read_captured_candidate(capture)?;
    let storage = retain_external_bytes(&bytes, backing_dir, next_sequence)?;
    Ok((source.to_path_buf(), storage, bytes, elapsed_ms))
}

fn oversized_input_error(path: &Path, population: &str) -> FindingsError {
    FindingsError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{population} candidate exceeds the supported testcase limit of {} bytes",
                crate::wire::EXTERNAL_INPUT_BYTES
            ),
        ),
    }
}

fn read_candidate(
    path: PathBuf,
) -> Result<(PathBuf, Vec<u8>, Option<std::time::SystemTime>), FindingsError> {
    let mut file = std::fs::File::open(&path).map_err(|source| FindingsError::Io {
        path: path.clone(),
        source,
    })?;
    let metadata = file.metadata().map_err(|source| FindingsError::Io {
        path: path.clone(),
        source,
    })?;
    let source_mtime = Some(metadata.modified().map_err(|source| FindingsError::Io {
        path: path.clone(),
        source,
    })?);
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes).map_err(|source| FindingsError::Io {
        path: path.clone(),
        source,
    })?;
    Ok((path, bytes, source_mtime))
}

fn read_fallback_candidate(
    path: PathBuf,
) -> Result<(PathBuf, Vec<u8>, Option<std::time::SystemTime>), FindingsError> {
    read_fallback_candidate_with(path, |file, bytes| {
        let limit = u64::try_from(crate::wire::EXTERNAL_INPUT_BYTES)
            .expect("the supported testcase size fits in u64")
            + 1;
        std::io::Read::read_to_end(&mut std::io::Read::take(file, limit), bytes).map(|_| ())
    })
}

fn read_fallback_candidate_with(
    path: PathBuf,
    read: impl FnOnce(&mut std::fs::File, &mut Vec<u8>) -> std::io::Result<()>,
) -> Result<(PathBuf, Vec<u8>, Option<std::time::SystemTime>), FindingsError> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(&path).map_err(|source| FindingsError::Io {
        path: path.clone(),
        source,
    })?;
    let metadata = file.metadata().map_err(|source| FindingsError::Io {
        path: path.clone(),
        source,
    })?;
    if !metadata.file_type().is_file() {
        return Err(FindingsError::Io {
            path,
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "fallback candidates must remain regular files; symlinks are not followed",
            ),
        });
    }
    let before_len = metadata.len();
    if before_len > crate::wire::EXTERNAL_INPUT_BYTES as u64 {
        return Err(oversized_input_error(&path, "fallback"));
    }
    let before_modified = metadata.modified().ok();
    let mut bytes = Vec::new();
    read(&mut file, &mut bytes).map_err(|source| FindingsError::Io {
        path: path.clone(),
        source,
    })?;
    if bytes.len() > crate::wire::EXTERNAL_INPUT_BYTES {
        return Err(oversized_input_error(&path, "fallback"));
    }
    let after = file.metadata().map_err(|source| FindingsError::Io {
        path: path.clone(),
        source,
    })?;
    let after_modified = after.modified().ok();
    if before_len != after.len()
        || after.len() != bytes.len() as u64
        || before_modified != after_modified
    {
        return Err(FindingsError::Io {
            path,
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the fallback candidate changed while it was being read",
            ),
        });
    }
    Ok((path, bytes, after_modified))
}

fn retain_fallback_candidate(
    path: PathBuf,
    backing_dir: &Path,
    next_sequence: &mut u64,
) -> Result<(PathBuf, PathBuf, Vec<u8>, Option<std::time::SystemTime>), FindingsError> {
    let (source, bytes, source_mtime) = read_fallback_candidate(path)?;
    let storage = retain_external_bytes(&bytes, backing_dir, next_sequence)?;
    Ok((source, storage, bytes, source_mtime))
}

fn retain_external_bytes(
    bytes: &[u8],
    backing_dir: &Path,
    next_sequence: &mut u64,
) -> Result<PathBuf, FindingsError> {
    let sequence = *next_sequence;
    *next_sequence = next_sequence
        .checked_add(1)
        .ok_or_else(|| FindingsError::Validation {
            message: "too many external import candidates to retain".to_string(),
        })?;
    let storage = backing_dir.join(format!("{sequence:020}"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&storage)
        .map_err(|source| FindingsError::Io {
            path: storage.clone(),
            source,
        })?;
    if let Err(source) = std::io::Write::write_all(&mut file, bytes) {
        drop(file);
        let _ = std::fs::remove_file(&storage);
        return Err(FindingsError::Io {
            path: storage,
            source,
        });
    }
    Ok(storage)
}

pub(crate) fn validate_readable_fallback(
    dir: &Path,
    skip: &[&str],
    backing_dir: &Path,
    next_sequence: &mut u64,
    session: &mut DiskValidationSession<'_>,
    reporter: &Reporter,
    journal: &mut Journal,
) -> Result<(usize, Vec<String>), FindingsError> {
    let (paths, mut source_errors) = readable_fallback_paths(dir, skip);
    let mut artifacts = 0usize;
    validate_in_chunks(paths, reporter, |chunk| {
        let mut candidates = Vec::with_capacity(chunk.len());
        for path in chunk {
            match retain_fallback_candidate(path, backing_dir, next_sequence) {
                Ok(candidate) => candidates.push(candidate),
                Err(error) => source_errors.push(error.to_string()),
            }
        }
        artifacts += candidates.len();
        if candidates.is_empty() {
            Ok(Vec::<()>::new())
        } else {
            session.validate_backed_batch(candidates, journal)?;
            Ok(Vec::<()>::new())
        }
    })?;
    Ok((artifacts, source_errors))
}

fn readable_fallback_paths(dir: &Path, skip: &[&str]) -> (Vec<PathBuf>, Vec<String>) {
    match std::fs::symlink_metadata(dir) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            let error = FindingsError::Io {
                path: dir.to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "the final fallback root must be a directory; symlinks are not followed",
                ),
            };
            return (Vec::new(), vec![error.to_string()]);
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return (Vec::new(), Vec::new());
        }
        Err(source) => {
            let error = FindingsError::Io {
                path: dir.to_path_buf(),
                source,
            };
            return (Vec::new(), vec![error.to_string()]);
        }
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return (Vec::new(), Vec::new());
        }
        Err(source) => {
            let error = FindingsError::Io {
                path: dir.to_path_buf(),
                source,
            };
            return (Vec::new(), vec![error.to_string()]);
        }
    };
    let mut paths = Vec::new();
    let mut errors = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(source) => {
                errors.push(
                    FindingsError::Io {
                        path: dir.to_path_buf(),
                        source,
                    }
                    .to_string(),
                );
                continue;
            }
        };
        if skip.iter().any(|skip| entry.file_name() == **skip) {
            continue;
        }
        let path = entry.path();
        match entry.file_type() {
            Ok(file_type) if file_type.is_file() => paths.push(path),
            Ok(_) => {}
            Err(source) => errors.push(FindingsError::Io { path, source }.to_string()),
        }
    }
    paths.sort();
    (paths, errors)
}

pub(crate) fn validate_all_disk_backed(
    dir: &Path,
    validator: &dyn ArtifactValidator,
    reporter: &Reporter,
    journal: &mut Journal,
) -> Result<Vec<DiskValidatedArtifact>, FindingsError> {
    let paths = candidate_paths(dir, &[])?;
    let mut session = DiskValidationSession::new(validator);
    validate_in_chunks(paths, reporter, |chunk| {
        let candidates = chunk
            .into_iter()
            .map(read_candidate)
            .collect::<Result<Vec<_>, _>>()?;
        session.validate_batch(candidates, journal)?;
        Ok(Vec::<()>::new())
    })?;
    Ok(session.into_canonical())
}
