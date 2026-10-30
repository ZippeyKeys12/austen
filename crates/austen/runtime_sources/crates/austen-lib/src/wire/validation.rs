use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::process::Stdio;

use serde::{Deserialize, Serialize};

use super::{
    ByteUse, INPUT_TIMEOUT, ORACLE_STATUS_ENV, TRUST_ENV, Trust, VALIDATE_ENV, remove_mode_vars,
    standalone_command, wait_within,
};

pub const CONSTRAINT_FALSE_KIND: &str = "constraint-false";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationFailure {
    pub kind: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, serde_json::Value>,
}

impl ValidationFailure {
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
            fields: BTreeMap::new(),
        }
    }

    pub fn field(mut self, name: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.fields.insert(name.into(), value.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "kebab-case")]
pub enum ValidationParse {
    Passed,
    Rejected { failure: ValidationFailure },
    NoFunction,
    InternalError { failure: ValidationFailure },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "kebab-case")]
pub enum ValidationPrecondition {
    Passed,
    Failed { failure: ValidationFailure },
    Unsupported { failure: ValidationFailure },
    InternalError { failure: ValidationFailure },
    NotRun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationObservation {
    pub candidate: u64,
    pub input_bytes: u64,
    pub function_id: Option<u8>,
    pub function: Option<String>,
    pub test_case_size: Option<u64>,
    pub trust: String,
    pub pointers: String,
    pub byte_use: ByteUse,
    pub parse: ValidationParse,
    pub precondition: ValidationPrecondition,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationFrameError {
    #[error(
        "candidate {candidate} has a truncated 8-byte length prefix ({available} bytes remain)"
    )]
    TruncatedLength { candidate: u64, available: usize },
    #[error("candidate {candidate} declares {declared} bytes, but only {available} bytes remain")]
    TruncatedInput {
        candidate: u64,
        declared: u64,
        available: usize,
    },
    #[error("candidate length {length} cannot be represented on this platform")]
    LengthOverflow { length: usize },
    #[error("the framed validation request is too large")]
    StreamOverflow,
}

pub fn encode_validation_frames<'a>(
    inputs: impl IntoIterator<Item = &'a [u8]>,
) -> Result<Vec<u8>, ValidationFrameError> {
    let mut framed = Vec::new();
    for input in inputs {
        let len = u64::try_from(input.len()).map_err(|_| ValidationFrameError::LengthOverflow {
            length: input.len(),
        })?;
        framed
            .len()
            .checked_add(8)
            .and_then(|size| size.checked_add(input.len()))
            .ok_or(ValidationFrameError::StreamOverflow)?;
        framed.extend_from_slice(&len.to_le_bytes());
        framed.extend_from_slice(input);
    }
    Ok(framed)
}

pub fn decode_validation_frames(framed: &[u8]) -> Result<Vec<&[u8]>, ValidationFrameError> {
    let mut rest = framed;
    let mut candidate = 0u64;
    let mut inputs = Vec::new();
    while !rest.is_empty() {
        if rest.len() < 8 {
            return Err(ValidationFrameError::TruncatedLength {
                candidate,
                available: rest.len(),
            });
        }
        let mut prefix = [0u8; 8];
        prefix.copy_from_slice(&rest[..8]);
        rest = &rest[8..];
        let declared = u64::from_le_bytes(prefix);
        let Ok(length) = usize::try_from(declared) else {
            return Err(ValidationFrameError::TruncatedInput {
                candidate,
                declared,
                available: rest.len(),
            });
        };
        if rest.len() < length {
            return Err(ValidationFrameError::TruncatedInput {
                candidate,
                declared,
                available: rest.len(),
            });
        }
        let (input, tail) = rest.split_at(length);
        inputs.push(input);
        rest = tail;
        candidate += 1;
    }
    Ok(inputs)
}

#[derive(Debug, thiserror::Error)]
pub enum ValidationRunError {
    #[error(transparent)]
    Frame(#[from] ValidationFrameError),
    #[error("could not start validation harness `{program}`: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("validation harness I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("validation harness exceeded {timeout:?}")]
    Timeout { timeout: core::time::Duration },
    #[error("validation harness exited {status}: {stderr}")]
    Failed { status: String, stderr: String },
    #[error("validation response line {line} is not a valid observation: {source}")]
    Observation {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("validation returned {observed} observations for {expected} candidates")]
    Count { expected: usize, observed: usize },
    #[error("validation response {at} identifies candidate {candidate}")]
    Candidate { at: usize, candidate: u64 },
    #[error("validation response {at} reports {observed} input bytes, expected {expected}")]
    InputBytes {
        at: usize,
        expected: usize,
        observed: u64,
    },
    #[error("validation response {at} did not use checked parsing")]
    Trust { at: usize },
}

pub fn validate_candidates<'a>(
    program: &Path,
    env: impl IntoIterator<Item = &'a (String, String)>,
    inputs: &[&[u8]],
) -> Result<Vec<ValidationObservation>, ValidationRunError> {
    let framed = encode_validation_frames(inputs.iter().copied())?;
    let mut command = standalone_command(program);
    for (key, value) in env {
        command.env(key, value);
    }
    remove_mode_vars(&mut command)
        .env_remove(ORACLE_STATUS_ENV)
        .env(VALIDATE_ENV, "1")
        .env(TRUST_ENV, Trust::Checked.name())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|source| ValidationRunError::Spawn {
            program: program.display().to_string(),
            source,
        })?;
    let mut stdin = child.stdin.take().expect("validation stdin was piped");
    let writer = std::thread::spawn(move || stdin.write_all(&framed));

    let mut stdout = child.stdout.take().expect("validation stdout was piped");
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let mut stderr = child.stderr.take().expect("validation stderr was piped");
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });

    let extra = core::time::Duration::from_millis(
        u64::try_from(inputs.len())
            .unwrap_or(u64::MAX)
            .saturating_mul(10),
    );
    let timeout = INPUT_TIMEOUT
        .saturating_add(extra)
        .min(core::time::Duration::from_secs(30));
    let Some(status) = wait_within(child, timeout) else {
        let _ = writer.join();
        let _ = stdout_reader.join();
        let _ = stderr_reader.join();
        return Err(ValidationRunError::Timeout { timeout });
    };
    let status = status?;
    writer
        .join()
        .map_err(|_| std::io::Error::other("validation stdin writer panicked"))??;
    let stdout = stdout_reader
        .join()
        .map_err(|_| std::io::Error::other("validation stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| std::io::Error::other("validation stderr reader panicked"))??;
    if !status.success() {
        return Err(ValidationRunError::Failed {
            status: status.to_string(),
            stderr: String::from_utf8_lossy(&stderr).trim().to_owned(),
        });
    }

    let text = String::from_utf8_lossy(&stdout);
    let observations: Vec<ValidationObservation> = text
        .lines()
        .enumerate()
        .map(|(line, text)| {
            serde_json::from_str(text).map_err(|source| ValidationRunError::Observation {
                line: line + 1,
                source,
            })
        })
        .collect::<Result<_, _>>()?;
    if observations.len() != inputs.len() {
        return Err(ValidationRunError::Count {
            expected: inputs.len(),
            observed: observations.len(),
        });
    }
    for (at, (observation, input)) in observations.iter().zip(inputs).enumerate() {
        if observation.candidate != at as u64 {
            return Err(ValidationRunError::Candidate {
                at,
                candidate: observation.candidate,
            });
        }
        if observation.input_bytes != input.len() as u64 {
            return Err(ValidationRunError::InputBytes {
                at,
                expected: input.len(),
                observed: observation.input_bytes,
            });
        }
        if observation.trust != Trust::Checked.name() {
            return Err(ValidationRunError::Trust { at });
        }
    }
    Ok(observations)
}
