use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::wire::{AustenInput, TestCaseSize};

#[derive(Debug, Clone)]
pub struct SeedCandidate {
    source: PathBuf,
    bytes: Vec<u8>,
    modified: Option<SystemTime>,
}

impl SeedCandidate {
    pub fn new(source: PathBuf, bytes: Vec<u8>, modified: Option<SystemTime>) -> Self {
        Self {
            source,
            bytes,
            modified,
        }
    }

    pub fn source(&self) -> &Path {
        &self.source
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn modified(&self) -> Option<SystemTime> {
        self.modified
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct CandidateGateError {
    message: String,
}

impl CandidateGateError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub trait CandidateGate {
    fn validate(&self, candidates: &[SeedCandidate]) -> Result<Vec<usize>, CandidateGateError>;

    fn promoted(&self, _seeds: &[StagedSeed]) -> Result<(), CandidateGateError> {
        Ok(())
    }
}

impl<F> CandidateGate for F
where
    F: Fn(&[SeedCandidate]) -> Result<Vec<usize>, CandidateGateError>,
{
    fn validate(&self, candidates: &[SeedCandidate]) -> Result<Vec<usize>, CandidateGateError> {
        self(candidates)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedSeed {
    pub path: PathBuf,
    pub bytes: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum LaunchSeedError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("launch seed validation failed: {0}")]
    CandidateGate(#[from] CandidateGateError),
}

fn io(path: impl Into<PathBuf>, source: std::io::Error) -> LaunchSeedError {
    LaunchSeedError::Io {
        path: path.into(),
        source,
    }
}

fn null_payload() -> [u8; 64] {
    [0u8; 64]
}

pub const FUNCTION_SEED_LEN: usize = crate::wire::INPUT_HEADER_BYTES + 64;

const FUNCTION_SEED_PREFIX: &str = "austen-function-seed-";

fn function_seed_name(target_id: u8) -> String {
    format!("{FUNCTION_SEED_PREFIX}{target_id:02x}")
}

fn read_seed_source_candidates(
    dir: &Path,
    candidates: &mut Vec<SeedCandidate>,
) -> Result<(), LaunchSeedError> {
    let metadata = match std::fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io(dir, source)),
    };
    if !metadata.file_type().is_dir() {
        return Err(io(
            dir,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the seed source is not a directory",
            ),
        ));
    }

    let mut entries = std::fs::read_dir(dir)
        .map_err(|source| io(dir, source))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| io(dir, source))?;
    entries.sort_by_key(std::fs::DirEntry::path);

    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| io(&path, source))?;
        if file_type.is_dir() {
            read_seed_source_candidates(&path, candidates)?;
            continue;
        }
        if !file_type.is_file() {
            return Err(io(
                path,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "seed source entries must be regular files; symlinks are not followed",
                ),
            ));
        }
        let bytes = std::fs::read(&path).map_err(|source| io(&path, source))?;
        let modified = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        candidates.push(SeedCandidate::new(path, bytes, modified));
    }
    Ok(())
}

fn add_function_seed_candidates(
    work: &Path,
    function_ids: &[u8],
    candidates: &mut Vec<SeedCandidate>,
) {
    let virtual_dir = work.join(".austen-generated-function-seeds");
    let payload = null_payload();
    for &id in function_ids {
        let seed = AustenInput::from_parts(id, TestCaseSize::MAX, payload).into_bytes();
        candidates.push(SeedCandidate::new(
            virtual_dir.join(function_seed_name(id)),
            seed,
            None,
        ));
    }
}

pub fn collect_seed_candidates(
    work: &Path,
    function_ids: &[u8],
    seed_sources: &[PathBuf],
) -> Result<Vec<SeedCandidate>, LaunchSeedError> {
    let mut candidates = Vec::new();
    for (_, source) in seed_input_paths(work, seed_sources) {
        read_seed_source_candidates(&source, &mut candidates)?;
    }
    add_function_seed_candidates(work, function_ids, &mut candidates);
    Ok(candidates)
}

pub fn seed_input_paths<'a>(
    work: &Path,
    seed_sources: &'a [PathBuf],
) -> impl Iterator<Item = (&'static str, Cow<'a, Path>)> + 'a {
    std::iter::once(("user seed directory", Cow::Owned(work.join("seeds")))).chain(
        seed_sources
            .iter()
            .map(|path| ("seed source", Cow::Borrowed(path.as_path()))),
    )
}

pub fn clear_managed_dir(dir: &Path) -> Result<(), LaunchSeedError> {
    let metadata = match std::fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io(dir, source)),
    };
    if !metadata.file_type().is_dir() {
        return Err(io(
            dir,
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "the managed path to clear is not a directory",
            ),
        ));
    }
    std::fs::remove_dir_all(dir).map_err(|source| io(dir, source))
}

pub fn write_launch_seeds(
    dir: &Path,
    candidates: &[SeedCandidate],
    canonical: &[usize],
) -> Result<Vec<StagedSeed>, LaunchSeedError> {
    let mut previous = None;
    for &index in canonical {
        if index >= candidates.len() || previous.is_some_and(|earlier| earlier >= index) {
            return Err(CandidateGateError::new(
                "candidate gate returned indices that were not strictly increasing and in range",
            )
            .into());
        }
        previous = Some(index);
    }
    if canonical.is_empty() {
        return Err(CandidateGateError::new("no seed candidate completed validation").into());
    }

    std::fs::create_dir(dir).map_err(|source| io(dir, source))?;
    let mut staged = Vec::with_capacity(canonical.len());
    for (ordinal, &index) in canonical.iter().enumerate() {
        use std::io::Write as _;

        let bytes = candidates[index].bytes();
        let path = dir.join(format!("validated-seed-{ordinal:08x}"));
        let result = (|| -> std::io::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            file.write_all(bytes)
        })();
        if let Err(source) = result {
            let _ = std::fs::remove_dir_all(dir);
            return Err(io(path, source));
        }
        staged.push(StagedSeed {
            path,
            bytes: bytes.len(),
        });
    }
    Ok(staged)
}
