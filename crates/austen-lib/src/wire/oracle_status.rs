use core::sync::atomic::Ordering;

pub const ORACLE_STATUS_ENV: &str = "AUSTEN_ORACLE_STATUS";

pub const ORACLE_STATUS_MAGIC: u64 = 0x4155_5354_4f52_3031;

const ORACLE_CLAIM_ORDER: Ordering = Ordering::Release;
const ORACLE_INCREMENT_ORDER: Ordering = Ordering::Release;
const ORACLE_SNAPSHOT_FENCE: Ordering = Ordering::Acquire;

const SLOT_MAGIC: usize = 0;
const SLOT_PARSE_REJECTED: usize = 1;
const SLOT_PRE_PASSED: usize = 2;
const SLOT_PRE_FAILED: usize = 3;
const SLOT_PRE_UNSUPPORTED: usize = 4;
const SLOT_PRE_INTERNAL_ERROR: usize = 5;
const SLOT_POST_UNSUPPORTED: usize = 6;

pub const ORACLE_STATUS_SLOTS: usize = 7;
pub const ORACLE_STATUS_BYTES: usize = ORACLE_STATUS_SLOTS * core::mem::size_of::<u64>();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreStatus {
    ParseRejected,
    Passed,
    Failed,
    Unsupported,
    InternalError,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OracleStatus {
    pub parse_rejected: u64,
    pub pre_passed: u64,
    pub pre_failed: u64,
    pub pre_unsupported: u64,
    pub pre_internal_error: u64,
    pub post_unsupported: u64,
}

impl OracleStatus {
    pub fn stage(path: &std::path::Path) -> std::io::Result<()> {
        super::counters::CountersMap::<ORACLE_STATUS_SLOTS>::stage(path)
    }

    pub fn read(path: &std::path::Path) -> Option<Self> {
        OracleStatusMap::open(path, OracleStatusAccess::Read)?.read()
    }

    fn settled(slots: impl FnMut() -> Option<[u64; ORACLE_STATUS_SLOTS]>) -> Option<Self> {
        super::counters::stable(SLOT_MAGIC, ORACLE_STATUS_MAGIC, slots, |slots| {
            Self::from_slots(slots)
        })
    }

    pub fn from_slots(slots: &[u64]) -> Option<Self> {
        (slots.len() >= ORACLE_STATUS_SLOTS && slots[SLOT_MAGIC] == ORACLE_STATUS_MAGIC).then(
            || Self {
                parse_rejected: slots[SLOT_PARSE_REJECTED],
                pre_passed: slots[SLOT_PRE_PASSED],
                pre_failed: slots[SLOT_PRE_FAILED],
                pre_unsupported: slots[SLOT_PRE_UNSUPPORTED],
                pre_internal_error: slots[SLOT_PRE_INTERNAL_ERROR],
                post_unsupported: slots[SLOT_POST_UNSUPPORTED],
            },
        )
    }

    pub fn observed(self) -> u64 {
        self.parse_rejected
            + self.pre_passed
            + self.pre_failed
            + self.pre_unsupported
            + self.pre_internal_error
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OracleStatusAccess {
    Read,
    ReadWrite,
}

pub struct OracleStatusMap(super::counters::CountersMap<ORACLE_STATUS_SLOTS>);

impl OracleStatusMap {
    pub fn open(path: &std::path::Path, access: OracleStatusAccess) -> Option<Self> {
        super::counters::CountersMap::open(path, access == OracleStatusAccess::ReadWrite).map(Self)
    }

    pub fn slots(&self) -> &[core::sync::atomic::AtomicU64; ORACLE_STATUS_SLOTS] {
        self.0.slots()
    }

    pub fn record_pre(&self, status: PreStatus) {
        let slots = self.slots();
        slots[SLOT_MAGIC].store(ORACLE_STATUS_MAGIC, ORACLE_CLAIM_ORDER);
        let slot = match status {
            PreStatus::ParseRejected => SLOT_PARSE_REJECTED,
            PreStatus::Passed => SLOT_PRE_PASSED,
            PreStatus::Failed => SLOT_PRE_FAILED,
            PreStatus::Unsupported => SLOT_PRE_UNSUPPORTED,
            PreStatus::InternalError => SLOT_PRE_INTERNAL_ERROR,
        };
        slots[slot].fetch_add(1, ORACLE_INCREMENT_ORDER);
    }

    pub fn record_post_unsupported(&self) {
        let slots = self.slots();
        slots[SLOT_MAGIC].store(ORACLE_STATUS_MAGIC, ORACLE_CLAIM_ORDER);
        slots[SLOT_POST_UNSUPPORTED].fetch_add(1, ORACLE_INCREMENT_ORDER);
    }

    fn published_snapshot(&self) -> [u64; ORACLE_STATUS_SLOTS] {
        let snapshot = self.0.snapshot();
        core::sync::atomic::fence(ORACLE_SNAPSHOT_FENCE);
        snapshot
    }

    pub fn read(&self) -> Option<OracleStatus> {
        OracleStatus::settled(|| Some(self.published_snapshot()))
    }

    pub fn into_raw(self) -> *mut core::sync::atomic::AtomicU64 {
        self.0.into_raw()
    }
}

pub enum OracleStatusReader {
    Mapped(OracleStatusMap),
    File(std::path::PathBuf),
}

impl OracleStatusReader {
    pub fn open(path: std::path::PathBuf) -> Self {
        OracleStatusMap::open(&path, OracleStatusAccess::Read)
            .map_or_else(|| Self::File(path), Self::Mapped)
    }

    pub fn read(&self) -> Option<OracleStatus> {
        match self {
            Self::Mapped(page) => page.read(),
            Self::File(path) => OracleStatus::read(path),
        }
    }
}

impl std::fmt::Debug for OracleStatusReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mapped(_) => f.write_str("Mapped"),
            Self::File(path) => f.debug_tuple("File").field(path).finish(),
        }
    }
}
