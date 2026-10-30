use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{ArtifactFolders, ClassifiedArtifact, FindingsError, ValidatedArtifact};

pub fn digest(bytes: &[u8]) -> u64 {
    crate::gen_dsl::fnv::fnv64(bytes)
}

pub(super) fn name_of(digest: u64) -> String {
    format!("{digest:016x}")
}

const COMPARE_CHUNK: usize = 16 * 1024;

pub fn file_holds(path: &Path, bytes: &[u8]) -> std::io::Result<bool> {
    use std::io::Read as _;

    let mut file = std::fs::File::open(path)?;
    if file.metadata()?.len() != bytes.len() as u64 {
        return Ok(false);
    }
    let mut buf = [0u8; COMPARE_CHUNK];
    let mut rest = bytes;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(rest.is_empty());
        }
        if n > rest.len() || buf[..n] != rest[..n] {
            return Ok(false);
        }
        rest = &rest[n..];
    }
}

fn store_bytes(dest: &Path, digest: u64, bytes: &[u8]) -> Result<(PathBuf, bool), FindingsError> {
    use std::io::Write as _;

    let name = name_of(digest);
    let mut suffix = 0u32;
    let mut retried_existing_race = false;
    loop {
        let path = match suffix {
            0 => dest.join(&name),
            n => dest.join(format!("{name}-{n}")),
        };
        let io_err = |source: std::io::Error| FindingsError::Io {
            path: path.clone(),
            source,
        };
        match file_holds(&path, bytes) {
            Ok(true) => return Ok((path, false)),
            Ok(false) => {
                suffix += 1;
                retried_existing_race = false;
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                let mut file = match std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                {
                    Ok(file) => file,
                    Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                        if retried_existing_race {
                            return Err(io_err(source));
                        }
                        retried_existing_race = true;
                        continue;
                    }
                    Err(source) => return Err(io_err(source)),
                };
                if let Err(source) = file.write_all(bytes) {
                    drop(file);
                    remove_or_cleanup(&path, &source)?;
                    return Err(io_err(source));
                }
                return Ok((path, true));
            }
            Err(source) => return Err(io_err(source)),
        }
    }
}

fn remove_or_cleanup(path: &Path, prior: impl std::fmt::Display) -> Result<(), FindingsError> {
    if let Err(source) = std::fs::remove_file(path)
        && source.kind() != std::io::ErrorKind::NotFound
    {
        return Err(FindingsError::Cleanup {
            path: path.to_path_buf(),
            prior: prior.to_string(),
            source,
        });
    }
    Ok(())
}

pub struct Store {
    dir: PathBuf,
    ready: bool,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Store {
        Store {
            dir: dir.into(),
            ready: false,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn file(&mut self, artifact: &ValidatedArtifact) -> Result<PathBuf, FindingsError> {
        self.file_with_status(artifact).map(|filing| filing.path)
    }

    pub fn promote_outcome(
        folders: &ArtifactFolders,
        artifact: &ClassifiedArtifact<'_>,
        journal: &mut crate::coverage::Journal,
        elapsed_ms: Option<u64>,
        why: crate::coverage::Provenance,
        timing: crate::coverage::Timing,
    ) -> Result<PathBuf, FindingsError> {
        let mut store = Store::new(folders.path(artifact.outcome()));
        let filing = store.file_with_status(artifact.validated())?;
        let outcome = OutcomeRow {
            source: artifact.source().to_string_lossy().into_owned(),
            digest: format!("{:016x}", artifact.digest()),
            outcome: artifact.outcome().token(),
            path: filing.path.to_string_lossy().into_owned(),
            bytes: artifact.bytes().len(),
        };
        if let Err(error) = journal.required_json("outcome", &outcome) {
            store.rollback(&filing, &error)?;
            return Err(error);
        }
        let kept = crate::coverage::Entry {
            elapsed_ms,
            why,
            timing,
            path: filing.path.clone(),
            bytes: artifact.bytes().len(),
        };
        if let Err(error) = journal.record_required(&kept) {
            store.rollback(&filing, &error)?;
            return Err(error);
        }
        Ok(filing.path)
    }

    pub(crate) fn file_with_status(
        &mut self,
        artifact: &ValidatedArtifact,
    ) -> Result<Filing, FindingsError> {
        let mut created_dir = false;
        if !self.ready {
            created_dir = !self.dir.try_exists().map_err(|source| FindingsError::Io {
                path: self.dir.clone(),
                source,
            })?;
            super::mkdir(&self.dir)?;
            self.ready = true;
        }
        match store_bytes(&self.dir, artifact.digest(), artifact.bytes()) {
            Ok((path, created)) => Ok(Filing { path, created }),
            Err(error) => {
                if created_dir {
                    self.remove_empty_dir(&error)?;
                }
                Err(error)
            }
        }
    }

    pub(crate) fn rollback(
        &mut self,
        filing: &Filing,
        prior: &FindingsError,
    ) -> Result<(), FindingsError> {
        if !filing.created {
            return Ok(());
        }
        remove_or_cleanup(&filing.path, prior)?;
        self.remove_empty_dir(prior)
    }

    fn remove_empty_dir(&mut self, prior: &FindingsError) -> Result<(), FindingsError> {
        match std::fs::remove_dir(&self.dir) {
            Ok(()) => self.ready = false,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                self.ready = false;
            }
            Err(source) if source.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
            Err(source) => {
                return Err(FindingsError::Cleanup {
                    path: self.dir.clone(),
                    prior: prior.to_string(),
                    source,
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct OutcomeRow {
    source: String,
    digest: String,
    outcome: &'static str,
    path: String,
    bytes: usize,
}

pub(crate) struct Filing {
    pub path: PathBuf,
    created: bool,
}
