use austen_lib::tools::discovery::{checked_archive, discover_archive};
use std::path::{Path, PathBuf};

use austen_lib::instrumentation::{CompileRequest, Coverage};

use austen_lib::external::{
    BinaryMarker, Capabilities, Discovery, ExternalError, ExternalMode, Fuzzer, HarnessLink,
    LinkArg, ModeFallback, NotFound, PersistentHookSymbols, RunPlan, RunRequest, Stats,
    StatsSource, TargetCompiler, Tier, Transport, discover_tool, harness_env, is_executable,
    on_path, parse_csv_stats,
};

pub const NAME: &str = "honggfuzz";

pub const DISPLAY_NAME: &str = "honggfuzz";

const SLUG: &str = "honggfuzz";

const PROGRAM: &str = "honggfuzz";

pub const HONGGFUZZ_ENV: &str = "AUSTEN_HONGGFUZZ";

const VENDORED: Option<&str> = option_env!("AUSTEN_HONGGFUZZ_VENDORED");

const REPORT_FILE: &str = "HONGGFUZZ.REPORT.TXT";

const STDIN_FALLBACK_FILE: &str = "stdin-fallback";

const PERSISTENT_SIGNATURE: &[u8] = b"\x01_LIBHFUZZ_PERSISTENT_BINARY_SIGNATURE_\x02\xff";

fn advertises_persistence(path: &Path) -> std::io::Result<bool> {
    let executable = std::fs::read(path)?;
    Ok(executable
        .windows(PERSISTENT_SIGNATURE.len())
        .any(|window| window == PERSISTENT_SIGNATURE))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HonggfuzzMode {
    #[default]
    Auto,
    Persistent,
    Stdin,
}

impl HonggfuzzMode {
    const fn transport(self) -> Transport {
        match self {
            HonggfuzzMode::Auto => Transport::HfAuto,
            HonggfuzzMode::Persistent => Transport::HfPersistent,
            HonggfuzzMode::Stdin => Transport::Stdin,
        }
    }
}

const INSTRUMENT: bool = true;

const HFUZZ_CLANG: &str = "hfuzz-clang";

pub const LIBHFUZZ_ENV: &str = "AUSTEN_HONGGFUZZ_LIBHFUZZ";

pub const HONGGFUZZ_CLANG_ENV: &str = "AUSTEN_HONGGFUZZ_CLANG";

const CLANG: &str = "clang";

const COMPILE: CompileRequest = CompileRequest {
    cflags: &["-fsanitize-coverage=trace-pc-guard"],
    env: &[],
};

const MODULE_ANCHOR: &str = "LIBHFUZZ_module_instrument";

fn hfuzz_archives(
    hfuzz_clang: impl FnOnce() -> Result<PathBuf, NotFound>,
) -> Result<(PathBuf, PathBuf), NotFound> {
    let explicit = std::env::var_os(LIBHFUZZ_ENV);
    resolve_hfuzz_archives(explicit.as_deref().map(Path::new), hfuzz_clang)
}

fn resolve_hfuzz_archives(
    explicit: Option<&Path>,
    hfuzz_clang: impl FnOnce() -> Result<PathBuf, NotFound>,
) -> Result<(PathBuf, PathBuf), NotFound> {
    if let Some(found) = discover_archive(LIBHFUZZ_ENV, explicit, None)? {
        let libhfuzz = found.path;
        let common = beside(explicit.unwrap_or(&libhfuzz), "libhfcommon")?;
        return Ok((libhfuzz, common));
    }

    let hfuzz_clang = hfuzz_clang()?;
    let out = std::process::Command::new(&hfuzz_clang)
        .args(["-###", "-x", "c", "/dev/null", "-o", "/dev/null"])
        .output()
        .map_err(|source| NotFound {
            message: format!(
                "could not run {} to locate libhfuzz.a: {source}",
                hfuzz_clang.display()
            ),
        })?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let find = |stem: &str| {
        text.split_whitespace()
            .map(|token| PathBuf::from(token.trim_matches('"')))
            .find(|path| path.is_absolute() && path.is_file() && named(path, stem))
    };
    match (find("libhfuzz"), find("libhfcommon")) {
        (Some(libhfuzz), Some(common)) => Ok((libhfuzz, common)),
        _ => Err(NotFound {
            message: format!(
                "{} did not produce libhfuzz.a and libhfcommon.a — it embeds \
                 them and dumps them into /tmp on first use, so this usually \
                 means /tmp is unwritable or mounted noexec. Set {LIBHFUZZ_ENV} \
                 to a libhfuzz.a, or run with a honggfuzz built from source",
                hfuzz_clang.display()
            ),
        }),
    }
}

fn named(path: &Path, stem: &str) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with(&format!("{stem}.")) && n.ends_with(".a"))
}

fn beside(libhfuzz: &Path, stem: &str) -> Result<PathBuf, NotFound> {
    let dir = libhfuzz.parent().unwrap_or_else(|| Path::new("."));
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let entries = std::fs::read_dir(dir).map_err(|error| NotFound {
        message: format!(
            "could not search beside {LIBHFUZZ_ENV}={} for {stem}.a: {error}",
            libhfuzz.display()
        ),
    })?;
    let mut candidates = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| NotFound {
                message: format!(
                    "could not read a companion archive entry in {}: {error}",
                    dir.display()
                ),
            })?
            .path();
        if named(&path, stem)
            && let Some(path) = checked_archive(&path)
        {
            candidates.push(path);
        }
    }
    candidates.sort();
    candidates.dedup();
    match candidates.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err(NotFound {
            message: format!(
                "{LIBHFUZZ_ENV} names {} but no {stem}.a sits beside it — \
                 `instrument.o` needs its log and file helpers",
                libhfuzz.display()
            ),
        }),
        _ => Err(NotFound {
            message: format!(
                "{LIBHFUZZ_ENV} names {} but its companion {stem}.a is ambiguous: {} — \
                 place the matching archive pair alone in one directory, or unset \
                 {LIBHFUZZ_ENV} to resolve both through hfuzz-clang",
                libhfuzz.display(),
                candidates
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }),
    }
}

fn transport_flag(transport: Transport) -> &'static str {
    match transport {
        Transport::HfAuto | Transport::Stdin => "-s",
        Transport::HfPersistent => "-P",
        Transport::SharedMemory | Transport::InProcess => {
            unreachable!("honggfuzz drives neither shared memory nor an in-process loop")
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Honggfuzz {
    mode: HonggfuzzMode,
}

impl Honggfuzz {
    pub const fn new(mode: HonggfuzzMode) -> Self {
        Self { mode }
    }

    pub const fn auto() -> Self {
        Self::new(HonggfuzzMode::Auto)
    }

    pub const fn persistent() -> Self {
        Self::new(HonggfuzzMode::Persistent)
    }

    pub const fn stdin() -> Self {
        Self::new(HonggfuzzMode::Stdin)
    }

    const fn transport(self) -> Transport {
        self.mode.transport()
    }

    const fn binary_marker(self) -> Option<BinaryMarker> {
        match self.mode {
            HonggfuzzMode::Auto => Some(BinaryMarker {
                bytes: PERSISTENT_SIGNATURE,
            }),
            HonggfuzzMode::Persistent | HonggfuzzMode::Stdin => None,
        }
    }
}

impl Fuzzer for Honggfuzz {
    fn name(&self) -> &'static str {
        NAME
    }

    fn mode(&self) -> Option<ExternalMode> {
        Some(match self.mode {
            HonggfuzzMode::Auto => ExternalMode::HonggfuzzAuto,
            HonggfuzzMode::Persistent => ExternalMode::HonggfuzzPersistent,
            HonggfuzzMode::Stdin => ExternalMode::HonggfuzzStdin,
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
            transport: self.transport(),
            austen_coverage: false,
            blind: true,
        }
    }

    fn coverage(&self) -> &'static [Coverage] {
        if INSTRUMENT {
            &[
                Coverage::TracePcGuard,
                Coverage::SancovCounters,
                Coverage::TraceCmp,
                Coverage::LegacyTraceCmp,
                Coverage::TraceDiv,
                Coverage::TraceGep,
                Coverage::IndirectCalls,
                Coverage::LegacyIndirectCalls,
                Coverage::StackDepth,
                Coverage::TracePc,
                Coverage::FunctionTrace,
            ]
        } else {
            &[]
        }
    }

    fn rejected_coverage(&self) -> &'static [Coverage] {
        &[]
    }

    fn persistent_hooks(&self) -> Option<PersistentHookSymbols> {
        (INSTRUMENT
            && matches!(
                self.transport(),
                Transport::HfAuto | Transport::HfPersistent
            ))
        .then_some(PersistentHookSymbols {
            clear_new_coverage: "instrumentClearNewCov",
            reset_local_coverage: "instrumentResetLocalCovFeedback",
            reset_stack_depth: "instrumentResetStackDepth",
            count_counters: "instrument8BitCountersCount",
            check_stack_depth: "instrumentCheckStackDepth",
        })
    }

    fn preflight_harness(&self, harness: &Path) -> Result<(), ExternalError> {
        if self.mode != HonggfuzzMode::Stdin {
            return Ok(());
        }
        let advertises = advertises_persistence(harness).map_err(|source| ExternalError::Io {
            path: harness.to_path_buf(),
            source,
        })?;
        if !advertises {
            return Ok(());
        }
        Err(ExternalError::Unrunnable {
            engine: Fuzzer::name(self),
            display_name: Fuzzer::display_name(self),
            why: "forced stdin was selected, but this harness advertises honggfuzz \
                  persistence and honggfuzz has no option to disable its automatic \
                  signature detection — rebuild the harness with \
                  `--engine=honggfuzz --honggfuzz-mode=stdin`, or use the default \
                  automatic mode"
                .to_string(),
        })
    }

    fn driver_symbol(&self) -> Option<&'static str> {
        None
    }

    fn compile_request(&self) -> CompileRequest {
        if INSTRUMENT {
            COMPILE
        } else {
            CompileRequest::NONE
        }
    }

    fn target_compiler(&self) -> Result<Option<TargetCompiler>, NotFound> {
        if !INSTRUMENT {
            return Ok(None);
        }
        let clang = self.clang()?;
        Ok(Some(TargetCompiler {
            program: clang,
            request: COMPILE,
        }))
    }

    fn harness_link(&self) -> Result<Option<HarnessLink>, NotFound> {
        if !INSTRUMENT {
            return Ok(self.binary_marker().map(|marker| HarnessLink {
                linker: None,
                link_args: Vec::new(),
                driver_symbol: None,
                persistent_hooks: None,
                binary_marker: Some(marker),
            }));
        }
        let (libhfuzz, libhfcommon) = hfuzz_archives(|| self.hfuzz_clang())?;
        Ok(Some(HarnessLink {
            linker: None,
            link_args: vec![
                LinkArg::Flag(format!("-Wl,--undefined={MODULE_ANCHOR}")),
                LinkArg::Path(libhfuzz),
                LinkArg::Path(libhfcommon),
                LinkArg::Flag("-lgcc".to_string()),
            ],
            driver_symbol: None,
            persistent_hooks: Fuzzer::persistent_hooks(self),
            binary_marker: self.binary_marker(),
        }))
    }

    fn run_plan(&self, req: &RunRequest<'_>) -> RunPlan {
        plan_for(self.transport(), req)
    }

    fn parse_stats(&self, text: &str) -> Stats {
        parse_csv_stats(text, "total_exec", "exec_per_sec")
    }

    fn discovery(&self) -> Discovery {
        Discovery {
            var: HONGGFUZZ_ENV,
            exe: PROGRAM,
            earlier: &[Tier::Baked(VENDORED)],
            remedy: format!(
                "reinstall austen with `--features honggfuzz-vendored` to build it, set \
                 {HONGGFUZZ_ENV} to a honggfuzz binary, or put one on PATH"
            ),
        }
    }
}

impl Honggfuzz {
    fn hfuzz_clang(&self) -> Result<PathBuf, NotFound> {
        let fuzz = self.discover()?;
        let beside = fuzz.with_file_name(HFUZZ_CLANG);
        if is_executable(&beside) {
            return Ok(beside);
        }
        on_path(HFUZZ_CLANG).ok_or_else(|| NotFound {
            message: format!(
                "{HFUZZ_CLANG} not found beside {} or on PATH — it is what \
                 carries libhfuzz.a, so coverage-guided honggfuzz needs it. \
                 Set {LIBHFUZZ_ENV} to a libhfuzz.a to skip it",
                fuzz.display()
            ),
        })
    }

    fn clang(&self) -> Result<PathBuf, NotFound> {
        discover_tool(
            HONGGFUZZ_CLANG_ENV,
            CLANG,
            &[],
            &format!(
                "install clang, put it on PATH, or set {HONGGFUZZ_CLANG_ENV} to \
                 one — gcc cannot be used, having no -fsanitize-coverage=trace-pc-guard"
            ),
        )
    }
}

fn plan_for(transport: Transport, req: &RunRequest<'_>) -> RunPlan {
    let out = req.out_dir;
    let corpus = out.join("corpus");
    let crashes = out.join("crashes");
    let stats = out.join("honggfuzz.stats");
    let report = out.join(REPORT_FILE);
    let fallback = out.join(STDIN_FALLBACK_FILE);

    let mut argv = vec![
        transport_flag(transport).to_string(),
        if req.instrumented { "-z" } else { "-x" }.to_string(),
        "-v".to_string(),
        "-i".to_string(),
        req.seeds_dir.display().to_string(),
        "-o".to_string(),
        corpus.display().to_string(),
        "-W".to_string(),
        out.display().to_string(),
        "--crashdir".to_string(),
        crashes.display().to_string(),
        "--statsfile".to_string(),
        stats.display().to_string(),
        "-R".to_string(),
        report.display().to_string(),
        "-n".to_string(),
        "1".to_string(),
        "-F".to_string(),
        austen_lib::wire::EXTERNAL_INPUT_BYTES.to_string(),
        "-t".to_string(),
        "5".to_string(),
    ];
    if let Some(secs) = req.duration_secs {
        argv.extend(["--run_time".to_string(), secs.to_string()]);
    }
    argv.extend(["--".to_string(), req.harness.display().to_string()]);

    let env = harness_env(req);
    let mut transport_env = Vec::new();
    let mode_fallback = match transport {
        Transport::HfAuto => {
            transport_env.push((austen_lib::wire::HF_AUTO_ENV.to_string(), "1".to_string()));
            transport_env.push((
                austen_lib::wire::HF_STDIN_FALLBACK_ENV.to_string(),
                fallback.display().to_string(),
            ));
            Some(ModeFallback {
                marker: fallback,
                from: ExternalMode::HonggfuzzAuto,
                to: ExternalMode::HonggfuzzStdin,
                message: "honggfuzz did not recognize the harness's persistent signature; continuing with one process per input",
            })
        }
        Transport::HfPersistent => {
            transport_env.push((
                austen_lib::wire::HF_PERSISTENT_ENV.to_string(),
                "1".to_string(),
            ));
            None
        }
        Transport::Stdin => None,
        Transport::SharedMemory | Transport::InProcess => {
            unreachable!("honggfuzz plan received a transport it cannot launch")
        }
    };

    RunPlan {
        argv,
        env,
        transport_env,
        mode_fallback,
        dirs: vec![out.to_path_buf(), corpus.clone(), crashes.clone()],
        crashes_dir: crashes,
        corpus_dir: Some(corpus),
        not_findings: &[REPORT_FILE],
        stats: StatsSource::ToolFile(stats),
    }
}
