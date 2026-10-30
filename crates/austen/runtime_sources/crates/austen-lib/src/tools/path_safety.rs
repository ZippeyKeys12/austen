use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub struct PathTraversal {
    lexical: PathBuf,
    resolved: PathBuf,
    traversed: Vec<PathBuf>,
}

#[derive(Clone)]
enum OwnedComponent {
    Prefix(OsString),
    Root(OsString),
    Cur,
    Parent,
    Normal(OsString),
}

fn components(path: &Path) -> VecDeque<OwnedComponent> {
    path.components()
        .map(|component| match component {
            Component::Prefix(prefix) => OwnedComponent::Prefix(prefix.as_os_str().to_os_string()),
            Component::RootDir => OwnedComponent::Root(component.as_os_str().to_os_string()),
            Component::CurDir => OwnedComponent::Cur,
            Component::ParentDir => OwnedComponent::Parent,
            Component::Normal(name) => OwnedComponent::Normal(name.to_os_string()),
        })
        .collect()
}

fn push_distinct(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if paths.last() != Some(&path) {
        paths.push(path);
    }
}

impl PathTraversal {
    pub fn inspect(path: &Path) -> std::io::Result<Self> {
        const MAX_SYMLINK_HOPS: usize = 64;

        let absolute = std::path::absolute(path)?;
        let lexical = normalize(&absolute);
        let mut pending = components(&absolute);
        let mut current = PathBuf::new();
        let mut traversed = Vec::new();
        let mut symlink_hops = 0usize;

        while let Some(component) = pending.pop_front() {
            match component {
                OwnedComponent::Prefix(prefix) => {
                    current.clear();
                    current.push(prefix);
                }
                OwnedComponent::Root(root) => {
                    let prefix = current.components().find_map(|component| match component {
                        Component::Prefix(prefix) => Some(prefix.as_os_str().to_os_string()),
                        _ => None,
                    });
                    current.clear();
                    if let Some(prefix) = prefix {
                        current.push(prefix);
                    }
                    current.push(root);
                }
                OwnedComponent::Cur => {}
                OwnedComponent::Parent => {
                    current.pop();
                    push_distinct(&mut traversed, normalize(&current));
                }
                OwnedComponent::Normal(name) => {
                    let candidate = normalize(&current.join(name));
                    push_distinct(&mut traversed, candidate.clone());
                    match std::fs::symlink_metadata(&candidate) {
                        Ok(metadata) if metadata.file_type().is_symlink() => {
                            symlink_hops += 1;
                            if symlink_hops > MAX_SYMLINK_HOPS {
                                return Err(std::io::Error::other(format!(
                                    "more than {MAX_SYMLINK_HOPS} symlinks while resolving {}",
                                    path.display()
                                )));
                            }
                            let target = std::fs::read_link(&candidate)?;
                            let mut expanded = components(&target);
                            expanded.append(&mut pending);
                            pending = expanded;
                        }
                        Ok(_) => current = candidate,
                        Err(source)
                            if matches!(
                                source.kind(),
                                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                            ) =>
                        {
                            current = candidate;
                        }
                        Err(source) => return Err(source),
                    }
                }
            }
        }

        Ok(Self {
            lexical,
            resolved: normalize(&current),
            traversed,
        })
    }

    pub fn crosses(&self, managed: &Self) -> bool {
        self.lexical.starts_with(&managed.lexical)
            || self.resolved.starts_with(&managed.resolved)
            || self
                .traversed
                .iter()
                .any(|path| path.starts_with(&managed.resolved))
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        self.crosses(other) || other.crosses(self)
    }

    pub fn disjoint_from<P: AsRef<Path>>(
        &self,
        protected: impl IntoIterator<Item = (&'static str, P)>,
    ) -> Result<(), Overlap> {
        for (role, path) in protected {
            let path = path.as_ref();
            let protected = Self::inspect(path).map_err(|source| Overlap::Unreadable {
                path: path.to_path_buf(),
                source,
            })?;
            if protected.overlaps(self) {
                return Err(Overlap::Conflict {
                    role,
                    path: path.to_path_buf(),
                });
            }
        }
        Ok(())
    }

    pub fn is_filesystem_root(&self) -> bool {
        self.lexical.parent().is_none() || self.resolved.parent().is_none()
    }
}

#[derive(Debug)]
pub enum Overlap {
    Unreadable {
        path: PathBuf,
        source: std::io::Error,
    },
    Conflict {
        role: &'static str,
        path: PathBuf,
    },
}

fn normalize(path: &Path) -> PathBuf {
    let absolute = path.is_absolute();
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() && !absolute {
                    normalized.push(component.as_os_str());
                }
            }
        }
    }
    normalized
}
