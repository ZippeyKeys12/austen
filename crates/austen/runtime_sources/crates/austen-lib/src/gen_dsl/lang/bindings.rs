use std::collections::HashMap;

use crate::gen_dsl::lang::sym::{Sym, SymMap};
use crate::gen_dsl::lang::value::Value;

pub const NARROW_BATCH: usize = 256;

pub struct Bindings<'a> {
    node: Node<'a>,
}

enum Node<'a> {
    Root(&'a SymMap<Value>),
    Frame {
        parent: &'a Bindings<'a>,
        sym: &'a Sym,
        value: Value,
    },
    Batch {
        parent: &'a Bindings<'a>,
        group: Group<'a>,
    },
}

enum Group<'a> {
    Few(Vec<(&'a Sym, Value)>),
    Many(HashMap<u64, Value>),
}

impl<'a> Bindings<'a> {
    pub fn root(map: &'a SymMap<Value>) -> Self {
        Bindings {
            node: Node::Root(map),
        }
    }

    pub fn push(&'a self, sym: &'a Sym, value: Value) -> Bindings<'a> {
        Bindings {
            node: Node::Frame {
                parent: self,
                sym,
                value,
            },
        }
    }

    pub fn batch(&'a self, group: Vec<(&'a Sym, Value)>) -> Bindings<'a> {
        let group = if group.len() <= NARROW_BATCH {
            Group::Few(group)
        } else {
            Group::Many(
                group
                    .into_iter()
                    .map(|(sym, value)| (sym.num, value))
                    .collect(),
            )
        };
        Bindings {
            node: Node::Batch {
                parent: self,
                group,
            },
        }
    }

    pub fn get(&self, sym: &Sym) -> Option<&Value> {
        let mut here = self;
        loop {
            match &here.node {
                Node::Root(map) => return map.get(sym),
                Node::Frame {
                    parent,
                    sym: bound,
                    value,
                } => {
                    if *bound == sym {
                        return Some(value);
                    }
                    here = parent;
                }
                Node::Batch { parent, group } => {
                    let found = match group {
                        Group::Few(entries) => entries
                            .iter()
                            .rev()
                            .find(|(bound, _)| *bound == sym)
                            .map(|(_, value)| value),
                        Group::Many(table) => table.get(&sym.num),
                    };
                    if let Some(value) = found {
                        return Some(value);
                    }
                    here = parent;
                }
            }
        }
    }
}
