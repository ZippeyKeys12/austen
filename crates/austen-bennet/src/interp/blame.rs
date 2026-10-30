use austen_lib::gen_dsl::{Congruence, Incoming, Sym, SymSet};

use austen_lib::gen_dsl::RejectReason;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureKind {
    Assert,
    Assign,
    Depth,
    Timeout,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct OwnDomain {
    pub before: u64,
    pub after: u64,
}

impl OwnDomain {
    pub fn covering(base: u64, write: u64, size: u64, capacity: u64) -> Self {
        if size == 0 {
            return Self::default();
        }
        if size > capacity {
            return Self {
                before: 0,
                after: size,
            };
        }
        let end = write.wrapping_add(size);
        let before = base.wrapping_sub(write);
        let after = end.wrapping_sub(base);
        OwnDomain {
            before: if before <= write.wrapping_sub(base) {
                before
            } else {
                0
            },
            after: if after <= base.wrapping_sub(end) {
                after
            } else {
                0
            },
        }
    }

    pub fn meet(self, other: Self) -> Self {
        OwnDomain {
            before: self.before.max(other.before),
            after: self.after.max(other.after),
        }
    }

    pub fn rebase(self, from: u64, to: u64, capacity: u64) -> Self {
        if self.is_bottom(capacity) {
            return self;
        }
        let bytes = self.bytes().expect("a non-bottom domain has a byte count");
        OwnDomain::covering(to, from.wrapping_sub(self.before), bytes, capacity)
    }

    pub fn bytes(self) -> Option<u64> {
        self.before.checked_add(self.after)
    }

    pub fn is_bottom(self, capacity: u64) -> bool {
        self.bytes().is_none_or(|bytes| bytes > capacity)
    }

    pub fn is_top(self) -> bool {
        self == OwnDomain::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PointerReq {
    pub ownership: OwnDomain,
    congruence: Option<Congruence>,
}

impl Default for PointerReq {
    fn default() -> Self {
        Self {
            ownership: OwnDomain::default(),
            congruence: Some(Congruence::TOP),
        }
    }
}

impl PointerReq {
    pub fn owning(ownership: OwnDomain) -> Self {
        Self {
            ownership,
            ..Self::default()
        }
    }

    pub fn aligned(alignment: u64) -> Self {
        Self {
            ownership: OwnDomain::default(),
            congruence: Congruence::aligned(alignment),
        }
    }

    pub fn meet(self, other: Self) -> Self {
        Self {
            ownership: self.ownership.meet(other.ownership),
            congruence: match (self.congruence, other.congruence) {
                (Some(a), Some(b)) => a.meet(b),
                _ => None,
            },
        }
    }

    pub fn rebase(self, from: u64, to: u64, capacity: u64) -> Self {
        Self {
            ownership: self.ownership.rebase(from, to, capacity),
            congruence: self.congruence.map(|class| class.rebased(from, to)),
        }
    }

    pub fn allocation_base_congruence(self) -> Option<Congruence> {
        Some(self.congruence?.shifted_down(self.ownership.before))
    }

    pub fn has_ownership(self) -> bool {
        !self.ownership.is_top()
    }

    pub fn has_congruence_solution(self) -> bool {
        self.congruence.is_some()
    }
}

pub(crate) fn ownership_capacity(incoming: &Incoming) -> u64 {
    incoming
        .write_regions()
        .map(|(_, region)| region.size)
        .max()
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    pub kind: FailureKind,
    pub young: bool,
    pub reason: RejectReason,
    blamed: Vec<(Sym, Option<PointerReq>)>,
}

impl Failure {
    pub fn unblamed(kind: FailureKind, reason: RejectReason) -> Self {
        Failure {
            kind,
            young: true,
            reason,
            blamed: Vec::new(),
        }
    }

    pub fn blaming(
        kind: FailureKind,
        reason: RejectReason,
        syms: impl IntoIterator<Item = Sym>,
    ) -> Self {
        let mut f = Failure::unblamed(kind, reason);
        for s in syms {
            f.blame(s, None);
        }
        f
    }

    pub fn blame(&mut self, sym: Sym, requirement: Option<PointerReq>) {
        if let Some(slot) = self.blamed.iter_mut().find(|(s, _)| *s == sym) {
            slot.1 = match (slot.1, requirement) {
                (Some(a), Some(b)) => Some(a.meet(b)),
                (a, b) => a.or(b),
            };
            return;
        }
        self.blamed.push((sym, requirement));
    }

    pub fn is_unblamed(&self) -> bool {
        self.blamed.is_empty()
    }

    pub fn blames(&self, sym: &Sym) -> bool {
        self.blamed.iter().any(|(s, _)| s == sym)
    }

    pub fn take_blame(&mut self, sym: &Sym) {
        let Some(at) = self.blamed.iter().position(|(s, _)| s == sym) else {
            return;
        };
        self.young = false;
        self.blamed.remove(at);
    }

    pub fn requirement_of(&self, sym: &Sym) -> Option<PointerReq> {
        self.blamed
            .iter()
            .find(|(s, _)| s == sym)
            .and_then(|(_, d)| *d)
    }

    pub fn relabel(&mut self, from: &Sym, to: SymSet) {
        if !self.blames(from) {
            return;
        }
        let young = self.young;
        self.take_blame(from);
        for s in to {
            self.blame(s, None);
        }
        self.young = young;
    }

    pub fn retarget(&mut self, from: &Sym, to: SymSet) {
        if !self.blames(from) {
            return;
        }
        self.relabel(from, to);
        self.young = false;
    }

    pub fn remap_call(
        &mut self,
        remap: impl Fn(&Sym, Option<PointerReq>) -> Option<Vec<(Sym, Option<PointerReq>)>>,
    ) {
        let young = self.young;
        let old = std::mem::take(&mut self.blamed);
        for (sym, domain) in old {
            match remap(&sym, domain) {
                Some(actual) => {
                    for (s, mapped_domain) in actual {
                        self.blame(s, mapped_domain);
                    }
                }
                None => self.blame(sym, domain),
            }
        }
        self.young = young;
    }
}
