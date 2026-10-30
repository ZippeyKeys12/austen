use std::path::{Path, PathBuf};

use austen_lib::driver::EvaluationBinary;
use austen_lib::observe::report::Instrumentation as InstrumentationToken;
use serde::{Deserialize, Serialize};

use crate::harness;

pub(crate) const NAME: &str = "prepared-harness.json";
pub(crate) const COVERAGE_DISABLED: &str = "common coverage evaluation disabled by --no-coverage";
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    version: u32,
    spec_digest: String,
    #[serde(default)]
    spec_digest_kind: SpecDigestKind,
    engine: Option<String>,
    check_heap_ownership: bool,
    campaign: Campaign,
    evaluation: Evaluation,
}

impl Manifest {
    pub(crate) fn matches_engine(&self, selected: Option<&str>) -> bool {
        match (self.engine.as_deref(), selected) {
            (Some(prepared), Some(selected)) => prepared == selected,
            _ => true,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SpecDigestKind {
    #[default]
    RawBytes,
    SpecWithoutGeneratorsV1,
}

impl SpecDigestKind {
    fn digest(&self, path: &Path) -> Result<String, String> {
        match self {
            Self::RawBytes => digest(path),
            Self::SpecWithoutGeneratorsV1 => {
                let text = std::fs::read_to_string(path)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                let bytes = austen_lib::driver::spec_without_generators(&text)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                Ok(format!("{:016x}", austen_lib::findings::digest(&bytes)))
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Campaign {
    harness: Artifact,
    instrumented: bool,
    instrumentation: InstrumentationToken,
    coverage_guards: Option<u64>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case", deny_unknown_fields)]
enum Evaluation {
    Ready {
        harness: Artifact,
        object: Artifact,
        instrumentation: InstrumentationToken,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    path: PathBuf,
    digest: String,
}

fn engine_instrumentation(
    token: InstrumentationToken,
) -> austen_lib::instrumentation::Instrumentation {
    use austen_lib::instrumentation::Instrumentation as Engine;
    match token {
        InstrumentationToken::Compiled => Engine::Compiled,
        InstrumentationToken::Detected => Engine::Detected,
        InstrumentationToken::Absent => Engine::Absent,
        InstrumentationToken::Unknown => Engine::Unknown,
    }
}

pub(crate) struct Loaded {
    pub built: harness::Built,
    pub evaluation: Result<EvaluationBinary, String>,
    pub check_heap_ownership: bool,
}

impl Loaded {
    pub fn protected_paths(&self) -> Vec<austen_lib::ProtectedPath> {
        let mut paths = Vec::new();
        if let Ok(evaluation) = &self.evaluation {
            paths.push(austen_lib::ProtectedPath::new(
                "prepared evaluation harness",
                evaluation.harness.clone(),
            ));
            if let Some(object) = &evaluation.object {
                paths.push(austen_lib::ProtectedPath::new(
                    "prepared evaluation object",
                    object.clone(),
                ));
            }
        }
        paths
    }
}

fn digest(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(format!("{:016x}", austen_lib::findings::digest(&bytes)))
}

fn artifact(root: &Path, path: &Path) -> Result<Artifact, String> {
    Ok(Artifact {
        path: path.strip_prefix(root).unwrap_or(path).to_path_buf(),
        digest: digest(path)?,
    })
}

fn retained_evaluation_object(dir: &Path, object: &Path) -> Result<PathBuf, String> {
    if object.starts_with(dir) {
        return Ok(object.to_path_buf());
    }
    let retained = dir.join("cov-harness").join("evaluation-target.bin");
    std::fs::copy(object, &retained).map_err(|error| {
        format!(
            "copying evaluation object {} to {}: {error}",
            object.display(),
            retained.display()
        )
    })?;
    Ok(retained)
}

fn verify(root: &Path, artifact: &Artifact, label: &str) -> Result<PathBuf, String> {
    let path = if artifact.path.is_absolute() {
        artifact.path.clone()
    } else {
        root.join(&artifact.path)
    };
    let found = digest(&path)?;
    if found != artifact.digest {
        return Err(format!(
            "{label} {} differs from the prepared build (digest {found}, expected {})",
            path.display(),
            artifact.digest
        ));
    }
    Ok(path)
}

pub(crate) fn read(path: &Path) -> Result<Manifest, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    if manifest.version != VERSION {
        return Err(format!(
            "{} has prepared-harness version {}, expected {VERSION}",
            path.display(),
            manifest.version
        ));
    }
    Ok(manifest)
}

pub(crate) fn write(
    dir: &Path,
    spec: &Path,
    engine: Option<&str>,
    built: &harness::Built,
    evaluation: &Result<EvaluationBinary, String>,
    check_heap_ownership: bool,
) -> Result<PathBuf, String> {
    let evaluation = match evaluation {
        Ok(binary) => match binary.object.as_deref() {
            Some(object) => Evaluation::Ready {
                harness: artifact(dir, &binary.harness)?,
                object: artifact(dir, &retained_evaluation_object(dir, object)?)?,
                instrumentation: crate::report::instrumentation(binary.instrumentation),
            },
            None => Evaluation::Unavailable {
                reason: "the evaluation build produced no target object".to_string(),
            },
        },
        Err(reason) => Evaluation::Unavailable {
            reason: reason.clone(),
        },
    };
    let spec_digest_kind = SpecDigestKind::SpecWithoutGeneratorsV1;
    let manifest = Manifest {
        version: VERSION,
        spec_digest: spec_digest_kind.digest(spec)?,
        spec_digest_kind,
        engine: engine.map(str::to_string),
        check_heap_ownership,
        campaign: Campaign {
            harness: artifact(dir, &built.harness)?,
            instrumented: built.instrumented,
            instrumentation: crate::report::instrumentation(built.instrumentation),
            coverage_guards: built.coverage_guards,
        },
        evaluation,
    };
    let path = dir.join(NAME);
    let temp = dir.join(format!(".{NAME}.{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(&manifest).expect("prepared manifest serializes");
    let written = std::fs::write(&temp, bytes).and_then(|()| std::fs::rename(&temp, &path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written.map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(path)
}

pub(crate) fn load(
    manifest_path: &Path,
    manifest: Manifest,
    supplied_harness: &Path,
    spec: &Path,
    engine: Option<&str>,
    austen_coverage: bool,
    measure_coverage: bool,
) -> Result<Loaded, String> {
    let spec_digest = manifest.spec_digest_kind.digest(spec)?;
    if spec_digest != manifest.spec_digest {
        return Err(format!(
            "specification {} differs from the prepared build",
            spec.display()
        ));
    }
    if !manifest.matches_engine(engine) {
        let prepared_engine = manifest
            .engine
            .as_deref()
            .expect("a mismatch names both engines");
        let engine = engine.expect("a mismatch names both engines");
        return Err(format!(
            "prepared harness was built for engine `{prepared_engine}`, but this run selected `{engine}`"
        ));
    }
    let supplied_digest = digest(supplied_harness)?;
    if supplied_digest != manifest.campaign.harness.digest {
        return Err(format!(
            "supplied harness {} differs from the prepared build",
            supplied_harness.display()
        ));
    }
    let root = manifest_path
        .parent()
        .ok_or("prepared manifest has no parent directory")?;
    let compatible_feedback = manifest.engine.is_some() || engine.is_none() || austen_coverage;
    let evaluation = match manifest.evaluation {
        _ if !measure_coverage => Err(COVERAGE_DISABLED.to_string()),
        Evaluation::Ready {
            harness,
            object,
            instrumentation,
        } => Ok(EvaluationBinary {
            harness: verify(root, &harness, "evaluation harness")?,
            object: Some(verify(root, &object, "evaluation object")?),
            instrumentation: engine_instrumentation(instrumentation),
        }),
        Evaluation::Unavailable { reason } => Err(reason),
    };
    Ok(Loaded {
        built: harness::Built {
            harness: supplied_harness.to_path_buf(),
            object: None,
            instrumented: compatible_feedback && manifest.campaign.instrumented,
            instrumentation: if compatible_feedback {
                engine_instrumentation(manifest.campaign.instrumentation)
            } else {
                austen_lib::instrumentation::Instrumentation::Unknown
            },
            coverage_guards: compatible_feedback
                .then_some(manifest.campaign.coverage_guards)
                .flatten(),
        },
        evaluation,
        check_heap_ownership: manifest.check_heap_ownership,
    })
}
