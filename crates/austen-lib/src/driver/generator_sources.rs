use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::gen_dsl::{GenModule, Sym, json};
use crate::spec::{SpecModule, validate_function_table};
use crate::{CustomProducer, ProductionModules, ProtectedPath};

type RawObject = BTreeMap<String, Box<serde_json::value::RawValue>>;

fn edit_spec_functions(
    text: &str,
    mut edit: impl FnMut(usize, &mut RawObject) -> Result<(), serde_json::Error>,
) -> Result<RawObject, serde_json::Error> {
    let mut spec: RawObject = json::from_str(text)?;
    if let Some(raw) = spec.get("functions") {
        let mut functions: Vec<RawObject> = json::from_str(raw.get())?;
        for (target_id, function) in functions.iter_mut().enumerate() {
            edit(target_id, function)?;
        }
        spec.insert(
            "functions".into(),
            serde_json::value::to_raw_value(&functions)?,
        );
    }
    Ok(spec)
}

pub fn spec_without_generators(text: &str) -> Result<Vec<u8>, serde_json::Error> {
    let spec = edit_spec_functions(text, |_, function| {
        function.remove("generator");
        Ok(())
    })?;
    serde_json::to_vec(&spec)
}

#[derive(Debug, thiserror::Error)]
pub enum GeneratorSourceError {
    #[error("cannot read generator source {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid generator JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("invalid specification {path}: {source}")]
    Spec {
        path: PathBuf,
        source: crate::spec::SpecError,
    },
    #[error("invalid function table in {path}: {source}")]
    Functions {
        path: PathBuf,
        source: Box<crate::spec::ConvertError>,
    },
    #[error("specification {path} has no function named {function:?}")]
    UnknownFunction { path: PathBuf, function: String },
    #[error(
        "loaded generator sources for {loaded_spec} (function {loaded_function:?}) cannot be used for {requested_spec} (function {requested_function:?})"
    )]
    Context {
        loaded_spec: PathBuf,
        loaded_function: Option<String>,
        requested_spec: PathBuf,
        requested_function: Option<String>,
    },
    #[error("generator source {path} has no entry named {entry:?} for target {target:?}")]
    MissingEntry {
        path: PathBuf,
        entry: String,
        target: String,
    },
    #[error("generator source {path} has multiple entries named {entry:?} for target {target:?}")]
    AmbiguousEntry {
        path: PathBuf,
        entry: String,
        target: String,
    },
    #[error("cannot write generator snapshot {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[derive(Debug, Clone)]
struct LoadedModule {
    path: PathBuf,
    text: String,
    module: Arc<GenModule>,
}

#[derive(Debug, Clone)]
struct SelectedEntry {
    target_id: u8,
    module: usize,
    entry: Sym,
}

#[derive(Debug, Clone)]
pub struct LoadedGeneratorSources {
    spec_path: PathBuf,
    spec_text: String,
    spec: SpecModule,
    function: Option<String>,
    modules: Vec<LoadedModule>,
    entries: Vec<SelectedEntry>,
    source_paths: Vec<PathBuf>,
}

impl LoadedGeneratorSources {
    pub fn load(spec: &Path, function: Option<&str>) -> Result<Self, GeneratorSourceError> {
        let spec_path = std::path::absolute(spec).map_err(|source| GeneratorSourceError::Read {
            path: spec.to_owned(),
            source,
        })?;
        let spec_text = read(&spec_path)?;
        let spec =
            SpecModule::from_json(&spec_text).map_err(|source| GeneratorSourceError::Spec {
                path: spec_path.clone(),
                source,
            })?;
        validate_function_table(&spec).map_err(|source| GeneratorSourceError::Functions {
            path: spec_path.clone(),
            source: Box::new(source),
        })?;
        if let Some(name) = function
            && spec.function(name).is_none()
        {
            return Err(GeneratorSourceError::UnknownFunction {
                path: spec_path,
                function: name.to_owned(),
            });
        }
        let mut loaded = Self {
            spec_path,
            spec_text,
            spec,
            function: function.map(str::to_owned),
            modules: Vec::new(),
            entries: Vec::new(),
            source_paths: Vec::new(),
        };
        let mut files = BTreeMap::new();
        let directory = loaded
            .spec_path
            .parent()
            .expect("absolute spec has a parent");
        for (target_id, target) in loaded.spec.functions.iter().enumerate() {
            if function.is_some_and(|selected| selected != target.name) {
                continue;
            }
            let Some(source) = &target.generator else {
                continue;
            };
            let requested = directory.join(source.file());
            let path =
                fs::canonicalize(&requested).map_err(|source| GeneratorSourceError::Read {
                    path: requested.clone(),
                    source,
                })?;
            for protected in [&requested, &path] {
                if !loaded.source_paths.contains(protected) {
                    loaded.source_paths.push(protected.clone());
                }
            }
            let index = if let Some(index) = files.get(&path) {
                *index
            } else {
                let text = read(&path)?;
                let module =
                    json::from_str(&text).map_err(|source| GeneratorSourceError::Json {
                        path: path.clone(),
                        source,
                    })?;
                let index = loaded.modules.len();
                loaded.modules.push(LoadedModule {
                    path: path.clone(),
                    text,
                    module: Arc::new(module),
                });
                files.insert(path.clone(), index);
                index
            };
            let entry_name = source.entry_or(&target.name);
            let mut candidates = loaded.modules[index]
                .module
                .defs
                .iter()
                .filter(|(_, definition)| definition.name.name.as_deref() == Some(entry_name));
            let Some((_, definition)) = candidates.next() else {
                return Err(GeneratorSourceError::MissingEntry {
                    path,
                    entry: entry_name.to_owned(),
                    target: target.name.clone(),
                });
            };
            if candidates.next().is_some() {
                return Err(GeneratorSourceError::AmbiguousEntry {
                    path,
                    entry: entry_name.to_owned(),
                    target: target.name.clone(),
                });
            }
            loaded.entries.push(SelectedEntry {
                target_id: target_id as u8,
                module: index,
                entry: definition.name.clone(),
            });
        }
        Ok(loaded)
    }

    pub fn spec_text(&self) -> &str {
        &self.spec_text
    }

    pub fn validate_context(
        &self,
        spec: &Path,
        function: Option<&str>,
    ) -> Result<(), GeneratorSourceError> {
        let requested_spec =
            std::path::absolute(spec).map_err(|source| GeneratorSourceError::Read {
                path: spec.to_owned(),
                source,
            })?;
        if self.spec_path != requested_spec || self.function.as_deref() != function {
            return Err(GeneratorSourceError::Context {
                loaded_spec: self.spec_path.clone(),
                loaded_function: self.function.clone(),
                requested_spec,
                requested_function: function.map(str::to_owned),
            });
        }
        Ok(())
    }

    pub fn has_custom(&self) -> bool {
        !self.entries.is_empty()
    }

    pub fn attach(&self, production: &mut ProductionModules) {
        for entry in &self.entries {
            let source = &self.modules[entry.module];
            production.insert_custom(
                entry.target_id,
                CustomProducer {
                    source: source.path.clone(),
                    entry: entry.entry.clone(),
                    module: source.module.clone(),
                },
            );
        }
    }

    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.source_paths.iter().map(PathBuf::as_path)
    }

    pub fn protected_paths(&self) -> impl Iterator<Item = ProtectedPath> {
        self.paths()
            .map(|path| ProtectedPath::new("custom generator source", path.to_path_buf()))
    }

    pub fn identity_bytes(&self) -> Option<Vec<u8>> {
        if self.entries.is_empty() {
            return None;
        }
        let mut bytes = b"austen-generator-sources-v1\0".to_vec();
        bytes.extend_from_slice(&(self.modules.len() as u64).to_le_bytes());
        for module in &self.modules {
            append_bytes(&mut bytes, module.text.as_bytes());
        }
        bytes.extend_from_slice(&(self.entries.len() as u64).to_le_bytes());
        for entry in &self.entries {
            bytes.push(entry.target_id);
            bytes.extend_from_slice(&(entry.module as u64).to_le_bytes());
            bytes.extend_from_slice(&entry.entry.num.to_le_bytes());
            append_bytes(
                &mut bytes,
                entry.entry.name.as_deref().unwrap_or_default().as_bytes(),
            );
        }
        Some(bytes)
    }

    pub fn snapshot_spec(&self, output: &Path) -> Result<(), GeneratorSourceError> {
        if self
            .spec
            .functions
            .iter()
            .all(|target| target.generator.is_none())
        {
            return write(output, self.spec_text.as_bytes());
        }
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let names = (0..self.modules.len())
            .map(|i| PathBuf::from(format!("generators/producer-{i:04}.json")))
            .collect::<Vec<_>>();
        if !self.modules.is_empty() {
            let directory = parent.join("generators");
            fs::create_dir_all(&directory).map_err(|source| GeneratorSourceError::Write {
                path: directory,
                source,
            })?;
            for (module, name) in self.modules.iter().zip(&names) {
                write(&parent.join(name), module.text.as_bytes())?;
            }
        }
        let json_error = |source| GeneratorSourceError::Json {
            path: self.spec_path.clone(),
            source,
        };
        let directory = self.spec_path.parent().expect("absolute spec has a parent");
        let original = edit_spec_functions(&self.spec_text, |target_id, function| {
            let Some(mut source) = self
                .spec
                .functions
                .get(target_id)
                .and_then(|target| target.generator.clone())
            else {
                return Ok(());
            };
            let file = match self
                .entries
                .iter()
                .find(|entry| usize::from(entry.target_id) == target_id)
            {
                Some(entry) => names[entry.module].clone(),
                None => directory.join(source.file()),
            };
            source.set_file(file);
            function.insert(
                "generator".into(),
                serde_json::value::to_raw_value(&source)?,
            );
            Ok(())
        })
        .map_err(json_error)?;
        let snapshot = serde_json::to_vec_pretty(&original).map_err(json_error)?;
        write(output, &snapshot)
    }
}

fn append_bytes(out: &mut Vec<u8>, value: &[u8]) {
    out.extend_from_slice(&(value.len() as u64).to_le_bytes());
    out.extend_from_slice(value);
}

fn read(path: &Path) -> Result<String, GeneratorSourceError> {
    fs::read_to_string(path).map_err(|source| GeneratorSourceError::Read {
        path: path.to_owned(),
        source,
    })
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), GeneratorSourceError> {
    fs::write(path, bytes).map_err(|source| GeneratorSourceError::Write {
        path: path.to_owned(),
        source,
    })
}
