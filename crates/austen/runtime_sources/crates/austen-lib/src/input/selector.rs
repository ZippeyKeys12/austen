use std::num::{NonZeroU64, NonZeroUsize};

use crate::gen_dsl::{
    Rng, TestCaseSize,
    rand_like::{DEFAULT_MAX_SIZE, uniform_in},
};

pub trait FunctionSelector {
    fn select(&mut self, count: NonZeroUsize, rng: &mut dyn Rng) -> usize;
}

pub trait SizeSelector {
    fn select(&mut self, rng: &mut dyn Rng) -> TestCaseSize;

    fn observe(&mut self, _outcome: SizeOutcome) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeOutcome {
    Produced,
    Discarded,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultSizeSelector;

impl SizeSelector for DefaultSizeSelector {
    fn select(&mut self, rng: &mut dyn Rng) -> TestCaseSize {
        UniformSizeSelector::default().select(rng)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UniformSizeSelector {
    max_test_case_size: TestCaseSize,
}

impl UniformSizeSelector {
    pub const fn new(max_test_case_size: TestCaseSize) -> Self {
        Self { max_test_case_size }
    }

    pub const fn max_test_case_size(self) -> TestCaseSize {
        self.max_test_case_size
    }
}

impl Default for UniformSizeSelector {
    fn default() -> Self {
        Self::new(
            TestCaseSize::new(DEFAULT_MAX_SIZE)
                .expect("the historical default size ceiling is valid"),
        )
    }
}

impl SizeSelector for UniformSizeSelector {
    fn select(&mut self, rng: &mut dyn Rng) -> TestCaseSize {
        let size = uniform_in(rng, 1, i128::from(self.max_test_case_size.get())) as u64;
        TestCaseSize::new(size).expect("the configured size interval is valid")
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RandomFunctionSelector;

impl FunctionSelector for RandomFunctionSelector {
    fn select(&mut self, count: NonZeroUsize, rng: &mut dyn Rng) -> usize {
        if count.get() == 1 {
            return 0;
        }
        uniform_in(rng, 0, (count.get() - 1) as i128) as usize
    }
}

#[derive(Debug, Clone, Default)]
pub struct RoundRobinFunctionSelector {
    next: usize,
}

impl FunctionSelector for RoundRobinFunctionSelector {
    fn select(&mut self, count: NonZeroUsize, _rng: &mut dyn Rng) -> usize {
        let selected = self.next % count.get();
        self.next = (selected + 1) % count.get();
        selected
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum FunctionSelectorKind {
    #[default]
    Random,
    RoundRobin,
}

impl FunctionSelectorKind {
    pub const fn token(self) -> &'static str {
        match self {
            FunctionSelectorKind::Random => "random",
            FunctionSelectorKind::RoundRobin => "round-robin",
        }
    }

    pub fn open(self) -> Box<dyn FunctionSelector> {
        match self {
            FunctionSelectorKind::Random => Box::new(RandomFunctionSelector),
            FunctionSelectorKind::RoundRobin => Box::new(RoundRobinFunctionSelector::default()),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SizeSelectorKind {
    #[default]
    Uniform,
    QuickCheck,
    Constant,
}

impl SizeSelectorKind {
    pub const fn token(self) -> &'static str {
        match self {
            SizeSelectorKind::Uniform => "uniform",
            SizeSelectorKind::QuickCheck => "quickcheck",
            SizeSelectorKind::Constant => "constant",
        }
    }

    pub fn open(self, max: TestCaseSize) -> Box<dyn SizeSelector> {
        match self {
            SizeSelectorKind::Uniform => Box::new(UniformSizeSelector::new(max)),
            SizeSelectorKind::QuickCheck => Box::new(QuickCheckSizeSelector::new(max)),
            SizeSelectorKind::Constant => Box::new(ConstantSizeSelector::new(max)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstantSizeSelector {
    max_test_case_size: TestCaseSize,
}

impl ConstantSizeSelector {
    pub const fn new(max_test_case_size: TestCaseSize) -> Self {
        Self { max_test_case_size }
    }
}

impl SizeSelector for ConstantSizeSelector {
    fn select(&mut self, _rng: &mut dyn Rng) -> TestCaseSize {
        self.max_test_case_size
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuickCheckSizeSelector {
    max_test_case_size: TestCaseSize,
    discard_factor: u64,
    successes: u64,
    recent_discards: u64,
}

pub const DEFAULT_DISCARD_FACTOR: u64 = 10;

impl QuickCheckSizeSelector {
    pub const fn new(max_test_case_size: TestCaseSize) -> Self {
        Self {
            max_test_case_size,
            discard_factor: DEFAULT_DISCARD_FACTOR,
            successes: 0,
            recent_discards: 0,
        }
    }

    fn discard_divisor(&self) -> u64 {
        if self.discard_factor == 0 {
            return 1;
        }
        (self.successes.saturating_mul(self.discard_factor) / 3).clamp(1, 10)
    }
}

impl SizeSelector for QuickCheckSizeSelector {
    fn select(&mut self, _rng: &mut dyn Rng) -> TestCaseSize {
        let max = self.max_test_case_size.get();
        let ramp = self.successes % max;
        let widened = ramp.saturating_add(self.recent_discards / self.discard_divisor());
        let size = widened.min(max - 1) + 1;
        TestCaseSize::new(size).expect("a value in 1..=max_test_case_size is a valid size")
    }

    fn observe(&mut self, outcome: SizeOutcome) {
        match outcome {
            SizeOutcome::Produced => {
                self.successes = self.successes.saturating_add(1);
                self.recent_discards = 0;
            }
            SizeOutcome::Discarded => {
                self.recent_discards = self.recent_discards.saturating_add(1);
            }
        }
    }
}

pub trait MemberSelector {
    fn select(&mut self, function: usize, members: NonZeroUsize, rng: &mut dyn Rng) -> usize;

    fn observe(&mut self, _function: usize, _member: usize, _outcome: MemberOutcome) {}

    fn retired(&self, _function: usize, _member: usize, _members: NonZeroUsize) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberOutcome {
    Produced,
    Exhausted,
}

#[derive(Debug, Clone, Default)]
pub struct EscalationMemberSelector {
    discard_limit: Option<NonZeroU64>,
    ledger: Vec<Vec<MemberLedger>>,
}

#[derive(Debug, Clone, Copy, Default)]
struct MemberLedger {
    inputs: u64,
    exhausted: u64,
}

impl MemberLedger {
    fn retired(self, discard_limit: Option<NonZeroU64>) -> bool {
        match discard_limit {
            Some(limit) => self.exhausted >= limit.get(),
            None => self.inputs == 0 && self.exhausted >= super::GAVE_UP_ATTEMPTS,
        }
    }
}

impl EscalationMemberSelector {
    pub const fn with_discard_limit(discard_limit: Option<NonZeroU64>) -> Self {
        Self {
            discard_limit,
            ledger: Vec::new(),
        }
    }

    fn entry(&mut self, function: usize, member: usize) -> &mut MemberLedger {
        if self.ledger.len() <= function {
            self.ledger.resize_with(function + 1, Vec::new);
        }
        let row = &mut self.ledger[function];
        if row.len() <= member {
            row.resize_with(member + 1, MemberLedger::default);
        }
        &mut row[member]
    }

    fn ledger(&self, function: usize, member: usize) -> MemberLedger {
        self.ledger
            .get(function)
            .and_then(|row| row.get(member))
            .copied()
            .unwrap_or_default()
    }
}

impl MemberSelector for EscalationMemberSelector {
    fn select(&mut self, function: usize, members: NonZeroUsize, _rng: &mut dyn Rng) -> usize {
        let last = members.get() - 1;
        (0..last)
            .find(|member| !self.ledger(function, *member).retired(self.discard_limit))
            .unwrap_or(last)
    }

    fn observe(&mut self, function: usize, member: usize, outcome: MemberOutcome) {
        let entry = self.entry(function, member);
        match outcome {
            MemberOutcome::Produced => entry.inputs = entry.inputs.saturating_add(1),
            MemberOutcome::Exhausted => entry.exhausted = entry.exhausted.saturating_add(1),
        }
    }

    fn retired(&self, function: usize, member: usize, members: NonZeroUsize) -> bool {
        member + 1 < members.get() && self.ledger(function, member).retired(self.discard_limit)
    }
}
