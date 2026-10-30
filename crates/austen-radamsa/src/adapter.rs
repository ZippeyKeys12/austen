use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use austen_lib::gen_dsl::passes::PassProfile;
use austen_lib::input::{
    GenerateCx, GeneratePoll, Generator, GeneratorError, Rejections, StopReason,
};
use austen_lib::observe::{Reporter, RunEvent};
use austen_lib::tools::launch_seeds::{
    CandidateGate, clear_managed_dir, collect_seed_candidates, seed_input_paths, write_launch_seeds,
};
use austen_lib::tools::path_safety::{Overlap, PathTraversal};
use austen_lib::tools::process::POLL_SLEEP;
use austen_lib::wire::Trust;

use crate::{FramePoll, Radamsa, RadamsaError, RadamsaSession, SUPPORTED_VERSION};

const NAME: &str = "radamsa";
const DISPLAY_NAME: &str = "Radamsa";
const LAUNCH_SEEDS: &str = "radamsa-launch-seeds";
const OVERSIZED_OUTPUT: &str = "oversized-output";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadamsaFactory;

pub struct RadamsaPrepare<'a> {
    pub work: &'a Path,
    pub seed: u64,
    pub function_ids: &'a [u8],
    pub seed_sources: &'a [PathBuf],
    pub protected_paths: &'a [(&'static str, &'a Path)],
    pub reporter: &'a Reporter,
    pub candidate_gate: &'a dyn CandidateGate,
}

pub struct PreparedRadamsa {
    tool: Radamsa,
    seed_directory: PathBuf,
    seed: u64,
}

struct LiveRadamsa {
    session: RadamsaSession,
    oversized: u64,
}

fn tool_failure(error: impl std::fmt::Display) -> GeneratorError {
    GeneratorError::Tool(error.to_string())
}

fn validate_launch_directory(
    context: &RadamsaPrepare<'_>,
    tool: &Radamsa,
    launch: &Path,
) -> Result<(), GeneratorError> {
    let managed = PathTraversal::inspect(launch).map_err(tool_failure)?;
    if managed.is_filesystem_root() {
        return Err(tool_failure(format!(
            "refusing to use filesystem root {} as the managed Radamsa launch-seed directory",
            launch.display()
        )));
    }

    let protected = context
        .protected_paths
        .iter()
        .map(|&(role, path)| (role, Cow::Borrowed(path)))
        .chain(seed_input_paths(context.work, context.seed_sources))
        .chain([("Radamsa executable", Cow::Borrowed(tool.executable()))]);
    managed.disjoint_from(protected).map_err(|overlap| {
        tool_failure(match overlap {
            Overlap::Unreadable { path, source } => format!("{}: {source}", path.display()),
            Overlap::Conflict { role, path } => format!(
                "managed Radamsa launch-seed directory {} overlaps configured {role} path {}",
                launch.display(),
                path.display()
            ),
        })
    })
}

impl RadamsaFactory {
    pub fn name(&self) -> &'static str {
        NAME
    }

    pub fn display_name(&self) -> &'static str {
        DISPLAY_NAME
    }

    pub fn profiles(&self) -> Vec<PassProfile> {
        vec![PassProfile::NONE]
    }

    pub fn prepare(&self, context: RadamsaPrepare<'_>) -> Result<PreparedRadamsa, GeneratorError> {
        let tool = Radamsa::discover().map_err(tool_failure)?;
        context.reporter.emit(|| RunEvent::ToolSelected {
            name: NAME.to_string(),
            path: tool.executable().to_path_buf(),
            version: Some(SUPPORTED_VERSION.to_string()),
        });
        let seed_directory =
            std::path::absolute(context.work.join(LAUNCH_SEEDS)).map_err(tool_failure)?;
        validate_launch_directory(&context, &tool, &seed_directory)?;

        clear_managed_dir(&seed_directory).map_err(tool_failure)?;
        let candidates =
            collect_seed_candidates(context.work, context.function_ids, context.seed_sources)
                .map_err(tool_failure)?;
        let canonical = context
            .candidate_gate
            .validate(&candidates)
            .map_err(tool_failure)?;
        write_launch_seeds(&seed_directory, &candidates, &canonical).map_err(tool_failure)?;

        Ok(PreparedRadamsa {
            tool,
            seed_directory,
            seed: context.seed,
        })
    }
}

impl PreparedRadamsa {
    pub fn name(&self) -> &'static str {
        NAME
    }

    pub fn trust(&self) -> Trust {
        Trust::Checked
    }

    pub fn open(self) -> Result<Box<dyn Generator>, GeneratorError> {
        let session = self
            .tool
            .start(&self.seed_directory, self.seed)
            .map_err(tool_failure)?;
        Ok(Box::new(LiveRadamsa {
            session,
            oversized: 0,
        }))
    }
}

impl Generator for LiveRadamsa {
    fn name(&self) -> &'static str {
        NAME
    }

    fn trust(&self) -> Trust {
        Trust::Checked
    }

    fn poll_next(&mut self, cx: GenerateCx<'_>) -> Result<GeneratePoll, GeneratorError> {
        let next = poll_input(cx, || self.session.poll_frame(), std::thread::sleep)?;
        if matches!(next, GeneratePoll::Rejected(OVERSIZED_OUTPUT)) {
            self.oversized = self.oversized.saturating_add(1);
        }
        Ok(next)
    }

    fn replay_remaining(&self) -> Option<usize> {
        None
    }

    fn rejections(&self) -> Rejections {
        Rejections::from_pairs([(OVERSIZED_OUTPUT, self.oversized)])
    }

    fn finish(&mut self, _reason: StopReason) -> Result<(), GeneratorError> {
        self.session.finish().map_err(tool_failure)
    }
}

fn poll_input(
    cx: GenerateCx<'_>,
    mut poll_frame: impl FnMut() -> Result<FramePoll, RadamsaError>,
    mut wait: impl FnMut(Duration),
) -> Result<GeneratePoll, GeneratorError> {
    loop {
        if cx.cancel().is_cancelled() || Instant::now() >= cx.wake_at() {
            return Ok(GeneratePoll::Pending);
        }
        match poll_frame().map_err(tool_failure)? {
            FramePoll::Input(input) => return Ok(GeneratePoll::Input(input)),
            FramePoll::Oversized => return Ok(GeneratePoll::Rejected(OVERSIZED_OUTPUT)),
            FramePoll::Progress => {}
            FramePoll::Pending => {
                let now = Instant::now();
                if cx.cancel().is_cancelled() || now >= cx.wake_at() {
                    return Ok(GeneratePoll::Pending);
                }
                wait(POLL_SLEEP.min(cx.wake_at().duration_since(now)));
            }
        }
    }
}
