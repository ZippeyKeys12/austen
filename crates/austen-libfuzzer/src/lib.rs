mod reduce;
pub use reduce::LibFuzzerMinimize;

use austen_lib::tools::discovery::{FileSource, discover_archive};
use std::path::{Path, PathBuf};

use austen_lib::instrumentation::{CompileRequest, Coverage};
use austen_lib::wire::{INPUT_TIMEOUT, LIBFUZZER_DRIVER_SYMBOL, LIBFUZZER_ENV};

use austen_lib::external::{
    Capabilities, Discovery, ExternalMode, Fuzzer, HarnessLink, LinkArg, NotFound,
    PersistentHookSymbols, RunPlan, RunRequest, Stats, StatsSource, TargetCompiler, Tier,
    Transport, harness_env,
};

pub const NAME: &str = "libfuzzer";

pub const DISPLAY_NAME: &str = "libFuzzer";

const SLUG: &str = "libfuzzer";

const CLANG: &str = "clang";

const COMPILE: CompileRequest = CompileRequest {
    cflags: &["-fsanitize-coverage=inline-8bit-counters"],
    env: &[],
};

const USE_COUNTERS: &str = "-use_counters=1";
const VALUE_PROFILE: &str = "-use_value_profile=1";
const USE_CMP: &str = "-use_cmp=1";

pub const LIBFUZZER_CLANG_ENV: &str = "AUSTEN_LIBFUZZER_CLANG";

pub const LIBFUZZER_RUNTIME_ENV: &str = "AUSTEN_LIBFUZZER_RUNTIME";

const VENDORED_CLANG_MAJOR: u32 = 18;

const VENDORED_CLANG: Option<&str> = option_env!("AUSTEN_LIBFUZZER_CLANG_VENDORED");

pub const EXIT_CRASH: i32 = 77;

pub const EXIT_TIMEOUT: i32 = 70;

const MAX_LEN: usize = 4096;

pub const RSS_LIMIT_MB: u64 = 4096;

fn runtime_names() -> [String; 2] {
    [
        "libclang_rt.fuzzer_no_main.a".to_string(),
        format!("libclang_rt.fuzzer_no_main-{}.a", std::env::consts::ARCH),
    ]
}

fn fuzzer_runtime(clang: impl FnOnce() -> Result<PathBuf, NotFound>) -> Result<PathBuf, NotFound> {
    let explicit = std::env::var_os(LIBFUZZER_RUNTIME_ENV);
    resolve_fuzzer_runtime(
        explicit.as_deref().map(Path::new),
        option_env!("AUSTEN_LIBFUZZER_VENDORED").map(Path::new),
        clang,
    )
}

fn resolve_fuzzer_runtime(
    explicit: Option<&Path>,
    vendored: Option<&Path>,
    clang: impl FnOnce() -> Result<PathBuf, NotFound>,
) -> Result<PathBuf, NotFound> {
    if let Some(found) = discover_archive(LIBFUZZER_RUNTIME_ENV, explicit, vendored)? {
        if found.source == FileSource::Earlier {
            validate_vendored_clang(&clang()?)?;
        }
        return Ok(found.path);
    }
    clang_fuzzer_runtime(&clang()?)
}

fn validate_vendored_clang(clang: &Path) -> Result<(), NotFound> {
    let probe = std::process::Command::new(clang).arg("--version").output();
    if let Ok(output) = &probe
        && output.status.success()
        && upstream_clang_major(&String::from_utf8_lossy(&output.stdout))
            == Some(VENDORED_CLANG_MAJOR)
    {
        return Ok(());
    }
    let detail = match probe {
        Ok(output) => String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .filter(|line| !line.is_empty())
            .map_or_else(
                || format!("--version exited with {}", output.status),
                str::to_owned,
            ),
        Err(error) => format!("could not run --version: {error}"),
    };
    Err(NotFound {
        message: format!(
            "the vendored libFuzzer runtime requires upstream Clang \
             {VENDORED_CLANG_MAJOR}, but {} reported {detail:?} — set \
             {LIBFUZZER_CLANG_ENV} to clang-{VENDORED_CLANG_MAJOR}, or set \
             {LIBFUZZER_RUNTIME_ENV} to a fuzzer_no_main archive matching your compiler",
            clang.display(),
        ),
    })
}

fn upstream_clang_major(banner: &str) -> Option<u32> {
    if banner.contains("Apple clang") {
        return None;
    }
    let (_, version) = banner.split_once("clang version ")?;
    version.split('.').next()?.parse().ok()
}

fn clang_fuzzer_runtime(clang: &Path) -> Result<PathBuf, NotFound> {
    for name in runtime_names() {
        let Ok(out) = std::process::Command::new(clang)
            .arg(format!("-print-file-name={name}"))
            .output()
        else {
            continue;
        };
        let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        if path.is_absolute() && path.is_file() {
            return Ok(path);
        }
    }
    Err(NotFound {
        message: format!(
            "{} ships no libFuzzer runtime ({}) — install the matching \
             compiler-rt (Debian/Ubuntu: libclang-rt-<version>-dev), or set \
             {LIBFUZZER_RUNTIME_ENV} to a fuzzer_no_main archive, or set \
             {LIBFUZZER_CLANG_ENV} to a clang that has one",
            clang.display(),
            runtime_names().join(" or "),
        ),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LibFuzzer {
    fork: bool,
}

impl LibFuzzer {
    pub const fn with_fork(fork: bool) -> Self {
        Self { fork }
    }

    pub const fn is_forked(self) -> bool {
        self.fork
    }
}

impl Default for LibFuzzer {
    fn default() -> Self {
        Self::with_fork(false)
    }
}

const FORK_JOBS: usize = 1;

const IGNORE_CRASHES: &str = "-ignore_crashes=1";

impl Fuzzer for LibFuzzer {
    fn name(&self) -> &'static str {
        NAME
    }

    fn mode(&self) -> Option<ExternalMode> {
        Some(if self.fork {
            ExternalMode::LibFuzzerFork
        } else {
            ExternalMode::LibFuzzerDirect
        })
    }

    fn display_name(&self) -> &'static str {
        DISPLAY_NAME
    }

    fn slug(&self) -> &'static str {
        SLUG
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            transport: Transport::InProcess,
            austen_coverage: false,
            blind: false,
        }
    }

    fn coverage(&self) -> &'static [Coverage] {
        &[
            Coverage::SancovCounters,
            Coverage::TraceCmp,
            Coverage::TraceDiv,
            Coverage::TraceGep,
            Coverage::IndirectCalls,
            Coverage::StackDepth,
        ]
    }

    fn rejected_coverage(&self) -> &'static [Coverage] {
        &[Coverage::TracePcGuard, Coverage::TracePc]
    }

    fn persistent_hooks(&self) -> Option<PersistentHookSymbols> {
        None
    }

    fn driver_symbol(&self) -> Option<&'static str> {
        Some(LIBFUZZER_DRIVER_SYMBOL)
    }

    fn compile_request(&self) -> CompileRequest {
        COMPILE
    }

    fn target_compiler(&self) -> Result<Option<TargetCompiler>, NotFound> {
        let clang = self.discover()?;
        fuzzer_runtime(|| Ok(clang.clone()))?;
        Ok(Some(TargetCompiler {
            program: clang,
            request: COMPILE,
        }))
    }

    fn harness_link(&self) -> Result<Option<HarnessLink>, NotFound> {
        let archive = fuzzer_runtime(|| self.discover())?;
        Ok(Some(HarnessLink {
            linker: None,
            link_args: vec![
                LinkArg::Path(archive),
                LinkArg::Flag("-lstdc++".to_string()),
                LinkArg::Flag("-lm".to_string()),
                LinkArg::Flag("-lgcc".to_string()),
            ],
            driver_symbol: Fuzzer::driver_symbol(self),
            persistent_hooks: Fuzzer::persistent_hooks(self),
            binary_marker: None,
        }))
    }

    fn run_plan(&self, req: &RunRequest<'_>) -> RunPlan {
        plan_for(req, self.fork.then_some(FORK_JOBS))
    }

    fn parse_stats(&self, text: &str) -> Stats {
        if self.fork {
            parse_supervisor_stats(text)
        } else {
            parse_console_stats(text)
        }
    }

    fn discovery(&self) -> Discovery {
        Discovery {
            var: LIBFUZZER_CLANG_ENV,
            exe: CLANG,
            earlier: &[Tier::Baked(VENDORED_CLANG)],
            remedy: format!(
                "install clang, rebuild with --features libfuzzer-vendored to remember Clang 18, \
                 put it on PATH, or set {LIBFUZZER_CLANG_ENV} to \
                 a clang executable (the vendored libFuzzer runtime requires clang-{VENDORED_CLANG_MAJOR})"
            ),
        }
    }
}

fn plan_for(req: &RunRequest<'_>, jobs: Option<usize>) -> RunPlan {
    let out = req.out_dir;
    let corpus = out.join("corpus");
    let crashes = out.join("crashes");
    let log = out.join("libfuzzer.log");

    let mut argv = vec![
        format!("-artifact_prefix={}/", crashes.display()),
        "-print_final_stats=1".to_string(),
        format!("-error_exitcode={EXIT_CRASH}"),
        format!("-timeout_exitcode={EXIT_TIMEOUT}"),
        format!("-timeout={}", INPUT_TIMEOUT.as_secs()),
        format!("-max_len={MAX_LEN}"),
        format!("-rss_limit_mb={RSS_LIMIT_MB}"),
        "-detect_leaks=0".to_string(),
        USE_COUNTERS.to_string(),
        USE_CMP.to_string(),
        VALUE_PROFILE.to_string(),
    ];
    if let Some(jobs) = jobs {
        argv.push(format!("-fork={jobs}"));
        argv.push(IGNORE_CRASHES.to_string());
    }
    if let Some(secs) = req.duration_secs {
        argv.push(format!("-max_total_time={secs}"));
    }
    argv.push(corpus.display().to_string());
    argv.push(req.seeds_dir.display().to_string());

    let transport_env = vec![(LIBFUZZER_ENV.to_string(), "1".to_string())];

    RunPlan {
        argv,
        env: harness_env(req),
        transport_env,
        mode_fallback: None,
        dirs: vec![out.to_path_buf(), corpus.clone(), crashes.clone()],
        crashes_dir: crashes,
        corpus_dir: Some(corpus),
        not_findings: &[],
        stats: StatsSource::Captured {
            path: log,
            exit_codes: &[0, EXIT_CRASH, EXIT_TIMEOUT],
        },
    }
}

fn parse_console_stats(text: &str) -> Stats {
    let mut stats = Stats::default();
    for line in text.lines() {
        if let Some(n) = leading_runs(line) {
            stats.execs = Some(n);
        }
        if let Some(value) = after(line, "exec/s:") {
            stats.execs_per_sec = value.parse().ok();
        }
        if let Some(value) = after(line, "stat::number_of_executed_units:") {
            stats.execs = value.parse().ok();
        }
        if let Some(value) = after(line, "stat::average_exec_per_sec:") {
            stats.execs_per_sec = value.parse().ok();
        }
    }
    stats
}

fn after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_once(key)?.1.split_whitespace().next()
}

fn leading_runs(line: &str) -> Option<u64> {
    line.strip_prefix('#')?
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .filter(|digits| !digits.is_empty())?
        .parse()
        .ok()
}

fn supervisor_runs(line: &str) -> Option<u64> {
    line.contains(" job: ")
        .then(|| leading_runs(line))
        .flatten()
}

fn parse_supervisor_stats(text: &str) -> Stats {
    let Some((line, runs)) = text
        .lines()
        .rev()
        .find_map(|line| Some((line, supervisor_runs(line)?)))
    else {
        return Stats::default();
    };
    Stats {
        execs: Some(runs),
        execs_per_sec: after(line, "time:")
            .and_then(|value| value.trim_end_matches('s').parse::<u64>().ok())
            .and_then(|secs| (secs != 0).then_some(secs))
            .map(|secs| runs as f64 / secs as f64),
    }
}
