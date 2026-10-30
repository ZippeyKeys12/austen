use std::io::{self, Read};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use austen_lib::wire::{AustenInput, EXTERNAL_INPUT_BYTES};

use austen_lib::tools::discovery::{Discovery, NotFound, Tier};
#[cfg(unix)]
use austen_lib::tools::process::CampaignProcessGroup;
use austen_lib::tools::process::{
    CampaignChild, GRACE, POLL_SLEEP, configure_campaign_process_group, stop_unverified,
};

pub const RADAMSA_ENV: &str = "AUSTEN_RADAMSA";

pub const RADAMSA_EXE: &str = "radamsa";

pub const SUPPORTED_VERSION: &str = "Radamsa 0.7";

const VENDORED: Option<&str> = option_env!("AUSTEN_RADAMSA_VENDORED");

const VERSION_TIMEOUT: Duration = Duration::from_secs(2);
const VERSION_STREAM_BYTES: usize = 8 * 1024;
const FRAME_READ_BYTES: usize = 16 * 1024;

pub fn discovery() -> Discovery {
    Discovery {
        var: RADAMSA_ENV,
        exe: RADAMSA_EXE,
        earlier: &[Tier::Baked(VENDORED)],
        remedy: format!(
            "reinstall austen with `--features radamsa-vendored` to build Radamsa 0.7, set \
             {RADAMSA_ENV} to a Radamsa 0.7 binary, or put one on PATH"
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Radamsa {
    executable: PathBuf,
}

impl Radamsa {
    pub fn discover() -> Result<Self, RadamsaError> {
        let executable = discovery().locate()?;
        Self::probe(executable)
    }

    pub fn probe(executable: impl Into<PathBuf>) -> Result<Self, RadamsaError> {
        let executable = executable.into();
        probe_version(&executable)?;
        Ok(Self { executable })
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn start(&self, seed_directory: &Path, seed: u64) -> Result<RadamsaSession, RadamsaError> {
        validate_seed_directory(seed_directory)?;

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .map_err(|source| io("bind the Radamsa input listener", source))?;
        listener
            .set_nonblocking(true)
            .map_err(|source| io("make the Radamsa input listener nonblocking", source))?;
        let address = listener
            .local_addr()
            .map_err(|source| io("read the Radamsa input listener address", source))?;
        let output = format!("127.0.0.1:{}", address.port());

        let mut command = Command::new(&self.executable);
        command
            .arg("--seed")
            .arg(seed.to_string())
            .args(["--count", "inf", "--output"])
            .arg(output)
            .arg("--recursive")
            .arg(seed_directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        configure_campaign_process_group(&mut command);
        let child = command
            .spawn()
            .map_err(|source| io_at("start Radamsa", &self.executable, source))?;
        let child = CampaignChild::new(child)
            .map_err(|source| io("verify the Radamsa process group", source))?;

        Ok(RadamsaSession {
            listener,
            child: Some(child),
            frame: None,
            chunk: Box::new([0; FRAME_READ_BYTES]),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FramePoll {
    Pending,
    Progress,
    Input(AustenInput),
    Oversized,
}

#[derive(Debug)]
struct PartialFrame {
    stream: TcpStream,
    bytes: Vec<u8>,
    oversized: bool,
}

pub struct RadamsaSession {
    listener: TcpListener,
    child: Option<CampaignChild>,
    frame: Option<PartialFrame>,
    chunk: Box<[u8; FRAME_READ_BYTES]>,
}

impl std::fmt::Debug for RadamsaSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RadamsaSession")
            .field("child_id", &self.child.as_ref().map(CampaignChild::id))
            .field("has_partial_frame", &self.frame.is_some())
            .finish()
    }
}

impl RadamsaSession {
    pub fn poll_frame(&mut self) -> Result<FramePoll, RadamsaError> {
        if self.child.is_none() {
            return Err(RadamsaError::Finished);
        }

        if self.frame.is_none() {
            let (stream, _) = match self.listener.accept() {
                Ok(accepted) => accepted,
                Err(source)
                    if matches!(
                        source.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    self.observe_child()?;
                    return Ok(FramePoll::Pending);
                }
                Err(source) => {
                    return Err(io("accept a Radamsa input connection", source));
                }
            };
            stream
                .set_nonblocking(true)
                .map_err(|source| io("make a Radamsa input connection nonblocking", source))?;
            self.frame = Some(PartialFrame {
                stream,
                bytes: Vec::new(),
                oversized: false,
            });
        }

        let frame = self
            .frame
            .as_mut()
            .expect("the accepted connection installed a partial frame");
        let chunk = self.chunk.as_mut_slice();
        match frame.stream.read(chunk) {
            Ok(0) => {
                let frame = self
                    .frame
                    .take()
                    .expect("the connection at EOF is still installed");
                if frame.oversized {
                    Ok(FramePoll::Oversized)
                } else {
                    Ok(FramePoll::Input(AustenInput::from_bytes(frame.bytes)))
                }
            }
            Ok(read) => {
                if !frame.oversized {
                    let retained = EXTERNAL_INPUT_BYTES
                        .saturating_add(1)
                        .saturating_sub(frame.bytes.len())
                        .min(read);
                    frame.bytes.extend_from_slice(&chunk[..retained]);
                    if frame.bytes.len() > EXTERNAL_INPUT_BYTES {
                        frame.oversized = true;
                    }
                }
                Ok(FramePoll::Progress)
            }
            Err(source)
                if matches!(
                    source.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                self.observe_child()?;
                Ok(FramePoll::Pending)
            }
            Err(source) => Err(io("read a Radamsa input connection", source)),
        }
    }

    pub fn finish(&mut self) -> Result<(), RadamsaError> {
        let deadline = Instant::now()
            .checked_add(GRACE)
            .ok_or(RadamsaError::ClockOverflow)?;
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                self.frame = None;
                return Err(RadamsaError::UnexpectedExit { status });
            }
            Ok(None) => {}
            Err(source) => {
                child.kill_and_reap();
                self.frame = None;
                return Err(io("check Radamsa before shutdown", source));
            }
        }

        if let Err(source) = child.interrupt() {
            child.kill_and_reap();
            self.frame = None;
            return Err(io("interrupt Radamsa", source));
        }
        self.frame = None;

        loop {
            match child.try_wait() {
                Ok(Some(_)) => return Ok(()),
                Ok(None) if Instant::now() < deadline => thread::sleep(POLL_SLEEP),
                Ok(None) => {
                    if let Err(source) = child.force_terminate() {
                        child.kill_and_reap();
                        return Err(io("kill Radamsa after its shutdown grace", source));
                    }
                    if let Err(source) = child.wait() {
                        child.kill_and_reap();
                        return Err(io("reap Radamsa after forced shutdown", source));
                    }
                    return Ok(());
                }
                Err(source) => {
                    child.kill_and_reap();
                    return Err(io("wait for Radamsa shutdown", source));
                }
            }
        }
    }

    fn observe_child(&mut self) -> Result<(), RadamsaError> {
        let Some(child) = self.child.as_mut() else {
            return Err(RadamsaError::Finished);
        };
        if let Some(status) = child
            .try_wait()
            .map_err(|source| io("check the Radamsa process", source))?
        {
            self.child = None;
            self.frame = None;
            return Err(RadamsaError::UnexpectedExit { status });
        }
        Ok(())
    }
}

impl Drop for RadamsaSession {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.kill_and_reap();
        }
    }
}

fn io(operation: &'static str, source: io::Error) -> RadamsaError {
    RadamsaError::Io {
        operation,
        path: None,
        source,
    }
}

fn io_at(operation: &'static str, path: &Path, source: io::Error) -> RadamsaError {
    RadamsaError::Io {
        operation,
        path: Some(path.to_path_buf()),
        source,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RadamsaError {
    #[error(transparent)]
    NotFound(#[from] NotFound),
    #[error("cannot {operation}{path}: {source}", path = display_path(.path.as_deref()))]
    Io {
        operation: &'static str,
        path: Option<PathBuf>,
        #[source]
        source: io::Error,
    },
    #[error("Radamsa version probe at {path} timed out after {VERSION_TIMEOUT:?}")]
    VersionTimeout { path: PathBuf },
    #[error("Radamsa version probe at {path} exceeded {VERSION_STREAM_BYTES} bytes on {stream}")]
    VersionOutputTooLarge { path: PathBuf, stream: &'static str },
    #[error("Radamsa version probe at {path} was refused: {reason}")]
    VersionRefused { path: PathBuf, reason: String },
    #[error("Radamsa seed directory must be absolute: {path}")]
    RelativeSeedDirectory { path: PathBuf },
    #[error("Radamsa seed directory is not a directory: {path}")]
    NotSeedDirectory { path: PathBuf },
    #[error("Radamsa seed directory is empty: {path}")]
    EmptySeedDirectory { path: PathBuf },
    #[error("Radamsa exited unexpectedly with {status}")]
    UnexpectedExit { status: ExitStatus },
    #[error("the Radamsa session has already finished")]
    Finished,
    #[error("the monotonic clock could not represent a process deadline")]
    ClockOverflow,
}

fn display_path(path: Option<&Path>) -> String {
    path.map(|path| format!(" {}", path.display()))
        .unwrap_or_default()
}

fn validate_seed_directory(path: &Path) -> Result<(), RadamsaError> {
    if !path.is_absolute() {
        return Err(RadamsaError::RelativeSeedDirectory {
            path: path.to_path_buf(),
        });
    }
    let metadata = std::fs::metadata(path)
        .map_err(|source| io_at("inspect the Radamsa seed directory", path, source))?;
    if !metadata.is_dir() {
        return Err(RadamsaError::NotSeedDirectory {
            path: path.to_path_buf(),
        });
    }
    let mut entries = std::fs::read_dir(path)
        .map_err(|source| io_at("read the Radamsa seed directory", path, source))?;
    if entries
        .next()
        .transpose()
        .map_err(|source| {
            io_at(
                "read an entry from the Radamsa seed directory",
                path,
                source,
            )
        })?
        .is_none()
    {
        return Err(RadamsaError::EmptySeedDirectory {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum ProbeStream {
    Stdout,
    Stderr,
}

impl ProbeStream {
    const fn name(self) -> &'static str {
        match self {
            ProbeStream::Stdout => "stdout",
            ProbeStream::Stderr => "stderr",
        }
    }
}

type ProbeCapture = (ProbeStream, io::Result<Vec<u8>>);

fn capture_probe<R: Read + Send + 'static>(
    reader: R,
    stream: ProbeStream,
    tx: mpsc::Sender<ProbeCapture>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = reader
            .take((VERSION_STREAM_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send((stream, result));
    })
}

fn probe_version(path: &Path) -> Result<(), RadamsaError> {
    let deadline = Instant::now()
        .checked_add(VERSION_TIMEOUT)
        .ok_or(RadamsaError::ClockOverflow)?;
    let mut child = loop {
        let mut command = Command::new(path);
        command
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_campaign_process_group(&mut command);
        match command.spawn() {
            Ok(child) => break child,
            Err(source) if executable_busy(&source) && Instant::now() < deadline => {
                thread::sleep(POLL_SLEEP);
            }
            Err(source) => {
                return Err(io_at("start the Radamsa version probe", path, source));
            }
        }
    };
    let stdout = child
        .stdout
        .take()
        .expect("piped Radamsa stdout is available");
    let stderr = child
        .stderr
        .take()
        .expect("piped Radamsa stderr is available");
    let (tx, rx) = mpsc::channel();
    let stdout_thread = capture_probe(stdout, ProbeStream::Stdout, tx.clone());
    let stderr_thread = capture_probe(stderr, ProbeStream::Stderr, tx);
    let mut status = None;
    let mut stdout = None;
    let mut stderr = None;

    let outcome = 'probe: loop {
        while let Ok((stream, result)) = rx.try_recv() {
            match result {
                Ok(bytes) if bytes.len() > VERSION_STREAM_BYTES => {
                    break 'probe Err(RadamsaError::VersionOutputTooLarge {
                        path: path.to_path_buf(),
                        stream: stream.name(),
                    });
                }
                Ok(bytes) => match stream {
                    ProbeStream::Stdout => stdout = Some(bytes),
                    ProbeStream::Stderr => stderr = Some(bytes),
                },
                Err(source) => {
                    break 'probe Err(io_at("read Radamsa version output", path, source));
                }
            }
        }
        if status.is_none() && stdout.is_some() && stderr.is_some() {
            match child.try_wait() {
                Ok(found) => status = found,
                Err(source) => {
                    break Err(io_at("wait for the Radamsa version probe", path, source));
                }
            }
        }
        if let (Some(status), Some(stdout), Some(stderr)) =
            (status, stdout.as_ref(), stderr.as_ref())
        {
            break validate_version_output(path, status, stdout, stderr);
        }
        if Instant::now() >= deadline {
            break Err(RadamsaError::VersionTimeout {
                path: path.to_path_buf(),
            });
        }
        thread::sleep(POLL_SLEEP);
    };

    if outcome.is_err() && status.is_none() {
        abort_probe(&mut child);
    } else {
        let _ = stdout_thread.join();
        let _ = stderr_thread.join();
    }
    outcome
}

#[cfg(unix)]
fn executable_busy(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ETXTBSY)
}

#[cfg(not(unix))]
fn executable_busy(_error: &io::Error) -> bool {
    false
}

#[cfg(unix)]
fn abort_probe(child: &mut Child) {
    if let Ok(group) = CampaignProcessGroup::of(child) {
        let _ = group.signal(libc::SIGKILL);
    }
    stop_unverified(child);
}

#[cfg(not(unix))]
fn abort_probe(child: &mut Child) {
    stop_unverified(child);
}

fn validate_version_output(
    path: &Path,
    status: ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<(), RadamsaError> {
    let expected_lf = format!("{SUPPORTED_VERSION}\n");
    let expected_crlf = format!("{SUPPORTED_VERSION}\r\n");
    let reason = if !status.success() {
        Some(format!("probe exited with {status}"))
    } else if !stderr.is_empty() {
        Some(format!(
            "expected empty stderr, got {:?}",
            String::from_utf8_lossy(stderr)
        ))
    } else if stdout != expected_lf.as_bytes() && stdout != expected_crlf.as_bytes() {
        Some(format!(
            "expected exactly {SUPPORTED_VERSION:?} followed by LF or CRLF, got {:?}",
            String::from_utf8_lossy(stdout)
        ))
    } else {
        None
    };
    match reason {
        Some(reason) => Err(RadamsaError::VersionRefused {
            path: path.to_path_buf(),
            reason,
        }),
        None => Ok(()),
    }
}
