use crate::gen_dsl::lang::base_type::{Sign, normalise_to_range};
use crate::gen_dsl::rand_like::{Rng, lemire};
use serde::{Deserialize, Serialize};

pub const DEFAULT_MAX_SIZE: u64 = 100;

pub const MAX_SIZE_LIMIT: u64 = 1 << 32;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct TestCaseSize(u64);

impl TestCaseSize {
    pub const MIN: Self = Self(1);

    pub const MAX: Self = Self(MAX_SIZE_LIMIT);

    pub const fn new(value: u64) -> Option<Self> {
        if value >= Self::MIN.0 && value <= Self::MAX.0 {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn sample_uniform<R: Rng + ?Sized>(rng: &mut R, max: u64) -> Self {
        let value = uniform_in(rng, 1, max as i128) as u64;
        Self::new(value).expect("max is clamped to 1..=MAX_SIZE_LIMIT by every caller")
    }
}

impl TryFrom<u64> for TestCaseSize {
    type Error = TestCaseSizeError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value).ok_or(TestCaseSizeError { value })
    }
}

impl From<TestCaseSize> for u64 {
    fn from(size: TestCaseSize) -> Self {
        size.get()
    }
}

impl std::fmt::Display for TestCaseSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("test-case size {value} is outside 1..={MAX_SIZE_LIMIT}")]
pub struct TestCaseSizeError {
    pub value: u64,
}

pub fn uniform_in<R: Rng + ?Sized>(rng: &mut R, lo: i128, hi: i128) -> i128 {
    debug_assert!(lo <= hi);
    let span = (hi - lo + 1) as u64;
    lo + i128::from(lemire::uniform_u64(rng, span))
}

pub fn sized_bits<R: Rng + ?Sized>(rng: &mut R, size: u64, sign: Sign, width: u16) -> Option<i128> {
    let size = i128::from(size);
    if width == 0 {
        return Some(uniform_in(rng, 0, 0));
    }
    match sign {
        Sign::Unsigned => {
            let hi = if width >= 64 {
                size - 1
            } else {
                (size - 1).min((1i128 << width) - 1)
            };
            Some(uniform_in(rng, 0, hi))
        }
        Sign::Signed => {
            if width >= 127 {
                return None;
            }
            let half = 1i128 << (width - 1);
            let lo = (-(size - 1)).max(-half);
            let hi = (size - 1).min(half - 1);
            let drawn = uniform_in(rng, lo, hi);
            normalise_to_range(Sign::Unsigned, width, drawn)
        }
    }
}

pub fn sized_bits_with_extrema_skew<R: Rng + ?Sized>(
    rng: &mut R,
    size: u64,
    sign: Sign,
    width: u16,
) -> Option<i128> {
    let extremum_fits = match sign {
        Sign::Unsigned => width < 128,
        Sign::Signed => width < 127,
    };
    if size > 64 || width == 0 || !extremum_fits {
        return sized_bits(rng, size, sign, width);
    }
    let span = 1u64 << (size / 2 + 1);
    if lemire::uniform_u64(rng, span) != 0 {
        return sized_bits(rng, size, sign, width);
    }
    Some(match sign {
        Sign::Unsigned => ((1u128 << width) - 1) as i128,
        Sign::Signed => {
            let half = 1i128 << (width - 1);
            if lemire::uniform_u64(rng, 2) == 0 {
                half
            } else {
                half - 1
            }
        }
    })
}
