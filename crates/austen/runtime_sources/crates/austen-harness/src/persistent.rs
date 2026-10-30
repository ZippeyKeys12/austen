use std::convert::Infallible;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicU32, Ordering};

use libafl_bolts::Error;
use libafl_bolts::os::{ChildHandle, ForkResult};
use libafl_targets::ForkserverParent;

pub const STOPPED_STATUS: i32 = (libc::SIGSTOP << 8) | 0x7f;

const CTL_FD: i32 = libafl::executors::forkserver::FORKSRV_FD;
const ST_FD: i32 = CTL_FD + 1;

const WORD: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Running,
    Reported,
}

impl Phase {
    fn from_word(word: u32) -> Phase {
        if word == 0 {
            Phase::Running
        } else {
            Phase::Reported
        }
    }

    fn word(self) -> u32 {
        match self {
            Phase::Running => 0,
            Phase::Reported => 1,
        }
    }
}

struct Stage {
    read: OwnedFd,
    write: OwnedFd,
}

impl Stage {
    fn new() -> io::Result<Stage> {
        let mut fds = [0i32; 2];
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe {
            Stage {
                read: OwnedFd::from_raw_fd(fds[0]),
                write: OwnedFd::from_raw_fd(fds[1]),
            }
        })
    }

    fn staged(&self) -> io::Result<usize> {
        let mut bytes: libc::c_int = 0;
        if unsafe { libc::ioctl(self.read.as_raw_fd(), libc::FIONREAD, &raw mut bytes) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(usize::try_from(bytes).unwrap_or(0))
    }

    fn take_word(&self) -> io::Result<()> {
        let mut word = [0u8; WORD];
        let mut got = 0;
        while got < WORD {
            let n = read_some(&self.read, &mut word[got..])?;
            if n == 0 {
                return Err(io::Error::other("a staged word vanished"));
            }
            got += n;
        }
        Ok(())
    }

    fn drain(&self) -> io::Result<()> {
        let mut left = self.staged()?;
        let mut sink = [0u8; 64];
        while left > 0 {
            let take = left.min(sink.len());
            let n = read_some(&self.read, &mut sink[..take])?;
            if n == 0 {
                break;
            }
            left -= n;
        }
        Ok(())
    }
}

pub(crate) struct Channel {
    ctl: OwnedFd,
    st: OwnedFd,
    status: Stage,
    control: Stage,
    pid: Stage,
    release: Stage,
    phase: &'static AtomicU32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Next {
    Input,
    DriverGone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Left {
    StatusOwed,
    Idle,
    PidOwed,
}

impl Channel {
    pub(crate) fn new() -> io::Result<Channel> {
        let ctl = dup_cloexec(CTL_FD)?;
        let st = dup_cloexec(ST_FD)?;
        let status = Stage::new()?;
        let control = Stage::new()?;
        let pid = Stage::new()?;
        let release = Stage::new()?;
        let page = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size_of::<AtomicU32>(),
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if page == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let phase = unsafe { &*page.cast::<AtomicU32>() };
        Ok(Channel {
            ctl,
            st,
            status,
            control,
            pid,
            release,
            phase,
        })
    }

    pub(crate) fn await_release(&self) -> Next {
        let mut byte = [0u8; 1];
        match read_some(&self.release.read, &mut byte) {
            Ok(1) => Next::Input,
            Ok(_) | Err(_) => Next::DriverGone,
        }
    }

    fn release_child(&self) -> Result<(), Error> {
        write_all(&self.release.write, &[1]).map_err(os_error)
    }

    fn reset_for_fork(&self) -> Result<(), Error> {
        self.release.drain().map_err(os_error)?;
        self.set_phase(Phase::Running);
        Ok(())
    }

    fn set_phase(&self, phase: Phase) {
        self.phase.store(phase.word(), Ordering::SeqCst);
    }

    fn phase(&self) -> Phase {
        Phase::from_word(self.phase.load(Ordering::SeqCst))
    }

    pub(crate) fn finish_input(&self, own_pid: u32) -> Next {
        if write_word(&self.status.write, STOPPED_STATUS.cast_unsigned()).is_err() {
            return Next::DriverGone;
        }
        self.set_phase(Phase::Reported);
        if splice_word(&self.status.read, &self.st).is_err() {
            return Next::DriverGone;
        }
        match splice_word(&self.ctl, &self.control.write) {
            Ok(true) => {}
            Ok(false) | Err(_) => return Next::DriverGone,
        }
        if write_word(&self.pid.write, own_pid).is_err() {
            return Next::DriverGone;
        }
        self.set_phase(Phase::Running);
        if splice_word(&self.pid.read, &self.st).is_err() {
            return Next::DriverGone;
        }
        if self.control.take_word().is_err() {
            return Next::DriverGone;
        }
        Next::Input
    }

    fn settle(&self) -> Result<Left, Error> {
        let phase = self.phase();
        let staged = |stage: &Stage| stage.staged().map_err(os_error);
        let status = staged(&self.status)?;
        let control = staged(&self.control)?;
        let pid = staged(&self.pid)?;
        let drain = |stage: &Stage| stage.drain().map_err(os_error);
        let left = match (status > 0, control > 0, pid > 0, phase) {
            (true, false, false, _) => {
                drain(&self.status)?;
                Left::StatusOwed
            }
            (false, false, false, Phase::Running) => Left::StatusOwed,
            (false, false, false, Phase::Reported) => Left::Idle,
            (false, true, false, Phase::Reported) => {
                drain(&self.control)?;
                Left::PidOwed
            }
            (false, true, true, _) => {
                drain(&self.control)?;
                drain(&self.pid)?;
                Left::PidOwed
            }
            (false, true, false, Phase::Running) => {
                drain(&self.control)?;
                Left::StatusOwed
            }
            (true, true, _, _) | (_, false, true, _) => {
                return Err(Error::illegal_state(format!(
                    "the persistent child left the direct channel in an impossible state \
                     (status {status} bytes, control {control}, pid {pid}, {phase:?})"
                )));
            }
        };
        self.set_phase(Phase::Running);
        Ok(left)
    }

    fn read_control_word(&self) -> Result<(), Error> {
        let mut buf = [0u8; WORD];
        let mut got = 0;
        while got < WORD {
            let n = read_some(&self.ctl, &mut buf[got..]).map_err(os_error)?;
            if n == 0 {
                return Err(Error::illegal_state(
                    "the control pipe closed while awaiting the next run",
                ));
            }
            got += n;
        }
        Ok(())
    }

    fn write_pid(&self, pid: i32) -> Result<(), Error> {
        write_word(&self.st, pid.cast_unsigned()).map_err(os_error)
    }
}

pub(crate) struct DirectParent<'a> {
    channel: &'a Channel,
    run_child: &'a dyn Fn() -> Infallible,
    child: Option<i32>,
    released: bool,
    narrating: bool,
}

impl<'a> DirectParent<'a> {
    pub(crate) fn new(
        channel: &'a Channel,
        run_child: &'a dyn Fn() -> Infallible,
        narrating: bool,
    ) -> Self {
        DirectParent {
            channel,
            run_child,
            child: None,
            released: true,
            narrating,
        }
    }

    fn fork_worker(&mut self) -> Result<ForkResult, Error> {
        self.channel.reset_for_fork()?;
        let forked = unsafe { libafl_bolts::os::fork() }?;
        if let ForkResult::Parent(ChildHandle { pid }) = &forked {
            self.child = Some(*pid);
            self.released = false;
        }
        Ok(forked)
    }

    fn wait_for_death(&self) -> Result<i32, Error> {
        let pid = self
            .child
            .ok_or_else(|| Error::illegal_state("waiting on a child that was never forked"))?;
        let mut status = 0i32;
        loop {
            if unsafe { libc::waitpid(pid, &raw mut status, 0) } >= 0 {
                return Ok(status);
            }
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::Interrupted {
                return Err(os_error(err));
            }
        }
    }
}

impl ForkserverParent for DirectParent<'_> {
    fn pre_fuzzing(&mut self) -> Result<(), Error> {
        if unsafe { libc::signal(libc::SIGCHLD, libc::SIG_DFL) } == libc::SIG_ERR {
            return Err(os_error(io::Error::last_os_error()));
        }
        Ok(())
    }

    fn spawn_child(&mut self, _was_killed: bool) -> Result<ForkResult, Error> {
        self.fork_worker()
    }

    fn handle_child_requests(&mut self) -> Result<i32, Error> {
        loop {
            if !self.released {
                self.channel.release_child()?;
                self.released = true;
            }
            let status = self.wait_for_death()?;
            let pid = self.child.take();
            let left = self.channel.settle()?;
            if self.narrating {
                eprintln!(
                    "austen-harness: forkserver worker {pid:?} died with status {status:#x} ({left:?})"
                );
            }
            match left {
                Left::StatusOwed => return Ok(status),
                Left::Idle => self.channel.read_control_word()?,
                Left::PidOwed => {}
            }
            match self.fork_worker()? {
                #[allow(unreachable_code)]
                ForkResult::Child => {
                    close_forkserver_fds();
                    match (self.run_child)() {}
                }
                ForkResult::Parent(ChildHandle { pid }) => self.channel.write_pid(pid)?,
            }
        }
    }
}

fn dup_cloexec(fd: i32) -> io::Result<OwnedFd> {
    let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if copy < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(copy) })
}

fn close_forkserver_fds() {
    unsafe {
        libc::close(CTL_FD);
        libc::close(ST_FD);
    }
}

fn os_error(err: io::Error) -> Error {
    Error::os_error(err, "direct forkserver channel")
}

fn read_some(fd: &OwnedFd, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        let n = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if n >= 0 {
            return Ok(usize::try_from(n).unwrap_or(0));
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

fn write_word(fd: &OwnedFd, word: u32) -> io::Result<()> {
    let bytes = word.to_ne_bytes();
    loop {
        let n = unsafe { libc::write(fd.as_raw_fd(), bytes.as_ptr().cast(), WORD) };
        if n == WORD as isize {
            return Ok(());
        }
        if n >= 0 {
            return Err(io::Error::other("short write of a protocol word"));
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

fn write_all(fd: &OwnedFd, bytes: &[u8]) -> io::Result<()> {
    loop {
        let n = unsafe { libc::write(fd.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
        if n == bytes.len() as isize {
            return Ok(());
        }
        if n >= 0 {
            return Err(io::Error::other("short write on the release pipe"));
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

fn splice_word(from: &OwnedFd, to: &OwnedFd) -> io::Result<bool> {
    let mut moved = 0usize;
    while moved < WORD {
        let n = unsafe {
            libc::splice(
                from.as_raw_fd(),
                std::ptr::null_mut(),
                to.as_raw_fd(),
                std::ptr::null_mut(),
                WORD - moved,
                0,
            )
        };
        if n > 0 {
            moved += usize::try_from(n).unwrap_or(0);
            continue;
        }
        if n == 0 {
            return Ok(false);
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
    Ok(true)
}
