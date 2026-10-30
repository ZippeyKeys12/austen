use crate::gen_dsl::{BaseType, Sym};
use serde::{Deserialize, Serialize};

use crate::spec::logical::lat::Lat;

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArgType<I> {
    Computational((Sym, BaseType), Box<ArgType<I>>),
    Ghost((Sym, BaseType), Box<ArgType<I>>),
    L(Lat<I>),
}

impl<I> ArgType<I> {
    pub fn lat(&self) -> &Lat<I> {
        let mut node = self;
        loop {
            node = match node {
                ArgType::Computational(_, rest) | ArgType::Ghost(_, rest) => rest,
                ArgType::L(lat) => return lat,
            };
        }
    }

    pub fn computational(&self) -> Vec<(Sym, BaseType)> {
        let mut out = Vec::new();
        let mut node = self;
        loop {
            node = match node {
                ArgType::Computational(binder, rest) => {
                    out.push(binder.clone());
                    rest
                }
                ArgType::Ghost(_, rest) => rest,
                ArgType::L(_) => return out,
            };
        }
    }
}
