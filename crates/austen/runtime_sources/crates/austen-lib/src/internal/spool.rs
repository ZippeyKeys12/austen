use std::path::{Path, PathBuf};

use crate::coverage::Provenance;
use crate::tools::path_safety::{Overlap, PathTraversal};

const INDEX_WIDTH: usize = 20;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PathConflict {
    pub role: &'static str,
    pub path: PathBuf,
    pub spool: PathBuf,
}

#[derive(Debug)]
pub(crate) enum LayoutError {
    Conflict(PathConflict),
    Resolve {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[derive(Debug)]
pub(crate) struct SpoolLayout {
    dir: PathBuf,
}

fn path_traversal(path: &Path) -> Result<PathTraversal, LayoutError> {
    PathTraversal::inspect(path).map_err(|source| LayoutError::Resolve {
        path: path.to_path_buf(),
        source,
    })
}

impl SpoolLayout {
    pub(crate) fn validate<'a>(
        work: &Path,
        protected: impl IntoIterator<Item = (&'static str, &'a Path)>,
    ) -> Result<SpoolLayout, LayoutError> {
        let dir = work.join(Spool::DIR);
        let spool = path_traversal(&dir)?;
        spool
            .disjoint_from(protected)
            .map_err(|overlap| match overlap {
                Overlap::Unreadable { path, source } => LayoutError::Resolve { path, source },
                Overlap::Conflict { role, path } => LayoutError::Conflict(PathConflict {
                    role,
                    path,
                    spool: dir.clone(),
                }),
            })?;
        Ok(SpoolLayout { dir })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Retention {
    pub index: usize,
    pub elapsed_ms: u64,
    pub coverage: Option<Provenance>,
    pub killed: bool,
    pub pre_failed: bool,
    pub parse_rejected: bool,
}

impl Retention {
    pub(crate) fn file_name(&self) -> String {
        let mut name = format!(
            "{:0INDEX_WIDTH$}.{}ms.{}",
            self.index,
            self.elapsed_ms,
            self.coverage.map_or("none", Provenance::token),
        );
        if self.killed {
            name.push_str(".killed");
        }
        if self.pre_failed {
            name.push_str(".pre-failed");
        }
        if self.parse_rejected {
            name.push_str(".parse-rejected");
        }
        name
    }

    pub(crate) fn parse(path: &Path) -> Option<Retention> {
        let name = path.file_name()?.to_str()?;
        let mut parts = name.split('.');
        let index = parts.next()?;
        if index.len() != INDEX_WIDTH || !index.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let index: usize = index.parse().ok()?;
        let elapsed_ms: u64 = parts.next()?.strip_suffix("ms")?.parse().ok()?;
        let coverage = match parts.next()? {
            "none" => None,
            token => Some(Provenance::parse(token)?),
        };
        let mut killed = false;
        let mut pre_failed = false;
        let mut parse_rejected = false;
        for part in parts {
            match part {
                "killed" if !killed => killed = true,
                "pre-failed" if !pre_failed => pre_failed = true,
                "parse-rejected" if !parse_rejected => parse_rejected = true,
                _ => return None,
            }
        }
        Some(Retention {
            index,
            elapsed_ms,
            coverage,
            killed,
            pre_failed,
            parse_rejected,
        })
    }
}

#[derive(Debug)]
pub(crate) struct Spool {
    dir: PathBuf,
    population_safe: bool,
    created: bool,
    broken: Option<String>,
}

impl Spool {
    pub(crate) const DIR: &'static str = "internal-out";

    pub(crate) fn open(layout: SpoolLayout) -> Spool {
        let dir = layout.dir;
        let (population_safe, broken) = match std::fs::remove_dir_all(&dir) {
            Ok(()) => (true, None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (true, None),
            Err(e) => (false, Some(format!("{}: {e}", dir.display()))),
        };
        Spool {
            dir,
            population_safe,
            created: false,
            broken,
        }
    }

    pub(crate) fn retain(&mut self, meta: &Retention, bytes: &[u8]) {
        self.retain_with(
            meta,
            bytes,
            |path, bytes| std::fs::write(path, bytes),
            |path| std::fs::remove_file(path),
        );
    }

    pub(super) fn retain_with(
        &mut self,
        meta: &Retention,
        bytes: &[u8],
        write: impl FnOnce(&Path, &[u8]) -> std::io::Result<()>,
        remove: impl FnOnce(&Path) -> std::io::Result<()>,
    ) {
        if self.broken.is_some() {
            return;
        }
        if !self.created {
            if let Err(e) = std::fs::create_dir_all(&self.dir) {
                self.broken = Some(format!("{}: {e}", self.dir.display()));
                return;
            }
            self.created = true;
        }
        let path = self.dir.join(meta.file_name());
        if let Err(write_error) = write(&path, bytes) {
            let write_failure = format!("{}: {write_error}", path.display());
            let cleaned = remove(&path).or_else(|cleanup| {
                if cleanup.kind() == std::io::ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(cleanup)
                }
            });
            match cleaned {
                Ok(()) => self.broken = Some(write_failure),
                Err(cleanup) => {
                    self.population_safe = false;
                    self.broken = Some(format!(
                        "{write_failure}; remove possibly partial input {}: {cleanup}",
                        path.display()
                    ));
                }
            }
        }
    }

    pub(crate) fn broken(&self) -> Option<&str> {
        self.broken.as_deref()
    }

    pub(crate) fn population(&self) -> Option<&Path> {
        self.population_safe.then_some(self.dir.as_path())
    }
}
