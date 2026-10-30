use crate::gen_dsl::{Congruence, Region};

#[derive(Debug, Default)]
pub struct MarkedHeap {
    base: u64,
    next: u64,
    regions: Vec<Region>,
    max_end: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocError {
    Overlap,
}

impl MarkedHeap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_base(base: u64) -> Self {
        Self {
            base,
            next: base,
            regions: Vec::new(),
            max_end: 0,
        }
    }

    pub fn alloc(&mut self, size: u64) -> Region {
        let base = self.next;
        self.next = base.saturating_add(size);
        let region = Region { base, size };
        self.record(region);
        region
    }

    pub fn try_alloc(&mut self, size: u64, capacity: u64) -> Option<Region> {
        self.try_alloc_congruent(size, Congruence::TOP, capacity)
    }

    pub fn try_alloc_congruent(
        &mut self,
        size: u64,
        base_class: Congruence,
        capacity: u64,
    ) -> Option<Region> {
        let used = self.next.checked_sub(self.base)?;
        let base = base_class.first_at_or_after(self.next)?;
        let padding = base.checked_sub(self.next)?;
        let bumped = used.checked_add(padding)?.checked_add(size)?;
        if bumped > capacity {
            return None;
        }
        let next = base.checked_add(size)?;
        let region = Region { base, size };
        self.next = next;
        self.record(region);
        Some(region)
    }

    pub fn mark(&mut self, region: Region) -> Result<(), AllocError> {
        if self.overlaps(&region) {
            return Err(AllocError::Overlap);
        }
        self.record(region);
        Ok(())
    }

    pub(in crate::gen_dsl) fn overlaps(&self, region: &Region) -> bool {
        if region.size > 0
            && region.base >= self.max_end
            && region.base.checked_add(region.size).is_some()
        {
            return false;
        }
        self.regions.iter().any(|r| r.overlaps(region))
    }

    pub(in crate::gen_dsl) fn record(&mut self, region: Region) {
        self.max_end = self.max_end.max(region.base.saturating_add(region.size));
        self.regions.push(region);
    }

    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    pub fn covers(&self, write: &Region) -> bool {
        write.size == 0 || self.regions.iter().any(|r| r.covers(write))
    }

    #[must_use]
    pub fn save(&self) -> HeapMark {
        HeapMark {
            next: self.next,
            regions: self.regions.len(),
            max_end: self.max_end,
        }
    }

    pub fn restore(&mut self, mark: HeapMark) {
        debug_assert!(
            mark.regions <= self.regions.len() && mark.next <= self.next,
            "a heap mark is from a later state than the heap"
        );
        self.regions.truncate(mark.regions);
        self.next = mark.next;
        self.max_end = mark.max_end;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeapMark {
    next: u64,
    regions: usize,
    max_end: u64,
}
