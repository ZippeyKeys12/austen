use std::path::{Path, PathBuf};

use crate::stamp::Stamp;

pub const RUNTIME_ENV: &str = "AUSTEN_RUNTIME";

const DEFAULT_RUNTIME: &str = env!("AUSTEN_RUNTIME_DEFAULT");

const HOST: &str = env!("AUSTEN_HOST");

const SOURCE: &str = env!("AUSTEN_SOURCE");

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const LIBRARY: &str = if cfg!(target_os = "macos") {
    "libausten_harness.dylib"
} else {
    "libausten_harness.so"
};
pub const TEMPLATE: &str = "harness_main.rs.in";
pub const STAMP: &str = "RUNTIME";

pub const LINK_NAME: &str = "austen_harness";

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error(
        "no harness runtime found.\n  \
         {RUNTIME_ENV}: {env_desc}\n  \
         installed:      {default}\n\
         Install one with `cargo install --path crates/austen`, or point \
         {RUNTIME_ENV} at a runtime directory."
    )]
    Missing { env_desc: String, default: String },
    #[error("{path} is missing {file} — the runtime directory is incomplete")]
    Incomplete { path: PathBuf, file: &'static str },
    #[error("read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "the runtime in {path} was built for {found}, but this austen is {HOST} — \
         reinstall austen, or point {RUNTIME_ENV} at a matching runtime"
    )]
    WrongHost { path: PathBuf, found: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeOrigin {
    Override,
    Installed,
}

#[derive(Debug, Clone)]
pub struct Runtime {
    pub dir: PathBuf,
    pub library: PathBuf,
    pub template: PathBuf,
    pub stamp: Stamp,
    pub origin: RuntimeOrigin,
}

pub fn find() -> Result<Runtime, RuntimeError> {
    resolve(std::env::var_os(RUNTIME_ENV).map(PathBuf::from))
}

fn resolve(overridden: Option<PathBuf>) -> Result<Runtime, RuntimeError> {
    let (candidate, origin) = match overridden {
        Some(dir) => (dir, RuntimeOrigin::Override),
        None => (PathBuf::from(DEFAULT_RUNTIME), RuntimeOrigin::Installed),
    };
    if !candidate.join(LIBRARY).is_file() {
        return Err(RuntimeError::Missing {
            env_desc: match origin {
                RuntimeOrigin::Override => {
                    format!("{} (set, but has no {LIBRARY})", candidate.display())
                }
                RuntimeOrigin::Installed => "not set".to_string(),
            },
            default: DEFAULT_RUNTIME.to_string(),
        });
    }
    let mut rt = load(&candidate)?;
    rt.origin = origin;
    Ok(rt)
}

pub fn load(dir: &Path) -> Result<Runtime, RuntimeError> {
    let library = dir.join(LIBRARY);
    let template = dir.join(TEMPLATE);
    let stamp_path = dir.join(STAMP);
    for (path, name) in [
        (&library, LIBRARY),
        (&template, TEMPLATE),
        (&stamp_path, STAMP),
    ] {
        if !path.is_file() {
            return Err(RuntimeError::Incomplete {
                path: dir.to_path_buf(),
                file: name,
            });
        }
    }
    let text = std::fs::read_to_string(&stamp_path).map_err(|source| RuntimeError::Read {
        path: stamp_path.clone(),
        source,
    })?;
    let stamp = Stamp::parse(&text);
    if !stamp.host.is_empty() && stamp.host != HOST {
        return Err(RuntimeError::WrongHost {
            path: dir.to_path_buf(),
            found: stamp.host,
        });
    }
    Ok(Runtime {
        dir: dir.to_path_buf(),
        library,
        template,
        stamp,
        origin: RuntimeOrigin::Installed,
    })
}

pub fn is_stale(rt: &Runtime) -> bool {
    !rt.stamp.version.is_empty() && rt.stamp.version != VERSION
}

pub fn is_foreign(rt: &Runtime) -> bool {
    !rt.stamp.source.is_empty() && rt.stamp.source != SOURCE
}

pub fn host() -> &'static str {
    HOST
}

pub fn source() -> &'static str {
    SOURCE
}
