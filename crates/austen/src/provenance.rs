use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use austen_lib::DelegatedEngine;
use austen_lib::driver::PassProfile;
use austen_lib::observe::report::{
    BuildCommandProvenance, CampaignConfiguration, PlatformProvenance, ProductionConfiguration,
    RunProvenance, RuntimeProvenance, ToolProvenance,
};
use austen_lib::observe::{Reporter, RunEvent, RunObserver};
use austen_lib::shrink::Shrinker;
use austen_lib::tools::discovery::{is_executable, on_path};

use crate::args::{ShrinkChoice, Target};
use crate::runtime::Runtime;
use crate::{TestArgs, TestMode};

pub(crate) struct Capture {
    downstream: Reporter,
    state: Mutex<State>,
}

type ProbeKey = (PathBuf, BTreeMap<String, String>, Option<PathBuf>, Query);

struct State {
    provenance: RunProvenance,
    tools: Vec<(ToolProvenance, Option<ProbeKey>)>,
    versions: BTreeMap<ProbeKey, Probe>,
}

enum Probe {
    Pending(JoinHandle<Result<String, String>>),
    Done(Result<String, String>),
}

impl Probe {
    fn resolve(&mut self) -> Result<String, String> {
        if matches!(self, Probe::Pending(_)) {
            let Probe::Pending(handle) = std::mem::replace(self, Probe::Done(Err(String::new())))
            else {
                unreachable!("matched pending above");
            };
            *self = Probe::Done(
                handle
                    .join()
                    .unwrap_or_else(|_| Err("version probe panicked".to_string())),
            );
        }
        match self {
            Probe::Done(result) => result.clone(),
            Probe::Pending(_) => unreachable!("resolved above"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Query {
    Version,
    HelpBanner(&'static str),
}

impl Query {
    fn for_role(role: &str) -> Option<Self> {
        Some(match role {
            "afl++" => Query::HelpBanner("afl-fuzz"),
            "afl-tmin" => Query::HelpBanner("afl-tmin"),
            "afl-cmin" => Query::HelpBanner("afl-cmin"),
            "target-compiler" | "harness-compiler" | "target-preparation" | "linker"
            | "libfuzzer" | "honggfuzz" => Query::Version,
            _ => return None,
        })
    }

    fn argument(self) -> &'static str {
        match self {
            Query::Version => "--version",
            Query::HelpBanner(_) => "-h",
        }
    }
}

const CONFIGURATION_ENVIRONMENT: &[&str] = &[
    "CC",
    "CXX",
    "CFLAGS",
    "CXXFLAGS",
    "CPPFLAGS",
    "LDFLAGS",
    "ASAN_OPTIONS",
    "UBSAN_OPTIONS",
    "LSAN_OPTIONS",
    "MSAN_OPTIONS",
    "TSAN_OPTIONS",
    "LD_LIBRARY_PATH",
    "LD_PRELOAD",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTUP_TOOLCHAIN",
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_BUILD_TARGET",
    "CARGO_TARGET_DIR",
    "AUSTEN_RUNTIME",
    "AUSTEN_PREFIX",
    "AUSTEN_CACHE_DIR",
    austen_lib::wire::HEAP_BASE_ENV,
    "AUSTEN_TEST",
    austen_lib::instrumentation::EVALUATION_CLANG_ENV,
    #[cfg(feature = "libfuzzer")]
    austen_libfuzzer::LIBFUZZER_CLANG_ENV,
    #[cfg(feature = "libfuzzer")]
    austen_libfuzzer::LIBFUZZER_RUNTIME_ENV,
    #[cfg(feature = "honggfuzz")]
    austen_honggfuzz::HONGGFUZZ_CLANG_ENV,
    #[cfg(feature = "honggfuzz")]
    austen_honggfuzz::LIBHFUZZ_ENV,
    #[cfg(feature = "honggfuzz")]
    austen_honggfuzz::HONGGFUZZ_ENV,
    #[cfg(feature = "radamsa")]
    "AUSTEN_RADAMSA",
    #[cfg(feature = "aflpp")]
    austen_aflpp::AFL_FUZZ_ENV,
    #[cfg(feature = "aflpp")]
    "AUSTEN_AFL_TMIN",
    #[cfg(feature = "aflpp")]
    austen_aflpp::AFL_CMIN_ENV,
    #[cfg(feature = "aflpp")]
    "AFL_PATH",
    #[cfg(feature = "aflpp")]
    "AFL_CC",
    #[cfg(feature = "aflpp")]
    "AFL_CXX",
];

impl Capture {
    pub(crate) fn new(t: &TestArgs, downstream: Reporter) -> Arc<Self> {
        Arc::new(Self {
            downstream,
            state: Mutex::new(State {
                provenance: initial(t),
                tools: Vec::new(),
                versions: BTreeMap::new(),
            }),
        })
    }

    pub(crate) fn snapshot(&self) -> RunProvenance {
        let mut state = self.state.lock().expect("provenance lock");
        let State {
            provenance,
            tools,
            versions,
        } = &mut *state;
        let mut provenance = provenance.clone();
        for (tool, key) in tools.iter() {
            let mut tool = tool.clone();
            if let Some(key) = key {
                match versions
                    .get_mut(key)
                    .expect("every keyed tool has a probe")
                    .resolve()
                {
                    Ok(version) => tool.version = Some(version),
                    Err(error) => tool.version_error = Some(error),
                }
            }
            provenance.tools.push(tool);
        }
        provenance
    }
}

pub(crate) fn runtime_provenance(rt: &Runtime) -> RuntimeProvenance {
    RuntimeProvenance {
        directory: rt.dir.clone(),
        version: rt.stamp.version.clone(),
        host: rt.stamp.host.clone(),
        profile: rt.stamp.profile.clone(),
        source: rt.stamp.source.clone(),
    }
}

impl State {
    fn tool(
        &mut self,
        role: &str,
        program: &Path,
        version: Option<&str>,
        environment: &BTreeMap<String, String>,
        cwd: Option<&Path>,
    ) {
        let resolved_path = resolve_program(program, cwd);
        let mut tool = ToolProvenance {
            role: role.to_string(),
            program: program.to_path_buf(),
            resolved_path: resolved_path.clone(),
            version: version.map(str::to_string),
            version_error: None,
        };
        let key = match (version, Query::for_role(role)) {
            (Some(_), _) => None,
            (None, None) => {
                tool.version_error =
                    Some("no safe version query is defined for this tool".to_string());
                None
            }
            (None, Some(query)) => {
                let key = (
                    resolved_path
                        .clone()
                        .unwrap_or_else(|| program.to_path_buf()),
                    environment.clone(),
                    cwd.map(Path::to_path_buf),
                    query,
                );
                self.versions
                    .entry(key.clone())
                    .or_insert_with(|| match &resolved_path {
                        None => Probe::Done(Err("executable lookup failed".to_string())),
                        Some(path) => {
                            let (path, environment, cwd) = (
                                path.clone(),
                                environment.clone(),
                                cwd.map(Path::to_path_buf),
                            );
                            Probe::Pending(std::thread::spawn(move || {
                                probe_version(&path, &environment, cwd.as_deref(), query)
                            }))
                        }
                    });
                Some(key)
            }
        };
        if !self.tools.iter().any(|(seen, _)| *seen == tool) {
            self.tools.push((tool, key));
        }
    }
}

impl RunObserver for Capture {
    fn event(&self, event: RunEvent) {
        {
            let mut state = self.state.lock().expect("provenance lock");
            match &event {
                RunEvent::ToolSelected {
                    name,
                    path,
                    version,
                } => {
                    state.tool(name, path, version.as_deref(), &BTreeMap::new(), None);
                }
                RunEvent::BuildCommand {
                    directory,
                    role,
                    program,
                    arguments,
                    environment,
                    working_directory,
                } => {
                    state.tool(
                        role,
                        program,
                        None,
                        environment,
                        working_directory.as_deref(),
                    );
                    state.provenance.builds.push(BuildCommandProvenance {
                        directory: directory.clone(),
                        role: role.clone(),
                        program: program.clone(),
                        arguments: arguments.clone(),
                        environment: environment.clone(),
                        working_directory: working_directory.clone(),
                    });
                }
                RunEvent::HarnessReused { harness } => {
                    state.provenance.reused_harnesses.push(harness.clone());
                }
                _ => {}
            }
        }
        self.downstream.emit(|| event);
    }
}

fn initial(t: &TestArgs) -> RunProvenance {
    let profile = t
        .schedule()
        .into_iter()
        .flat_map(austen_lib::Schedule::profiles)
        .fold(PassProfile::NONE, PassProfile::union);
    let options = t.pass_options;
    let (mode, seed, replay, record) = match &t.mode {
        TestMode::Campaign { seed, record, .. } => (
            "campaign",
            t.descriptor().generic_seed.then_some(*seed),
            None,
            record.clone(),
        ),
        TestMode::Replay { tape, record } => ("replay", None, Some(tape.clone()), record.clone()),
        TestMode::Build => ("build", None, None, None),
        TestMode::SingleInput { .. } => ("single-input", None, None, None),
    };
    let selected_engine = t.schedule();
    let executable = std::env::current_exe().ok();
    RunProvenance {
        captured_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        invocation_directory: std::env::current_dir().ok(),
        platform: PlatformProvenance {
            os: std::env::consts::OS.to_string(),
            architecture: std::env::consts::ARCH.to_string(),
            kernel_release: std::fs::read_to_string("/proc/sys/kernel/osrelease")
                .ok()
                .map(|value| value.trim().to_string()),
            available_parallelism: std::thread::available_parallelism().ok().map(usize::from),
            austen_build_host: env!("AUSTEN_HOST").to_string(),
            austen_build_profile: env!("AUSTEN_BUILD_PROFILE").to_string(),
        },
        environment: Some(
            CONFIGURATION_ENVIRONMENT
                .iter()
                .filter_map(|name| {
                    std::env::var_os(name)
                        .map(|value| ((*name).to_string(), value.to_string_lossy().into_owned()))
                })
                .collect(),
        ),
        configuration: CampaignConfiguration {
            mode: mode.to_string(),
            engine: selected_engine.map(|schedule| schedule.to_string()),
            external_mode: selected_engine.and_then(|schedule| {
                schedule
                    .delegated_stage()
                    .and_then(|(_, engine)| engine.mode())
                    .map(crate::report::external_mode)
            }),
            function_selector: t
                .reported_function_selector()
                .map(crate::report::function_selector),
            max_generator_size: t.reported_max_generator_size().map(|size| size.get()),
            chain_discard_limit: t.reported_chain_discard_limit().map(|limit| limit.get()),
            duration_seconds: t.mode.duration_secs(),
            seed,
            replay,
            record,
            work: t.work.clone(),
            crashes: t.crashes.clone(),
            corpus: None,
            target_kind: match &t.target {
                Some(Target::CSource(_)) => "source",
                Some(Target::Object(_)) => "object",
                Some(Target::Archive(_)) => "archive",
                None => "harness",
            }
            .to_string(),
            coverage: t
                .coverage
                .iter()
                .map(|mode| mode.token().to_string())
                .collect(),
            cflags: t.cflags.clone(),
            corpus_snapshot_interval_ms: t.corpus_snapshot_interval.map(|duration| {
                duration
                    .as_millis()
                    .try_into()
                    .expect("validated snapshot interval")
            }),
            resolved_snapshot_interval_ms: None,
            production: ProductionConfiguration {
                path_pruning: options.path_pruning,
                smt_pruning: options.smt_pruning,
                inline: options.inline.as_str().to_string(),
                max_unfolds: options.max_unfolds,
                experimental_product_arg_destruction: options.experimental_product_arg_destruction,
                experimental_return_pruning: options.experimental_return_pruning,
                experimental_arg_pruning: options.experimental_arg_pruning,
                sink_args: options.sink_args,
                max_array_length: options.max_array_length,
                lift_constraints: profile.lift_constraints,
                each_fusion: profile.each_fusion,
                apply_array_max_length: profile.apply_array_max_length,
                specialize_guarded_choices: options.specialize_guarded_choices,
                specialize_guarded_choices_elected: profile.specialize_guarded_choices,
            },
            check_heap_ownership: t.check_heap_ownership,
            allow_unstable_addresses: t.allow_unstable_addresses,
            shrink: t.shrink.map(|choice| match choice {
                ShrinkChoice::Default => "default".to_string(),
                ShrinkChoice::Named(shrinker) => shrinker.name().to_string(),
            }),
            resolved_shrinker: None,
            minimize_suite: t.minimize_suite,
            exit_fast: t.exit_fast,
            strict_duration: t.strict_duration,
            fail_on_crash: t.fail_on_crash,
            clean: t.clean,
            debug_child: std::env::var_os(austen_lib::driver::DEBUG_CHILD_ENV).is_some(),
        },
        tools: vec![ToolProvenance {
            role: "austen".to_string(),
            program: executable.clone().unwrap_or_else(|| "austen".into()),
            resolved_path: executable,
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            version_error: None,
        }],
        libraries: BTreeMap::from([
            #[cfg(feature = "smt")]
            (
                "z3".to_string(),
                austen_lib::smt_runtime_version().to_string(),
            ),
        ]),
        builds: Vec::new(),
        reused_harnesses: Vec::new(),
        runtime: None,
    }
}

fn resolve_program(program: &Path, cwd: Option<&Path>) -> Option<PathBuf> {
    if program.components().count() > 1 || program.is_absolute() {
        let cwd = cwd
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())?;
        let path = cwd.join(program);
        return is_executable(&path).then_some(path);
    }
    on_path(program.to_str()?)
}

const VERSION_TIMEOUT: Duration = Duration::from_secs(2);
const VERSION_BYTES: usize = 8192;

#[cfg(unix)]
fn probe_version(
    program: &Path,
    environment: &BTreeMap<String, String>,
    cwd: Option<&Path>,
    query: Query,
) -> Result<String, String> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;

    let mut command = Command::new(program);
    command
        .arg(query.argument())
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let deadline = Instant::now() + VERSION_TIMEOUT;
    let mut owns_child = true;
    let outcome = (|| {
        for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
        }
        let mut outputs = [Vec::new(), Vec::new()];
        let mut ended = [false, false];
        loop {
            for (index, reader) in [&mut stdout as &mut dyn Read, &mut stderr]
                .into_iter()
                .enumerate()
            {
                let mut buffer = [0; 1024];
                match reader.read(&mut buffer) {
                    Ok(0) => ended[index] = true,
                    Ok(count) => {
                        outputs[index].extend_from_slice(&buffer[..count]);
                        if outputs[index].len() > VERSION_BYTES {
                            return Err("version output exceeded 8192 bytes".to_string());
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
            if ended == [true, true]
                && let Some(status) = peek_probe_exit(&child).map_err(|error| {
                    if error.raw_os_error() == Some(libc::ECHILD) {
                        owns_child = false;
                    }
                    error.to_string()
                })?
            {
                let text = outputs
                    .into_iter()
                    .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                let text = anstream::adapter::strip_str(&text).to_string();
                if let Query::HelpBanner(prefix) = query {
                    if (status.success() || status.code() == Some(1))
                        && let Some(banner) = text
                            .lines()
                            .find(|line| line.trim_start().starts_with(prefix))
                    {
                        return Ok(banner.trim().to_string());
                    }
                    return Err(format!(
                        "help query returned no recognized version banner ({status})"
                    ));
                }
                if !status.success() {
                    return Err(format!("version query exited with {status}"));
                }
                return if text.is_empty() {
                    Err("version query returned no output".to_string())
                } else {
                    Ok(text)
                };
            }
            if Instant::now() >= deadline {
                return Err("version query timed out after 2 seconds".to_string());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    })();
    if owns_child {
        let pid = child.id() as libc::pid_t;
        if unsafe { libc::getpgid(pid) } == pid {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    outcome
}

#[cfg(unix)]
fn peek_probe_exit(
    child: &std::process::Child,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    use std::os::unix::process::ExitStatusExt;
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id() as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { info.si_pid() } == 0 {
        return Ok(None);
    }
    let status = unsafe { info.si_status() };
    let raw = match info.si_code {
        libc::CLD_EXITED => status << 8,
        libc::CLD_KILLED => status,
        libc::CLD_DUMPED => status | 0x80,
        _ => {
            return Err(std::io::Error::other(
                "unexpected version-probe child status",
            ));
        }
    };
    Ok(Some(std::process::ExitStatus::from_raw(raw)))
}

#[cfg(not(unix))]
fn probe_version(
    _program: &Path,
    _environment: &BTreeMap<String, String>,
    _cwd: Option<&Path>,
    _query: Query,
) -> Result<String, String> {
    Err("bounded version queries are unavailable on this platform".to_string())
}
