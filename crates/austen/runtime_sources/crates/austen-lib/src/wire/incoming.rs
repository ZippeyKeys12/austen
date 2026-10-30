use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::gen_dsl::Sym;
use serde::{Deserialize, Serialize};

use super::Region;

pub const INCOMING_ENV: &str = "AUSTEN_INCOMING";

pub const INCOMING_VERSION: u8 = 1;

pub use crate::gen_dsl::PublishedGlobal as IncomingGlobal;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingInfo {
    pub layout: String,
    pub heap: Region,
    pub globals: Vec<IncomingGlobal>,
}

impl IncomingInfo {
    pub fn new(layout: String, heap: Region, globals: Vec<IncomingGlobal>) -> Self {
        Self {
            layout,
            heap,
            globals,
        }
    }

    pub fn to_line(&self) -> String {
        let wire = IncomingWire {
            version: INCOMING_VERSION,
            layout: self.layout.clone(),
            heap: RegionWire {
                base: self.heap.base,
                size: self.heap.size,
            },
            globals: self
                .globals
                .iter()
                .map(|global| GlobalWire {
                    sym: SymWire {
                        name: global.sym.name.clone(),
                        num: global.sym.num,
                    },
                    address: global.address,
                    size: global.size,
                })
                .collect(),
        };
        let mut line = serde_json::to_string(&wire)
            .expect("serializing the closed IncomingInfo wire mirror cannot fail");
        line.push('\n');
        line
    }

    pub fn parse(line: &str) -> Result<Self, IncomingParseError> {
        let json = line
            .strip_suffix('\n')
            .ok_or(IncomingParseError::MissingLf)?;
        if json.contains(['\n', '\r']) {
            return Err(IncomingParseError::NotOneLine);
        }
        let wire: IncomingWire =
            serde_json::from_str(json).map_err(|error| IncomingParseError::Json {
                message: error.to_string(),
            })?;
        if wire.version != INCOMING_VERSION {
            return Err(IncomingParseError::UnsupportedVersion {
                found: wire.version,
            });
        }
        Ok(Self {
            layout: wire.layout,
            heap: Region {
                base: wire.heap.base,
                size: wire.heap.size,
            },
            globals: wire
                .globals
                .into_iter()
                .map(|global| IncomingGlobal {
                    sym: Sym {
                        name: global.sym.name,
                        num: global.sym.num,
                    },
                    address: global.address,
                    size: global.size,
                })
                .collect(),
        })
    }

    pub fn poll(path: &Path, timeout: Duration) -> Result<Self, IncomingPollError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(line) = std::fs::read_to_string(path)
                && line.ends_with('\n')
            {
                return Self::parse(&line)
                    .map_err(|source| IncomingPollError::Malformed { line, source });
            }
            if Instant::now() >= deadline {
                return Err(IncomingPollError::Timeout {
                    path: path.to_path_buf(),
                    timeout,
                });
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IncomingWire {
    version: u8,
    layout: String,
    heap: RegionWire,
    globals: Vec<GlobalWire>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegionWire {
    base: u64,
    size: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GlobalWire {
    sym: SymWire,
    address: u64,
    size: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SymWire {
    #[serde(deserialize_with = "deserialize_sym_name")]
    name: Option<String>,
    num: u64,
}

fn deserialize_sym_name<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IncomingParseError {
    #[error("incoming information is not terminated by LF")]
    MissingLf,
    #[error("incoming information must occupy exactly one LF-terminated line")]
    NotOneLine,
    #[error("incoming information is not strict JSON: {message}")]
    Json { message: String },
    #[error("incoming information has unsupported version {found}; expected {INCOMING_VERSION}")]
    UnsupportedVersion { found: u8 },
}

pub const PUBLISH_TIMEOUT: Duration = Duration::from_secs(10);

const POLL_INTERVAL: Duration = Duration::from_millis(5);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IncomingPollError {
    #[error("no incoming information published to {path} within {timeout:?}")]
    Timeout { path: PathBuf, timeout: Duration },
    #[error("published incoming information is malformed: {source}; line: {line:?}")]
    Malformed {
        line: String,
        #[source]
        source: IncomingParseError,
    },
}
