use crate::gen_dsl::{BaseType, IndexTerm, LogicalConstraint, Sym, SymMap};
use serde::{Deserialize, Serialize};

use crate::spec::logical::request::Request;

pub type ResourceBinding = (Sym, (Request, BaseType));

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lat<I> {
    Define((Sym, IndexTerm), Box<Lat<I>>),
    Resource(ResourceBinding, Box<Lat<I>>),
    Constraint(LogicalConstraint, Box<Lat<I>>),
    I(I),
}

impl<I> Lat<I> {
    pub fn map<J>(self, f: &mut impl FnMut(I) -> J) -> Lat<J> {
        match self {
            Lat::Define(bound, rest) => Lat::Define(bound, Box::new(rest.map(f))),
            Lat::Resource(bound, rest) => Lat::Resource(bound, Box::new(rest.map(f))),
            Lat::Constraint(lc, rest) => Lat::Constraint(lc, Box::new(rest.map(f))),
            Lat::I(i) => Lat::I(f(i)),
        }
    }

    pub fn resource_requests(&self) -> Vec<&ResourceBinding> {
        let mut out = Vec::new();
        let mut node = self;
        loop {
            node = match node {
                Lat::Resource(binding, rest) => {
                    out.push(binding);
                    rest
                }
                Lat::Define(_, rest) | Lat::Constraint(_, rest) => rest,
                Lat::I(_) => return out,
            };
        }
    }

    pub fn free_vars_bts(&self, i_free_vars: &dyn Fn(&I) -> SymMap<BaseType>) -> SymMap<BaseType> {
        fn union(mut acc: SymMap<BaseType>, other: SymMap<BaseType>) -> SymMap<BaseType> {
            for (sym, bt) in other {
                acc.entry(sym).or_insert(bt);
            }
            acc
        }
        match self {
            Lat::Define((s, it), rest) => {
                let mut tail = rest.free_vars_bts(i_free_vars);
                tail.remove(s);
                union(it.free_vars_bts(), tail)
            }
            Lat::Resource((s, (req, _)), rest) => {
                let mut tail = rest.free_vars_bts(i_free_vars);
                tail.remove(s);
                union(req.free_vars_bts(), tail)
            }
            Lat::Constraint(lc, rest) => union(lc.free_vars_bts(), rest.free_vars_bts(i_free_vars)),
            Lat::I(i) => i_free_vars(i),
        }
    }
}
