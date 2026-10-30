mod source;
mod tape;

pub use source::{Blind, DrawSource, Site};
pub use tape::{ChoiceTape, DrawTape, site_bytes};

use crate::gen_dsl::choice::draws::source::Then;
use crate::gen_dsl::choice::seq::Bv;
use crate::gen_dsl::rand_like::{Rng, SplitMix64, lemire};

pub const DEFAULT_SAMPLE_SEED: u64 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Draw {
    Value(i128),
    Bits(u128),
}

impl Draw {
    pub fn at(self, width: u16) -> Bv {
        match self {
            Draw::Value(value) => Bv { value, bits: width },
            Draw::Bits(bits) => Bv {
                value: keep_low(bits, width),
                bits: width,
            },
        }
    }
}

pub fn of_values(values: &[i128]) -> impl Iterator<Item = Draw> + '_ {
    values.iter().copied().map(Draw::Value)
}

pub fn sampled(seed: u64) -> impl DrawSource {
    Sampler {
        rng: SplitMix64::new(seed),
    }
}

struct Sampler {
    rng: SplitMix64,
}

impl DrawSource for Sampler {
    fn draw(&mut self, site: Site) -> Option<Draw> {
        Some(Draw::Bits(uniform_bits(&mut self.rng, site.width)))
    }
}

fn uniform_bits<R: Rng>(rng: &mut R, width: u16) -> u128 {
    match width.min(DRAW_BITS) {
        0 => 0,
        keep if keep <= 64 => u128::from(chunk(rng, keep)),
        keep => u128::from(chunk(rng, 64)) | (u128::from(chunk(rng, keep - 64)) << 64),
    }
}

fn chunk<R: Rng>(rng: &mut R, bits: u16) -> u64 {
    lemire::uniform_u64(rng, if bits == 64 { 0 } else { 1u64 << bits })
}

pub fn endless<S: DrawSource>(source: S) -> impl DrawSource {
    Then::new(source, sampled(DEFAULT_SAMPLE_SEED))
}

const DRAW_BITS: u16 = 127;

fn keep_low(bits: u128, width: u16) -> i128 {
    let keep = width.min(DRAW_BITS);
    if keep == 0 {
        return 0;
    }
    (bits & ((1u128 << keep) - 1)) as i128
}
