use crate::{EXIT_CRASH, EXIT_TIMEOUT, LibFuzzer, RSS_LIMIT_MB};
use austen_lib::external::{AnyFuzzer, Discovery, Fuzzer};
use austen_lib::wire::{INPUT_TIMEOUT, LIBFUZZER_ENV};

use austen_lib::shrink::{
    HarnessKind, Program, ShrinkPlan, ShrinkRequest, ShrinkTool, harness_env,
};

pub const NAME: &str = "libfuzzer";

const SLUG: &str = "libfuzzer-min";

const RUNS: u64 = 10_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LibFuzzerMinimize;

static BUILD_ENGINE: LibFuzzer = LibFuzzer::with_fork(false);

impl ShrinkTool for LibFuzzerMinimize {
    fn name(&self) -> &'static str {
        NAME
    }

    fn drives(&self) -> HarnessKind {
        HarnessKind::InProcess
    }

    fn builds_with(&self) -> Option<AnyFuzzer> {
        Some(AnyFuzzer::new(&BUILD_ENGINE))
    }

    fn shrink_plan(&self, req: &ShrinkRequest<'_>) -> ShrinkPlan {
        let out = req.work.join(SLUG).join("minimized");

        let mut argv = vec![
            "-minimize_crash=1".to_string(),
            format!("-exact_artifact_path={}", out.display()),
            format!("-runs={RUNS}"),
            format!("-timeout={}", INPUT_TIMEOUT.as_secs()),
            format!("-rss_limit_mb={RSS_LIMIT_MB}"),
            "-detect_leaks=0".to_string(),
            format!("-error_exitcode={EXIT_CRASH}"),
            format!("-timeout_exitcode={EXIT_TIMEOUT}"),
        ];
        if let Some(secs) = req.budget_secs {
            argv.push(format!("-max_total_time={secs}"));
        }
        argv.push(req.input.display().to_string());

        let mut env = harness_env(req);
        env.push((LIBFUZZER_ENV.to_string(), "1".to_string()));

        ShrinkPlan {
            program: Program::Harness,
            argv,
            env,
            out,
        }
    }

    fn discovery(&self) -> Discovery {
        Fuzzer::discovery(&LibFuzzer::default())
    }
}
