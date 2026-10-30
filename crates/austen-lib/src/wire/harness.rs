use std::path::Path;
use std::process::{Command, Stdio};

use super::{ECHO_ENV, LIBFUZZER_ENV, SHM_ENV_VAR};

pub const AFL_NO_FORKSRV_ENV: &str = "AFL_NO_FORKSRV";

pub const INPUT_TIMEOUT: core::time::Duration = core::time::Duration::from_millis(2000);

const MODE_VARS: &[(&str, Mode)] = &[
    (SHM_ENV_VAR, Mode::Forkserver),
    (AFL_NO_FORKSRV_ENV, Mode::AflOneShot),
    (HF_PERSISTENT_ENV, Mode::HfPersistent),
    (HF_AUTO_ENV, Mode::HfAuto),
    (ECHO_ENV, Mode::Echo),
    (LIBFUZZER_ENV, Mode::LibFuzzer),
    (VALIDATE_ENV, Mode::Validate),
];

pub const VALIDATE_ENV: &str = "AUSTEN_VALIDATE";

pub const ORACLE_CAPABILITY_SYMBOL: &str = "austen_oracle_capability";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocationRedirect {
    pub original: &'static str,
    pub replacement: &'static str,
    pub cleanup_replacement: Option<&'static str>,
}

pub const ALLOCATION_REDIRECTS: [AllocationRedirect; 5] = [
    AllocationRedirect {
        original: "free",
        replacement: "austen_harness_target_free",
        cleanup_replacement: None,
    },
    AllocationRedirect {
        original: "malloc",
        replacement: "austen_harness_target_malloc",
        cleanup_replacement: Some("austen_harness_target_malloc_owned"),
    },
    AllocationRedirect {
        original: "calloc",
        replacement: "austen_harness_target_calloc",
        cleanup_replacement: Some("austen_harness_target_calloc_owned"),
    },
    AllocationRedirect {
        original: "realloc",
        replacement: "austen_harness_target_realloc",
        cleanup_replacement: Some("austen_harness_target_realloc_owned"),
    },
    AllocationRedirect {
        original: "aligned_alloc",
        replacement: "austen_harness_target_aligned_alloc",
        cleanup_replacement: Some("austen_harness_target_aligned_alloc_owned"),
    },
];

pub fn mode_vars() -> impl Iterator<Item = &'static str> {
    MODE_VARS.iter().map(|(var, _)| *var)
}

pub fn remove_mode_vars(command: &mut Command) -> &mut Command {
    for var in mode_vars() {
        command.env_remove(var);
    }
    command
}

pub const HF_PERSISTENT_ENV: &str = "AUSTEN_HF_PERSISTENT";

pub const HF_AUTO_ENV: &str = "AUSTEN_HF_AUTO";

pub const HF_STDIN_FALLBACK_ENV: &str = "AUSTEN_HF_STDIN_FALLBACK";

pub const EXTERNAL_INPUT_BYTES: usize = 1 << 20;

pub const PERSISTENT_ENV: &str = "__AFL_PERSISTENT";

pub const PERSISTENT_YES: &str = "1";

fn hostile_inherited() -> impl Iterator<Item = &'static str> {
    MODE_VARS
        .iter()
        .filter(|(_, mode)| *mode != Mode::Forkserver)
        .map(|(var, _)| *var)
        .chain(std::iter::once(super::cover::COVER_ENV))
}

pub fn inherited_refusal() -> Option<&'static str> {
    hostile_inherited().find(|var| std::env::var_os(var).is_some())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Forkserver,
    AflOneShot,
    HfPersistent,
    HfAuto,
    Echo,
    LibFuzzer,
    Validate,
    Standalone,
}

impl Mode {
    fn does(self) -> &'static str {
        match self {
            Mode::Forkserver => "runs a campaign's inputs off AFL's shared memory",
            Mode::AflOneShot => "runs one stdin input against AFL's coverage map",
            Mode::HfPersistent => "runs a campaign's inputs off honggfuzz's descriptors",
            Mode::HfAuto => "runs honggfuzz's inputs off auto-detected descriptors or stdin",
            Mode::Echo => "prints one parse and dispatches nothing",
            Mode::LibFuzzer => "hands the process to libFuzzer's own loop",
            Mode::Validate => "validates framed inputs without dispatching",
            Mode::Standalone => "runs one input from stdin",
        }
    }

    pub fn from_env() -> Result<Mode, ModeError> {
        let no_forkserver = std::env::var_os(AFL_NO_FORKSRV_ENV).is_some();
        let declared: Vec<(&'static str, Mode)> = MODE_VARS
            .iter()
            .copied()
            .filter(|(var, _)| std::env::var_os(var).is_some())
            .map(|(var, mode)| {
                if no_forkserver && var == SHM_ENV_VAR {
                    (var, Mode::AflOneShot)
                } else {
                    (var, mode)
                }
            })
            .collect();
        let Some((var, mode)) = declared.first().copied() else {
            return Ok(Mode::Standalone);
        };
        if declared.iter().any(|(_, other)| *other != mode) {
            return Err(ModeError::Contradictory {
                declared: declared_list(&declared),
            });
        }
        if mode == Mode::Forkserver && !persistent() {
            return Err(ModeError::NotPersistent { var });
        }
        Ok(mode)
    }
}

fn persistent() -> bool {
    std::env::var_os(PERSISTENT_ENV).is_some_and(|v| v == PERSISTENT_YES)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModeError {
    #[error(
        "this launch declares more than one runtime mode — {declared}; a \
         harness is one of them for its whole life, so there is no order that \
         serves both — unset all but one"
    )]
    Contradictory { declared: String },
    #[error(
        "{PERSISTENT_ENV}={PERSISTENT_YES} is required under a forkserver \
         ({var} is present): the persistent child stops itself after every \
         input, and a forkserver that never read the variable waits for a \
         stop it cannot see — set it on the harness's environment (a tool \
         driving one input, like afl-showmap, must state it too)"
    )]
    NotPersistent { var: &'static str },
}

fn declared_list(declared: &[(&'static str, Mode)]) -> String {
    declared
        .iter()
        .map(|(var, mode)| format!("{var} {}", mode.does()))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn standalone_command(program: &Path) -> Command {
    let mut command = Command::new(program);
    remove_mode_vars(&mut command).stdin(Stdio::piped());
    command
}

pub fn wait_within(
    mut child: std::process::Child,
    timeout: core::time::Duration,
) -> Option<std::io::Result<std::process::ExitStatus>> {
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let status = child.wait();
        if status.is_err() {
            let _ = child.kill();
        }
        let _ = tx.send(status);
    });

    match rx.recv_timeout(timeout) {
        Ok(status) => Some(status),
        Err(_) => {
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
            let _ = rx.recv();
            None
        }
    }
}
