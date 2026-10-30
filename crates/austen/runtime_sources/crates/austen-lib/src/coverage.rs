use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::wire::EdgeSet;

const HEADER: &str = "austen-corpus-journal 1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    Seed,
    Calibration,
    Novel,
    Recorded,
    Tool,
}

impl Provenance {
    pub fn token(self) -> &'static str {
        match self {
            Provenance::Seed => "seed",
            Provenance::Calibration => "calibration",
            Provenance::Novel => "novel",
            Provenance::Recorded => "recorded",
            Provenance::Tool => "tool",
        }
    }

    pub(crate) fn parse(token: &str) -> Option<Provenance> {
        match token {
            "seed" => Some(Provenance::Seed),
            "calibration" => Some(Provenance::Calibration),
            "novel" => Some(Provenance::Novel),
            "recorded" => Some(Provenance::Recorded),
            "tool" => Some(Provenance::Tool),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timing {
    Observed,
    Mtime,
    Unknown,
}

impl Timing {
    pub fn token(self) -> &'static str {
        match self {
            Timing::Observed => "observed",
            Timing::Mtime => "mtime",
            Timing::Unknown => "unknown",
        }
    }

    fn parse(token: &str) -> Option<Timing> {
        match token {
            "observed" => Some(Timing::Observed),
            "mtime" => Some(Timing::Mtime),
            "unknown" => Some(Timing::Unknown),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub elapsed_ms: Option<u64>,
    pub why: Provenance,
    pub timing: Timing,
    pub path: PathBuf,
    pub bytes: usize,
}

#[derive(Debug, Default)]
pub struct Novelty {
    seen: EdgeSet,
    seeded: bool,
}

impl Novelty {
    pub fn new() -> Novelty {
        Novelty::default()
    }

    pub fn showed_anything_new(&mut self, map: &[u8]) -> Option<Provenance> {
        let grew = self.seen.absorb_map(map);
        if !self.seeded {
            self.seeded = true;
            return Some(Provenance::Seed);
        }
        grew.then_some(Provenance::Novel)
    }
}

fn unix_millis(at: std::time::SystemTime) -> u64 {
    at.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

#[derive(Debug)]
pub struct Journal {
    path: PathBuf,
    file: std::fs::File,
    started: std::time::SystemTime,
    broken: Option<String>,
}

impl Journal {
    pub fn open(path: &Path, engine: &str, started: std::time::SystemTime) -> Journal {
        let fresh = !path.exists();
        let mut journal = Journal {
            path: path.to_path_buf(),
            file: match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                Ok(file) => file,
                Err(e) => {
                    return Journal {
                        path: path.to_path_buf(),
                        file: sink(),
                        started,
                        broken: Some(format!("{}: {e}", path.display())),
                    };
                }
            },
            started,
            broken: None,
        };
        let mut line = String::new();
        if fresh {
            line.push_str(HEADER);
            line.push('\n');
        }
        line.push_str(&format!("run {engine} {}\n", unix_millis(started)));
        journal.write(&line);
        journal
    }

    pub fn started(&self) -> std::time::SystemTime {
        self.started
    }

    pub fn broken(&self) -> Option<&str> {
        self.broken.as_deref()
    }

    pub fn record(&mut self, entry: &Entry) {
        let at = entry
            .elapsed_ms
            .map_or_else(|| "-".to_string(), |ms| ms.to_string());
        self.write(&format!(
            "kept {at} {} {} {} {}\n",
            entry.why.token(),
            entry.timing.token(),
            entry.path.display(),
            entry.bytes
        ));
    }

    pub(crate) fn record_required(
        &mut self,
        entry: &Entry,
    ) -> Result<(), crate::findings::FindingsError> {
        let at = entry
            .elapsed_ms
            .map_or_else(|| "-".to_string(), |ms| ms.to_string());
        self.write_required(&format!(
            "kept {at} {} {} {} {}\n",
            entry.why.token(),
            entry.timing.token(),
            entry.path.display(),
            entry.bytes
        ))
    }

    pub(crate) fn required_json<T: serde::Serialize>(
        &mut self,
        kind: &'static str,
        value: &T,
    ) -> Result<(), crate::findings::FindingsError> {
        let json = serde_json::to_string(value).map_err(|source| {
            crate::findings::FindingsError::Journal {
                path: self.path.clone(),
                message: source.to_string(),
            }
        })?;
        self.write_required(&format!("{kind} {json}\n"))
    }

    fn write(&mut self, line: &str) {
        if self.broken.is_some() {
            return;
        }
        if let Err(e) = self.file.write_all(line.as_bytes()) {
            self.broken = Some(format!("{}: {e}", self.path.display()));
        }
    }

    fn write_required(&mut self, line: &str) -> Result<(), crate::findings::FindingsError> {
        if let Some(message) = &self.broken {
            return Err(crate::findings::FindingsError::Journal {
                path: self.path.clone(),
                message: message.clone(),
            });
        }
        if let Err(source) = self.file.write_all(line.as_bytes()) {
            let message = source.to_string();
            self.broken = Some(format!("{}: {message}", self.path.display()));
            return Err(crate::findings::FindingsError::Journal {
                path: self.path.clone(),
                message,
            });
        }
        Ok(())
    }
}

fn sink() -> std::fs::File {
    std::fs::OpenOptions::new()
        .write(true)
        .open(if cfg!(windows) { "NUL" } else { "/dev/null" })
        .expect("the null device is writable")
}

pub fn read_last_run(path: &Path) -> Option<Vec<Entry>> {
    read_runs_from(path, 0).map(|sections| {
        sections
            .into_iter()
            .last()
            .map_or_else(Vec::new, |section| section.entries)
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSection {
    pub offset: u64,
    pub engine: String,
    pub started_unix_ms: u64,
    pub entries: Vec<Entry>,
}

pub fn read_runs_from(path: &Path, offset: u64) -> Option<Vec<RunSection>> {
    let text = std::fs::read_to_string(path).ok()?;
    if offset > text.len() as u64 {
        return None;
    }
    let mut lines = text.split_inclusive('\n');
    let header = lines.next()?;
    if header.trim_end_matches('\n') != HEADER {
        return None;
    }
    let mut position = header.len() as u64;
    let mut sections: Vec<RunSection> = Vec::new();
    let mut open = false;
    let mut seen_run = false;
    for raw in lines {
        let at = position;
        position += raw.len() as u64;
        let line = raw.trim_end_matches('\n');
        if let Some(json) = line
            .strip_prefix("validation ")
            .or_else(|| line.strip_prefix("validation-disposition "))
            .or_else(|| line.strip_prefix("outcome "))
            .or_else(|| line.strip_prefix("byte-use "))
        {
            let value: serde_json::Value = serde_json::from_str(json).ok()?;
            if !value.is_object() {
                return None;
            }
            continue;
        }
        let mut words = line.split_whitespace();
        match words.next() {
            Some("run") => {
                let engine = words.next()?.to_string();
                let started_unix_ms = words.next()?.parse().ok()?;
                seen_run = true;
                open = at >= offset;
                if open {
                    sections.push(RunSection {
                        offset: at,
                        engine,
                        started_unix_ms,
                        entries: Vec::new(),
                    });
                }
            }
            Some("kept") => {
                let at_ms = words.next()?;
                let why = Provenance::parse(words.next()?)?;
                let timing = Timing::parse(words.next()?)?;
                let path = words.next()?;
                let bytes = words.next()?.parse().ok()?;
                let entry = Entry {
                    elapsed_ms: (at_ms != "-").then(|| at_ms.parse().ok()).flatten(),
                    why,
                    timing,
                    path: PathBuf::from(path),
                    bytes,
                };
                if open {
                    sections
                        .last_mut()
                        .expect("an open section was pushed")
                        .entries
                        .push(entry);
                } else if at < offset {
                } else if !seen_run {
                    sections.push(RunSection {
                        offset: header.len() as u64,
                        engine: String::new(),
                        started_unix_ms: 0,
                        entries: vec![entry],
                    });
                    open = true;
                } else {
                    return None;
                }
            }
            Some(_) => return None,
            None => continue,
        }
    }
    Some(sections)
}
