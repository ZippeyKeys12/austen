use std::ops::Deref;

use crate::gen_dsl::lang::sym::{Sym, SymMap};
use crate::gen_dsl::lang::value::Value;

#[derive(Debug, Default)]
pub struct Env {
    bindings: SymMap<Value>,
    journal: Vec<(Sym, Option<Value>)>,
}

impl Env {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, sym: &Sym, value: Value) {
        let old = self.bindings.insert(sym.clone(), value);
        self.journal.push((sym.clone(), old));
    }

    #[must_use]
    pub fn mark(&self) -> EnvMark {
        EnvMark(self.journal.len())
    }

    pub fn rewind(&mut self, mark: EnvMark) {
        while self.journal.len() > mark.0 {
            let (sym, old) = self.journal.pop().expect("journal longer than the mark");
            match old {
                Some(v) => self.bindings.insert(sym, v),
                None => self.bindings.remove(&sym),
            };
        }
    }

    pub fn into_bindings(self) -> SymMap<Value> {
        self.bindings
    }

    pub fn scoped<T>(&mut self, sym: &Sym, value: Value, f: impl FnOnce(&mut Self) -> T) -> T {
        let mark = self.mark();
        self.insert(sym, value);
        let out = f(self);
        self.rewind(mark);
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EnvMark(usize);

impl Deref for Env {
    type Target = SymMap<Value>;

    fn deref(&self) -> &Self::Target {
        &self.bindings
    }
}
