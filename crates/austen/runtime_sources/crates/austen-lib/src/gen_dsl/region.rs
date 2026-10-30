use std::num::NonZeroU64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub base: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Congruence {
    modulus: NonZeroU64,
    residue: u64,
}

impl Congruence {
    pub const TOP: Self = Self {
        modulus: NonZeroU64::MIN,
        residue: 0,
    };

    pub fn of(modulus: u64, residue: u64) -> Option<Self> {
        let modulus = NonZeroU64::new(modulus)?;
        Some(Self {
            modulus,
            residue: residue % modulus,
        })
    }

    pub fn aligned(alignment: u64) -> Option<Self> {
        Self::of(alignment, 0)
    }

    fn sub(self, a: u64, b: u64) -> u64 {
        if a >= b {
            a - b
        } else {
            self.modulus.get() - (b - a)
        }
    }

    pub fn first_at_or_after(self, base: u64) -> Option<u64> {
        let modulus = self.modulus.get();
        if modulus == 1 {
            return Some(base);
        }
        base.checked_add(self.sub(self.residue, base % modulus))
    }

    pub fn last_at_or_before(self, addr: u64) -> Option<u64> {
        let modulus = self.modulus.get();
        if modulus == 1 {
            return Some(addr);
        }
        addr.checked_sub(self.sub(addr % modulus, self.residue))
    }

    pub fn meet(self, other: Self) -> Option<Self> {
        let (strong, weak) = if self.modulus >= other.modulus {
            (self, other)
        } else {
            (other, self)
        };
        let weak_modulus = weak.modulus.get();
        (strong.modulus.get() % weak_modulus == 0 && strong.residue % weak_modulus == weak.residue)
            .then_some(strong)
    }

    pub fn shifted_down(self, bytes: u64) -> Self {
        Self {
            residue: self.sub(self.residue, bytes % self.modulus.get()),
            ..self
        }
    }

    pub fn rebased(self, from: u64, to: u64) -> Self {
        let modulus = self.modulus.get();
        self.shifted_down(self.sub(from % modulus, to % modulus))
    }
}

impl Region {
    pub fn overlaps(&self, other: &Region) -> bool {
        self.base < other.base + other.size && other.base < self.base + self.size
    }

    pub fn covers(&self, inner: &Region) -> bool {
        let Some(inner_end) = inner.base.checked_add(inner.size) else {
            return false;
        };
        let Some(end) = self.base.checked_add(self.size) else {
            return false;
        };
        inner.base >= self.base && inner_end <= end
    }

    pub fn contains(&self, addr: u64) -> bool {
        addr >= self.base && addr - self.base < self.size
    }

    pub fn lowest_gap(&self, occupied: &mut [Region], want: u64) -> Option<Region> {
        self.lowest_congruent_gap(occupied, want, Congruence::TOP)
    }

    pub fn lowest_congruent_gap(
        &self,
        occupied: &mut [Region],
        want: u64,
        base_class: Congruence,
    ) -> Option<Region> {
        if want == 0 || want > self.size {
            return None;
        }
        occupied.sort_unstable_by_key(|region| region.base);
        let mut base = self.base;
        for region in occupied.iter().copied().filter(|region| region.size > 0) {
            let end = region.base.checked_add(region.size)?;
            let candidate = base_class.first_at_or_after(base)?;
            if candidate <= region.base && region.base - candidate >= want {
                return Some(Region {
                    base: candidate,
                    size: want,
                });
            }
            base = base.max(end);
        }
        let candidate = Region {
            base: base_class.first_at_or_after(base)?,
            size: want,
        };
        self.covers(&candidate).then_some(candidate)
    }

    pub fn debug_check_published(&self) {
        debug_assert!(self.base != 0, "base 0 would alias NULL");
        debug_assert!(
            self.base.checked_add(self.size).is_some(),
            "the heap region overflows the address space"
        );
    }
}
