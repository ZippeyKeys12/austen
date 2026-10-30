use std::io::Write as _;
use std::path::Path;
use std::process::{ExitStatus, Stdio};

use crate::wire::{
    COVER_ENV, CoverPage, INPUT_TIMEOUT, ORACLE_STATUS_ENV, OracleStatus, TALLY_ENV, Tally,
    standalone_command, wait_within,
};

use super::{FindingsError, Verdict};

pub struct Replay<'a> {
    pub harness: &'a Path,
    pub env: &'a [(String, String)],
    pub tally: &'a Path,
    pub cover: Option<&'a Path>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measured {
    pub verdict: Verdict,
    pub cover: Option<CoverPage>,
    pub post: bool,
}

impl Replay<'_> {
    pub fn classify(&self, bytes: &[u8]) -> Result<Verdict, FindingsError> {
        Ok(self.measure(bytes)?.verdict)
    }

    pub fn measure(&self, bytes: &[u8]) -> Result<Measured, FindingsError> {
        Tally::stage(self.tally).map_err(|source| FindingsError::Io {
            path: self.tally.to_path_buf(),
            source,
        })?;
        let oracle_status_path = self.tally.with_extension("oracle-status");
        OracleStatus::stage(&oracle_status_path).map_err(|source| FindingsError::Io {
            path: oracle_status_path.clone(),
            source,
        })?;
        if let Some(path) = self.cover {
            let _ = std::fs::remove_file(path);
        }

        let mut command = standalone_command(self.harness);
        command.envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        command
            .env(TALLY_ENV, self.tally)
            .env(ORACLE_STATUS_ENV, &oracle_status_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(path) = self.cover {
            command.env(COVER_ENV, path);
        }

        let mut child = command.spawn().map_err(|source| FindingsError::Spawn {
            program: self.harness.display().to_string(),
            source,
        })?;
        if let Some(mut stdin) = child.stdin.take() {
            let bytes = bytes.to_vec();
            std::thread::spawn(move || {
                let _ = stdin.write_all(&bytes);
            });
        }

        match wait_within(child, INPUT_TIMEOUT) {
            Some(Ok(status)) => {
                let tally = Tally::read(self.tally);
                let verdict = account_for_oracle(
                    verdict_of(status, tally),
                    OracleStatus::read(&oracle_status_path),
                );
                let post = verdict == Verdict::Crash && tally.is_some_and(|t| t.post_failed > 0);
                Ok(Measured {
                    verdict,
                    cover: self.cover.and_then(CoverPage::read),
                    post,
                })
            }
            Some(Err(source)) => Err(FindingsError::Spawn {
                program: self.harness.display().to_string(),
                source,
            }),
            None => Ok(Measured {
                verdict: Verdict::Timeout,
                cover: None,
                post: false,
            }),
        }
    }
}

fn account_for_oracle(verdict: Verdict, status: Option<OracleStatus>) -> Verdict {
    match status {
        Some(status)
            if status.pre_unsupported > 0
                || status.pre_internal_error > 0
                || status.post_unsupported > 0 =>
        {
            Verdict::HarnessError
        }
        None if verdict == Verdict::NoSuchFunction => verdict,
        Some(status)
            if !matches!(
                verdict,
                Verdict::NoSuchFunction | Verdict::HarnessError | Verdict::Timeout
            ) && status.observed() == 1 =>
        {
            verdict
        }
        _ if matches!(verdict, Verdict::HarnessError | Verdict::Timeout) => verdict,
        _ => Verdict::HarnessError,
    }
}

#[cfg(unix)]
fn killed_by_signal(status: ExitStatus) -> bool {
    use std::os::unix::process::ExitStatusExt;
    status.signal().is_some()
}

#[cfg(not(unix))]
fn killed_by_signal(_status: ExitStatus) -> bool {
    false
}

fn verdict_of(status: ExitStatus, tally: Option<Tally>) -> Verdict {
    if killed_by_signal(status) {
        return Verdict::Crash;
    }
    if !status.success() {
        return Verdict::HarnessError;
    }
    match tally {
        Some(t) if t.dispatched > 0 => Verdict::Survived,
        Some(t) if t.selected > 0 => Verdict::NotDispatched,
        Some(t) if t.seen > 0 => Verdict::NoSuchFunction,
        Some(_) => Verdict::HarnessError,
        None => Verdict::HarnessError,
    }
}
