use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

pub const GRACE: Duration = Duration::from_secs(2);

pub const POLL_SLEEP: Duration = Duration::from_millis(5);

#[cfg(unix)]
pub fn configure_campaign_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;

    command.process_group(0);
}

#[cfg(not(unix))]
pub fn configure_campaign_process_group(_command: &mut Command) {}

pub fn stop_unverified(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
pub struct CampaignProcessGroup {
    id: libc::pid_t,
}

#[cfg(unix)]
impl CampaignProcessGroup {
    pub fn of(child: &Child) -> std::io::Result<CampaignProcessGroup> {
        let id = libc::pid_t::try_from(child.id()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("child PID {} does not fit pid_t", child.id()),
            )
        })?;
        if id <= 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("refusing unsafe campaign process-group id {id}"),
            ));
        }
        let actual = unsafe { libc::getpgid(id) };
        if actual == -1 {
            return Err(std::io::Error::last_os_error());
        }
        if actual != id {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "launched child {id} is in process group {actual}, not its required dedicated group"
                ),
            ));
        }
        Ok(CampaignProcessGroup { id })
    }

    fn target(self) -> libc::pid_t {
        self.id
            .checked_neg()
            .expect("a positive process-group id has a negative kill target")
    }

    pub fn signal(self, signal: libc::c_int) -> std::io::Result<()> {
        if unsafe { libc::kill(self.target(), signal) } == 0 {
            return Ok(());
        }
        let source = std::io::Error::last_os_error();
        if source.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(source)
        }
    }

    fn has_members(self) -> std::io::Result<bool> {
        if unsafe { libc::kill(self.target(), 0) } == 0 {
            return Ok(true);
        }
        let source = std::io::Error::last_os_error();
        match source.raw_os_error() {
            Some(libc::ESRCH) => Ok(false),
            Some(libc::EPERM) => Ok(true),
            _ => Err(source),
        }
    }

    fn quiesce(self) -> std::io::Result<()> {
        if !self.has_members()? {
            return Ok(());
        }
        self.signal(libc::SIGKILL)?;
        let deadline = Instant::now().checked_add(GRACE).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "process-group quiescence deadline overflowed the monotonic clock",
            )
        })?;
        loop {
            if !self.has_members()? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "campaign process group {} remained present after SIGKILL",
                        self.id
                    ),
                ));
            }
            std::thread::sleep(POLL_SLEEP);
        }
    }
}

pub struct CampaignChild {
    child: Child,
    #[cfg(unix)]
    group: CampaignProcessGroup,
}

impl CampaignChild {
    pub fn new(child: Child) -> std::io::Result<CampaignChild> {
        #[cfg(unix)]
        {
            let mut child = child;
            let group = match CampaignProcessGroup::of(&child) {
                Ok(group) => group,
                Err(source) => {
                    stop_unverified(&mut child);
                    return Err(source);
                }
            };
            Ok(CampaignChild { child, group })
        }
        #[cfg(not(unix))]
        {
            Ok(CampaignChild { child })
        }
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.child.wait()
    }

    pub fn kill_and_reap(&mut self) {
        let _ = self.force_terminate();
        let _ = self.child.wait();
    }

    pub fn interrupt(&mut self) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            self.group.signal(libc::SIGINT)
        }
        #[cfg(not(unix))]
        {
            Ok(())
        }
    }

    pub fn force_terminate(&mut self) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            self.group.signal(libc::SIGKILL)
        }
        #[cfg(not(unix))]
        {
            self.child.kill()
        }
    }

    pub fn quiesce_after_exit(&self) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            self.group.quiesce()
        }
        #[cfg(not(unix))]
        {
            Ok(())
        }
    }

    pub fn stop_and_quiesce(&mut self) -> std::io::Result<()> {
        self.force_terminate()?;
        let _ = self.child.wait()?;
        self.quiesce_after_exit()
    }
}
