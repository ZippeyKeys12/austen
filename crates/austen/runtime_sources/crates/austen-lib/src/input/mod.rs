mod functions;
mod selector;

pub use functions::TreeGenerator;
pub use selector::{
    ConstantSizeSelector, DEFAULT_DISCARD_FACTOR, DefaultSizeSelector, EscalationMemberSelector,
    FunctionSelector, FunctionSelectorKind, MemberOutcome, MemberSelector, QuickCheckSizeSelector,
    RandomFunctionSelector, RoundRobinFunctionSelector, SizeOutcome, SizeSelector,
    SizeSelectorKind, UniformSizeSelector,
};

mod rejections;
mod stats;

pub use rejections::{Rejections, backtrack_total};
pub use stats::{AttemptStats, FunctionStats, GAVE_UP_ATTEMPTS, MemberStats, SizeStats};

pub const RETRIES_EXHAUSTED: &str = "retries-exhausted";

use std::time::Instant;

use crate::gen_dsl::GenFailure;
use crate::observe::Cancel;
use crate::wire::{AustenInput, Trust};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FunctionSelectionError {
    #[error(
        "recorded input has only {available} of the 8 bytes required for its function-selection marker"
    )]
    Truncated { available: usize },
    #[error("function index {index} is out of range (active function count: {functions})")]
    OutOfRange { index: u64, functions: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SizeSelectionError {
    #[error(
        "recorded input has only {available} of the 8 bytes required for its test-case-size marker"
    )]
    Truncated { available: usize },
    #[error("recorded test-case size {size} is outside 1..={max}", max = crate::gen_dsl::TestCaseSize::MAX.get())]
    OutOfRange { size: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MemberSelectionError {
    #[error(
        "recorded input has only {available} of the 8 bytes required for its engine-member marker"
    )]
    Truncated { available: usize },
    #[error("engine member index {index} is out of range (chain member count: {members})")]
    OutOfRange { index: u64, members: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GeneratorError {
    #[error(
        "custom generator {entry:?} from {file} produced an incompatible tree for {target}: {reason}"
    )]
    CustomTree {
        file: std::path::PathBuf,
        entry: crate::gen_dsl::Sym,
        target: String,
        reason: String,
    },
    #[error("recorded input has only {available} of the 8 bytes required for its default seed")]
    DefaultSeedTruncated { available: usize },
    #[error(transparent)]
    Defaults(#[from] crate::gen_dsl::DefaultError),
    #[error(transparent)]
    FunctionSelection(#[from] FunctionSelectionError),
    #[error(transparent)]
    SizeSelection(#[from] SizeSelectionError),
    #[error(transparent)]
    MemberSelection(#[from] MemberSelectionError),
    #[error(transparent)]
    Generation(#[from] GenFailure),
    #[error("{0}")]
    Tool(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Budget,
    Finding,
    Cancelled,
    Failure,
}

pub struct GenerateCx<'a> {
    cancel: &'a Cancel,
    wake_at: Instant,
    deadline: Option<Instant>,
    replay: bool,
}

impl<'a> GenerateCx<'a> {
    pub fn new(cancel: &'a Cancel, wake_at: Instant, replay: bool) -> Self {
        Self {
            cancel,
            wake_at,
            deadline: None,
            replay,
        }
    }

    pub fn cancel(&self) -> &Cancel {
        self.cancel
    }

    pub fn wake_at(&self) -> Instant {
        self.wake_at
    }

    pub fn is_replay(&self) -> bool {
        self.replay
    }

    pub fn with_deadline(mut self, deadline: Option<Instant>) -> Self {
        self.deadline = if self.replay { None } else { deadline };
        self
    }

    pub fn is_stopped(&self) -> bool {
        self.cancel.is_cancelled()
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeneratePoll {
    Input(AustenInput),
    Rejected(&'static str),
    Pending,
    Interrupted,
    Exhausted,
}

pub trait Generator {
    fn name(&self) -> &'static str;

    fn poll_next(&mut self, cx: GenerateCx<'_>) -> Result<GeneratePoll, GeneratorError>;

    fn trust(&self) -> Trust;

    fn rejections(&self) -> Rejections;

    fn per_function(&self) -> Option<Vec<FunctionStats>> {
        None
    }

    fn last_attempt(&self) -> Option<&AttemptStats> {
        None
    }

    fn replay_remaining(&self) -> Option<usize>;

    fn finish(&mut self, _reason: StopReason) -> Result<(), GeneratorError> {
        Ok(())
    }
}
