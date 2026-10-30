use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sym {
    pub name: Option<String>,
    pub num: u64,
}

impl PartialEq for Sym {
    fn eq(&self, other: &Self) -> bool {
        self.num == other.num
    }
}

impl Eq for Sym {}

impl PartialOrd for Sym {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Sym {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.num.cmp(&other.num)
    }
}

impl std::hash::Hash for Sym {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.num.hash(state);
    }
}

pub type SymSet = BTreeSet<Sym>;
pub type SymMap<V> = BTreeMap<Sym, V>;

#[derive(Debug, Clone)]
pub struct SymGen {
    next: u64,
}

impl SymGen {
    pub fn starting_at(next: u64) -> Self {
        SymGen { next }
    }

    pub fn fresh_anon(&mut self) -> Sym {
        let num = self.next;
        self.next += 1;
        Sym { name: None, num }
    }

    pub fn fresh(&mut self, name: &str) -> Sym {
        let num = self.next;
        self.next += 1;
        Sym {
            name: Some(name.to_owned()),
            num,
        }
    }

    pub fn watermark(&self) -> u64 {
        self.next
    }
}
