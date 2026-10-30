pub const TALLY_ENV: &str = "AUSTEN_TALLY";

pub const TALLY_MAGIC: u64 = 0x4155_5354_5459_3031;

pub const SLOT_MAGIC: usize = 0;
pub const SLOT_SEEN: usize = 1;
pub const SLOT_DISPATCHED: usize = 2;
pub const SLOT_POST_FAILED: usize = 3;
pub const SLOT_SELECTED: usize = 4;

pub const TALLY_SLOTS: usize = 5;

pub const TALLY_BYTES: usize = TALLY_SLOTS * std::mem::size_of::<u64>();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    pub seen: u64,
    pub selected: u64,
    pub dispatched: u64,
    pub post_failed: u64,
}

impl Tally {
    pub fn stage(path: &std::path::Path) -> std::io::Result<()> {
        super::counters::CountersMap::<TALLY_SLOTS>::stage(path)
    }

    pub fn read(path: &std::path::Path) -> Option<Tally> {
        Self::settled(|| super::counters::file_slots(path))
    }

    fn settled(slots: impl FnMut() -> Option<[u64; TALLY_SLOTS]>) -> Option<Tally> {
        super::counters::settled(SLOT_MAGIC, TALLY_MAGIC, slots, |slots| {
            Self::from_slots(slots)
        })
    }

    pub fn from_slots(slots: &[u64]) -> Option<Tally> {
        if slots.len() < TALLY_SLOTS || slots[SLOT_MAGIC] != TALLY_MAGIC {
            return None;
        }
        let tally = Tally {
            seen: slots[SLOT_SEEN],
            selected: slots[SLOT_SELECTED],
            dispatched: slots[SLOT_DISPATCHED],
            post_failed: slots[SLOT_POST_FAILED],
        };
        (tally.post_failed <= tally.dispatched
            && tally.dispatched <= tally.selected
            && tally.selected <= tally.seen)
            .then_some(tally)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TallyAccess {
    Read,
    ReadWrite,
}

pub struct TallyMap(super::counters::CountersMap<TALLY_SLOTS>);

impl TallyMap {
    pub fn open(path: &std::path::Path, access: TallyAccess) -> Option<Self> {
        super::counters::CountersMap::open(path, access == TallyAccess::ReadWrite).map(Self)
    }

    pub fn slots(&self) -> &[core::sync::atomic::AtomicU64; TALLY_SLOTS] {
        self.0.slots()
    }

    pub fn read(&self) -> Option<Tally> {
        Tally::settled(|| Some(self.0.snapshot()))
    }

    pub fn into_raw(self) -> *mut core::sync::atomic::AtomicU64 {
        self.0.into_raw()
    }
}

pub enum TallyReader {
    Mapped(TallyMap),
    File(std::path::PathBuf),
}

impl TallyReader {
    pub fn open(path: std::path::PathBuf) -> Self {
        TallyMap::open(&path, TallyAccess::Read).map_or_else(|| Self::File(path), Self::Mapped)
    }

    pub fn read(&self) -> Option<Tally> {
        match self {
            Self::Mapped(page) => page.read(),
            Self::File(path) => Tally::read(path),
        }
    }
}

impl std::fmt::Debug for TallyReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mapped(_) => f.write_str("Mapped"),
            Self::File(path) => f.debug_tuple("File").field(path).finish(),
        }
    }
}
