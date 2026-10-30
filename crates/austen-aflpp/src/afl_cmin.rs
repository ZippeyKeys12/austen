use std::path::PathBuf;

use crate::{discover_showmap, one_shot_env};
use austen_lib::external::{Discovery, NotFound, Tier, harness_env_for};

use crate::reduction_timeout_ms;
use austen_lib::shrink::{SuitePlan, SuitePlanError, SuiteRequest, SuiteTool, TRUST};

pub const NAME: &str = "afl-cmin";

const EXE: &str = "afl-cmin";

pub const AFL_CMIN_ENV: &str = "AUSTEN_AFL_CMIN";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AflCmin;

impl SuiteTool for AflCmin {
    fn name(&self) -> &'static str {
        NAME
    }

    fn suite_plan(&self, req: &SuiteRequest<'_>) -> Result<SuitePlan, SuitePlanError> {
        if !req.checked_coverage_harness.is_absolute() {
            return Err(SuitePlanError::HarnessNotAbsolute {
                path: req.checked_coverage_harness.to_path_buf(),
            });
        }

        let mut argv = vec![
            "-i".to_string(),
            req.input_dir.display().to_string(),
            "-o".to_string(),
            req.output_dir.display().to_string(),
            "-t".to_string(),
            reduction_timeout_ms().to_string(),
            "-T".to_string(),
            "1".to_string(),
            "-e".to_string(),
        ];
        if req.crashes_only {
            argv.push("-C".to_string());
        }
        argv.extend([
            "--".to_string(),
            req.checked_coverage_harness.display().to_string(),
        ]);

        let mut env = harness_env_for(
            req.generator,
            TRUST,
            req.heap_bytes,
            req.tally,
            req.allow_unstable_addresses,
        );
        env.extend(one_shot_env());

        Ok(SuitePlan { argv, env })
    }

    fn discovery(&self) -> Discovery {
        Discovery {
            var: AFL_CMIN_ENV,
            exe: EXE,
            earlier: &[Tier::Beside(crate::DISCOVERY)],
            remedy: format!(
                "reinstall austen with `--features aflpp-vendored` to build it, set \
                 {AFL_CMIN_ENV} to an afl-cmin executable, or put one on PATH"
            ),
        }
    }

    fn preflight(&self) -> Result<PathBuf, NotFound> {
        let cmin = self.discover()?;
        discover_showmap(&cmin)?;
        Ok(cmin)
    }
}
