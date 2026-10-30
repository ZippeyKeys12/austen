mod entropy;
mod splitmix;

pub mod lemire;
pub mod recording;
pub mod sized;

pub use entropy::{Entropy, EntropyMark, InputDecisions, RecordingLog, TruncatedDecision};
pub use recording::RecordingError;
pub use sized::{
    DEFAULT_MAX_SIZE, MAX_SIZE_LIMIT, TestCaseSize, TestCaseSizeError, sized_bits,
    sized_bits_with_extrema_skew, uniform_in,
};
pub use splitmix::SplitMix64;

pub trait Rng {
    fn next_u64(&mut self) -> u64;

    fn decide(&mut self, choose: &mut dyn FnMut() -> bool) -> bool {
        choose()
    }
}

pub trait Rewind: Rng {
    type Mark: Copy;

    fn save(&self) -> Self::Mark;

    fn restore(&mut self, mark: Self::Mark);
}
