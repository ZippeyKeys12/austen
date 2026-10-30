use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use crate::gen_dsl::{GenModule, GenTerm, GenTermKind, Sym};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TimingStats {
    pub count: u64,
    pub min: u64,
    pub max: u64,
    pub total: u64,
    pub last: u64,
}

impl TimingStats {
    pub fn seconds(micros: u64) -> f64 {
        micros as f64 / 1_000_000.0
    }

    pub fn last_secs(&self) -> f64 {
        Self::seconds(self.last)
    }

    pub fn observe(&mut self, elapsed: Duration) {
        let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        self.merge(Self {
            count: 1,
            min: micros,
            max: micros,
            total: micros,
            last: micros,
        });
    }

    pub fn merge(&mut self, interval: Self) {
        if interval.count == 0 {
            return;
        }
        if self.count == 0 {
            *self = interval;
            return;
        }
        self.count = self.count.saturating_add(interval.count);
        self.min = self.min.min(interval.min);
        self.max = self.max.max(interval.max);
        self.total = self.total.saturating_add(interval.total);
        self.last = interval.last;
    }
}

pub type Timings = BTreeMap<&'static str, TimingStats>;

pub fn merge_timings(into: &mut Timings, from: &Timings) {
    for (event, sample) in from {
        into.entry(event).or_default().merge(*sample);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SiteId {
    pub def: Arc<Sym>,
    pub ordinal: u64,
}

impl SiteId {
    pub fn new(def: Sym, ordinal: u64) -> Self {
        Self {
            def: Arc::new(def),
            ordinal,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SiteCount {
    pub failed: u64,
    pub passed: u64,
}

pub type SiteTally = BTreeMap<SiteId, SiteCount>;

#[derive(Debug, Default)]
pub struct SiteIndex(HashMap<usize, SiteId>);

impl SiteIndex {
    pub fn new(module: &GenModule) -> Self {
        let mut sites = HashMap::new();
        for (_, def) in &module.defs {
            let name = Arc::new(def.name.clone());
            let mut todo = vec![&def.body];
            let mut ordinal = 0;
            while let Some(term) = todo.pop() {
                sites.insert(
                    term as *const GenTerm as usize,
                    SiteId {
                        def: Arc::clone(&name),
                        ordinal,
                    },
                );
                ordinal += 1;
                match &term.term {
                    GenTermKind::Arbitrary | GenTermKind::Call { .. } | GenTermKind::Return(_) => {}
                    GenTermKind::Asgn { rest, .. }
                    | GenTermKind::Assert { rest, .. }
                    | GenTermKind::WriteChoice { rest, .. } => todo.push(rest),
                    GenTermKind::LetStar { value, rest, .. } => {
                        todo.push(rest);
                        todo.push(value);
                    }
                    GenTermKind::Ite { then_, else_, .. } => {
                        todo.push(else_);
                        todo.push(then_);
                    }
                    GenTermKind::Map { body, .. } | GenTermKind::Scope { body, .. } => {
                        todo.push(body)
                    }
                }
            }
        }
        Self(sites)
    }

    pub fn get(&self, term: &GenTerm) -> Option<&SiteId> {
        self.0.get(&(term as *const GenTerm as usize))
    }

    pub fn observe(&self, tally: &mut SiteTally, term: &GenTerm, passed: bool) {
        if let Some(site) = self.get(term) {
            Self::count(tally, site, passed);
        }
    }

    pub fn count(tally: &mut SiteTally, site: &SiteId, passed: bool) {
        let count = tally.entry(site.clone()).or_default();
        if passed {
            count.passed += 1;
        } else {
            count.failed += 1;
        }
    }
}
