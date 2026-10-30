mod afl_cmin;
mod afl_tmin;
pub use afl_cmin::{AFL_CMIN_ENV, AflCmin};
pub use afl_tmin::{AFL_TMIN_ENV, AflTmin};

fn reduction_timeout_ms() -> u64 {
    austen_lib::wire::INPUT_TIMEOUT.as_millis() as u64
}

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use austen_lib::instrumentation::{CompileRequest, Coverage};
use austen_lib::tools::discovery::{DiscoveryAnchor, DiscoveryProvider};
use austen_lib::wire::{AFL_NO_FORKSRV_ENV, PERSISTENT_ENV, PERSISTENT_YES};

use austen_lib::external::{
    Capabilities, Discovery, ExternalMode, Fuzzer, NotFound, PersistentHookSymbols, RunPlan,
    RunRequest, Stats, StatsSource, TargetCompiler, Tier, Transport, harness_env, is_executable,
    on_path, parse_csv_stats,
};

pub const NAME: &str = "afl++";

pub const DISPLAY_NAME: &str = "AFL++";

pub const DISCOVERY: DiscoveryAnchor = DiscoveryAnchor::new(&AflPlusPlus);

impl DiscoveryProvider for AflPlusPlus {
    fn identity(&self) -> &'static str {
        NAME
    }

    fn declaration(&self) -> Discovery {
        Fuzzer::discovery(self)
    }
}

const SLUG: &str = "afl";

pub const AFL_CC: &str = "afl-cc";

const COMPILE: CompileRequest = CompileRequest {
    cflags: &[],
    env: &[("AFL_LLVM_INSTRUMENT", "NATIVE"), ("AFL_QUIET", "1")],
};

pub const AFL_FUZZ_ENV: &str = "AUSTEN_AFL_FUZZ";

const VENDORED: Option<&str> = option_env!("AUSTEN_AFL_FUZZ_VENDORED");

pub fn forkserver_env() -> Vec<(String, String)> {
    afl_env(PERSISTENT_ENV, PERSISTENT_YES)
}

pub fn one_shot_env() -> Vec<(String, String)> {
    afl_env(AFL_NO_FORKSRV_ENV, "1")
}

fn afl_env(mode_var: &str, mode_value: &str) -> Vec<(String, String)> {
    vec![
        ("AFL_SKIP_BIN_CHECK".to_string(), "1".to_string()),
        (mode_var.to_string(), mode_value.to_string()),
        (
            "AFL_I_DONT_CARE_ABOUT_MISSING_CRASHES".to_string(),
            "1".to_string(),
        ),
        ("AFL_SKIP_CPUFREQ".to_string(), "1".to_string()),
    ]
}

pub fn discover_showmap(front_end: &Path) -> Result<PathBuf, NotFound> {
    let current = std::env::current_dir().ok();
    find_showmap(
        front_end,
        std::env::var_os("AFL_PATH").as_deref(),
        current.as_deref(),
        std::env::var_os("PATH").as_deref(),
    )
    .ok_or_else(|| NotFound {
        message: format!(
            "afl-showmap not found for {} — install the matching AFL++ tools beside it, \
             set AFL_PATH to their directory, or put afl-showmap on PATH",
            front_end.display()
        ),
    })
}

fn find_showmap(
    front_end: &Path,
    afl_path: Option<&OsStr>,
    current_dir: Option<&Path>,
    path: Option<&OsStr>,
) -> Option<PathBuf> {
    let beside = front_end.parent().map(|dir| dir.join("afl-showmap"));
    let current = current_dir.map(|dir| dir.join("afl-showmap"));
    afl_path
        .into_iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join("afl-showmap"))
        .chain(beside)
        .chain(current)
        .chain(
            path.into_iter()
                .flat_map(std::env::split_paths)
                .map(|dir| dir.join("afl-showmap")),
        )
        .find(|candidate| is_executable(candidate))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AflPlusPlus;

impl Fuzzer for AflPlusPlus {
    fn name(&self) -> &'static str {
        NAME
    }

    fn mode(&self) -> Option<ExternalMode> {
        None
    }

    fn display_name(&self) -> &'static str {
        DISPLAY_NAME
    }

    fn slug(&self) -> &'static str {
        SLUG
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            transport: Transport::SharedMemory,
            austen_coverage: true,
            blind: true,
        }
    }

    fn coverage(&self) -> &'static [Coverage] {
        &[Coverage::TracePcGuard]
    }

    fn rejected_coverage(&self) -> &'static [Coverage] {
        &[]
    }

    fn persistent_hooks(&self) -> Option<PersistentHookSymbols> {
        None
    }

    fn driver_symbol(&self) -> Option<&'static str> {
        None
    }

    fn compile_request(&self) -> CompileRequest {
        COMPILE
    }

    fn target_compiler(&self) -> Result<Option<TargetCompiler>, NotFound> {
        let fuzz = self.discover()?;
        let beside = fuzz.with_file_name(AFL_CC);
        let program = if is_executable(&beside) {
            beside
        } else if let Some(path) = on_path(AFL_CC) {
            path
        } else {
            return Err(NotFound {
                message: format!(
                    "{AFL_CC} not found beside {} or on PATH — the AFL++ install is incomplete",
                    fuzz.display()
                ),
            });
        };
        Ok(Some(TargetCompiler {
            program,
            request: COMPILE,
        }))
    }

    fn run_plan(&self, req: &RunRequest<'_>) -> RunPlan {
        let mut argv = Vec::new();
        if !req.instrumented {
            argv.push("-n".to_string());
        }
        argv.extend([
            "-i".to_string(),
            req.seeds_dir.display().to_string(),
            "-o".to_string(),
            req.out_dir.display().to_string(),
            "-t".to_string(),
            "2000+".to_string(),
        ]);
        if let Some(secs) = req.duration_secs {
            argv.extend(["-V".to_string(), secs.to_string()]);
        }
        argv.extend(["--".to_string(), req.harness.display().to_string()]);
        let mut transport_env = forkserver_env();
        transport_env.push(("AFL_NO_UI".to_string(), "1".to_string()));
        let dir = if req.instrumented {
            req.out_dir.join("default")
        } else {
            req.out_dir.to_path_buf()
        };
        RunPlan {
            argv,
            env: harness_env(req),
            transport_env,
            mode_fallback: None,
            dirs: Vec::new(),
            crashes_dir: dir.join("crashes"),
            corpus_dir: Some(dir.join("queue")),
            not_findings: &["README.txt"],
            stats: StatsSource::ToolFile(dir.join("plot_data")),
        }
    }

    fn parse_stats(&self, text: &str) -> Stats {
        parse_csv_stats(text, "total_execs", "execs_per_sec")
    }

    fn discovery(&self) -> Discovery {
        Discovery {
            var: AFL_FUZZ_ENV,
            exe: "afl-fuzz",
            earlier: &[Tier::Baked(VENDORED)],
            remedy: format!(
                "reinstall austen with `--features aflpp-vendored` to build it, set \
                 {AFL_FUZZ_ENV} to an afl-fuzz binary, or put one on PATH"
            ),
        }
    }
}
