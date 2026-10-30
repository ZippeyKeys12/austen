use crate::forkserver_env;
use austen_lib::external::{AnyFuzzer, Discovery, Tier};

use crate::reduction_timeout_ms;
use austen_lib::shrink::{
    HarnessKind, Program, ShrinkPlan, ShrinkRequest, ShrinkTool, harness_env,
};

pub const NAME: &str = "afl-tmin";

const SLUG: &str = "afl-tmin";

const EXE: &str = "afl-tmin";

pub const AFL_TMIN_ENV: &str = "AUSTEN_AFL_TMIN";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AflTmin;

impl ShrinkTool for AflTmin {
    fn name(&self) -> &'static str {
        NAME
    }

    fn drives(&self) -> HarnessKind {
        HarnessKind::Forkserver
    }

    fn builds_with(&self) -> Option<AnyFuzzer> {
        None
    }

    fn shrink_plan(&self, req: &ShrinkRequest<'_>) -> ShrinkPlan {
        let out = req.work.join(SLUG).join("minimized");

        let argv = vec![
            "-i".to_string(),
            req.input.display().to_string(),
            "-o".to_string(),
            out.display().to_string(),
            "-t".to_string(),
            reduction_timeout_ms().to_string(),
            "--".to_string(),
            req.harness.display().to_string(),
        ];

        let mut env = harness_env(req);
        env.extend(forkserver_env());

        ShrinkPlan {
            program: Program::Tool,
            argv,
            env,
            out,
        }
    }

    fn discovery(&self) -> Discovery {
        Discovery {
            var: AFL_TMIN_ENV,
            exe: EXE,
            earlier: &[Tier::Beside(crate::DISCOVERY)],
            remedy: format!(
                "reinstall austen with `--features aflpp-vendored` to build it, set \
                 {AFL_TMIN_ENV} to an afl-tmin binary, or put one on PATH"
            ),
        }
    }
}
