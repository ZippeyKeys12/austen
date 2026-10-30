use std::collections::VecDeque;

use crate::gen_dsl::{GenDef, Sym, SymSet};

use crate::spec::convert::ConvertError;

#[derive(Debug, Default)]
pub struct Builder {
    defs: Vec<(Sym, GenDef)>,
    requested: SymSet,
    queue: VecDeque<Sym>,
}

impl Builder {
    pub fn new() -> Self {
        Builder::default()
    }

    pub fn request(&mut self, fsym: &Sym) {
        if self.requested.insert(fsym.clone()) {
            self.queue.push_back(fsym.clone());
        }
    }

    pub fn add(&mut self, def: GenDef) -> Result<(), ConvertError> {
        if self.defs.iter().any(|(name, _)| *name == def.name) {
            return Err(ConvertError::DuplicateGenerator {
                name: def.name.clone(),
            });
        }
        self.defs.push((def.name.clone(), def));
        Ok(())
    }

    pub fn next_request(&mut self) -> Option<Sym> {
        self.queue.pop_front()
    }

    pub fn into_defs(self) -> Vec<(Sym, GenDef)> {
        self.defs
    }
}
