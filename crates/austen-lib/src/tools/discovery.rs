use std::path::{Path, PathBuf};

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct NotFound {
    pub message: String,
}

pub fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

pub fn on_path(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(exe))
        .find(|candidate| is_executable(candidate))
}

pub trait DiscoveryProvider: Sync {
    fn identity(&self) -> &'static str;

    fn declaration(&self) -> Discovery;
}

#[derive(Clone, Copy)]
pub struct DiscoveryAnchor(&'static dyn DiscoveryProvider);

impl DiscoveryAnchor {
    pub const fn new(provider: &'static dyn DiscoveryProvider) -> Self {
        Self(provider)
    }

    pub fn identity(self) -> &'static str {
        self.0.identity()
    }

    pub fn discovery(self) -> Discovery {
        self.0.declaration()
    }

    pub fn discover(self) -> Result<PathBuf, NotFound> {
        self.discovery().locate()
    }
}

impl std::fmt::Debug for DiscoveryAnchor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("DiscoveryAnchor")
            .field(&self.identity())
            .finish()
    }
}

impl PartialEq for DiscoveryAnchor {
    fn eq(&self, other: &Self) -> bool {
        self.identity() == other.identity()
    }
}

impl Eq for DiscoveryAnchor {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Baked(Option<&'static str>),
    Beside(DiscoveryAnchor),
}

impl Tier {
    fn candidate(&self, exe: &str) -> Option<PathBuf> {
        match self {
            Tier::Baked(path) => path.map(PathBuf::from),
            Tier::Beside(anchor) => Some(anchor.discover().ok()?.with_file_name(exe)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    pub var: &'static str,
    pub exe: &'static str,
    pub earlier: &'static [Tier],
    pub remedy: String,
}

impl Discovery {
    pub fn locate(&self) -> Result<PathBuf, NotFound> {
        let earlier: Vec<PathBuf> = self
            .earlier
            .iter()
            .filter_map(|tier| tier.candidate(self.exe))
            .collect();
        discover_tool(self.var, self.exe, &earlier, &self.remedy)
    }
}

pub fn discover_tool(
    var: &str,
    exe: &str,
    earlier: &[PathBuf],
    remedy: &str,
) -> Result<PathBuf, NotFound> {
    let explicit = std::env::var_os(var);
    if let Some(found) = find_in_tiers(
        var,
        explicit.as_deref().map(Path::new),
        earlier.iter().cloned(),
        FileKind::Executable,
    )? {
        return Ok(found.path);
    }
    on_path(exe).ok_or_else(|| NotFound {
        message: format!("{exe} not found — {remedy}"),
    })
}

#[derive(Clone, Copy)]
enum FileKind {
    Executable,
    Archive,
}

impl FileKind {
    fn checked(self, path: &Path) -> Option<PathBuf> {
        match self {
            Self::Executable => is_executable(path).then(|| path.to_path_buf()),
            Self::Archive => checked_archive(path),
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Executable => "an executable file",
            Self::Archive => "a regular file",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSource {
    Override,
    Earlier,
}

#[derive(Debug)]
pub struct LocatedFile {
    pub path: PathBuf,
    pub source: FileSource,
}

pub fn checked_archive(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path)
        .ok()
        .filter(|path| path.is_file())
}

fn find_in_tiers(
    var: &str,
    explicit: Option<&Path>,
    earlier: impl IntoIterator<Item = PathBuf>,
    kind: FileKind,
) -> Result<Option<LocatedFile>, NotFound> {
    if let Some(path) = explicit {
        return kind
            .checked(path)
            .map(|path| {
                Some(LocatedFile {
                    path,
                    source: FileSource::Override,
                })
            })
            .ok_or_else(|| NotFound {
                message: format!("{var} = {} is not {}", path.display(), kind.description()),
            });
    }
    Ok(earlier.into_iter().find_map(|candidate| {
        kind.checked(&candidate).map(|path| LocatedFile {
            path,
            source: FileSource::Earlier,
        })
    }))
}

pub fn discover_archive(
    var: &str,
    explicit: Option<&Path>,
    vendored: Option<&Path>,
) -> Result<Option<LocatedFile>, NotFound> {
    find_in_tiers(
        var,
        explicit,
        vendored.map(Path::to_path_buf),
        FileKind::Archive,
    )
}
