use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::observe::{Reporter, RunEvent, Warning};
use crate::wire::AustenInput;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TycheStatus {
    Passed,
    Failed,
    GaveUp,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Execution {
    pub killed: bool,
    pub post_failed: bool,
    pub pre_failed: bool,
    pub parse_rejected: bool,
    pub oracle_unsupported: bool,
    pub dispatched: bool,
}

impl TycheStatus {
    pub fn classify(execution: Execution) -> (Self, &'static str) {
        if execution.killed {
            let reason = if execution.post_failed {
                "postcondition"
            } else {
                "crash"
            };
            (Self::Failed, reason)
        } else if execution.parse_rejected {
            (Self::GaveUp, "parse rejected")
        } else if execution.pre_failed {
            (Self::GaveUp, "precondition failed")
        } else if execution.oracle_unsupported {
            (Self::GaveUp, "oracle unsupported")
        } else if !execution.dispatched {
            (Self::GaveUp, "not dispatched")
        } else {
            (Self::Passed, "")
        }
    }
}

pub struct TycheCase<'a> {
    pub property: &'a str,
    pub status: TycheStatus,
    pub status_reason: &'a str,
    pub input: Option<&'a AustenInput>,
    pub backtracks: Option<u64>,
    pub owned_bytes: Option<u64>,
    pub timing: BTreeMap<&'static str, f64>,
}

#[derive(Serialize)]
struct Features {
    #[serde(rename = "Backtracks")]
    backtracks: Option<u64>,
    #[serde(rename = "Memory Allocated (Bytes)")]
    owned_bytes: Option<u64>,
}

#[derive(Serialize)]
struct Row<'a> {
    r#type: &'static str,
    property: &'a str,
    run_start: f64,
    status: TycheStatus,
    status_reason: &'a str,
    representation: Option<String>,
    features: Features,
    timing: BTreeMap<&'static str, f64>,
}

pub struct TycheWriter {
    requested: Option<Requested>,
    run_start: f64,
}

struct Requested {
    path: PathBuf,
    state: State,
    warned: bool,
}

enum State {
    Open(BufWriter<File>),
    Failed(String),
}

impl TycheWriter {
    pub fn open(path: Option<&Path>, started: SystemTime, reporter: &Reporter) -> Self {
        let mut writer = Self {
            requested: path.map(|path| Requested {
                path: path.to_path_buf(),
                state: match File::create(path) {
                    Ok(file) => State::Open(BufWriter::new(file)),
                    Err(error) => State::Failed(error.to_string()),
                },
                warned: false,
            }),
            run_start: started
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64(),
        };
        if let Some(requested) = &mut writer.requested {
            requested.warn(reporter);
        }
        writer
    }

    pub fn enabled(&self) -> bool {
        matches!(
            &self.requested,
            Some(Requested {
                state: State::Open(_),
                ..
            })
        )
    }

    pub fn write(&mut self, case: TycheCase<'_>, reporter: &Reporter) {
        let Some(requested) = &mut self.requested else {
            return;
        };
        if let State::Open(writer) = &mut requested.state {
            let row = Row {
                r#type: "test_case",
                property: case.property,
                run_start: self.run_start,
                status: case.status,
                status_reason: case.status_reason,
                representation: case.input.map(AustenInput::to_hex),
                features: Features {
                    backtracks: case.backtracks,
                    owned_bytes: case.owned_bytes,
                },
                timing: case.timing,
            };
            let mut bytes = serde_json::to_vec(&row).expect("Tyche facts serialize");
            bytes.push(b'\n');
            if let Err(error) = writer.write_all(&bytes) {
                requested.state = State::Failed(error.to_string());
            }
        }
        requested.warn(reporter);
    }

    pub fn finish(&mut self, reporter: &Reporter) {
        let Some(requested) = &mut self.requested else {
            return;
        };
        if let State::Open(writer) = &mut requested.state
            && let Err(error) = writer.flush()
        {
            requested.state = State::Failed(error.to_string());
        }
        requested.warn(reporter);
    }
}

impl Requested {
    fn warn(&mut self, reporter: &Reporter) {
        if !self.warned
            && let State::Failed(message) = &self.state
        {
            self.warned = true;
            reporter.emit(|| {
                RunEvent::Warning(Warning::TycheWriteFailed {
                    path: self.path.clone(),
                    message: message.clone(),
                })
            });
        }
    }
}
