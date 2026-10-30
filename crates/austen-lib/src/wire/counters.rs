use core::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::path::Path;

const ATTEMPTS: usize = 3;

pub(super) fn settled<const SLOTS: usize, T>(
    magic_slot: usize,
    magic: u64,
    mut slots: impl FnMut() -> Option<[u64; SLOTS]>,
    mut decode: impl FnMut(&[u64; SLOTS]) -> Option<T>,
) -> Option<T> {
    for _ in 0..ATTEMPTS {
        let snapshot = slots()?;
        if snapshot[magic_slot] != magic {
            let active = snapshot
                .iter()
                .enumerate()
                .any(|(slot, value)| slot != magic_slot && *value != 0);
            if active {
                continue;
            }
            return None;
        }
        if let Some(value) = decode(&snapshot) {
            return Some(value);
        }
    }
    None
}

pub(super) fn stable<const SLOTS: usize, T>(
    magic_slot: usize,
    magic: u64,
    slots: impl FnMut() -> Option<[u64; SLOTS]>,
    decode: impl Fn(&[u64; SLOTS]) -> Option<T>,
) -> Option<T> {
    let mut previous: Option<[u64; SLOTS]> = None;
    settled(magic_slot, magic, slots, move |snapshot| {
        if previous.as_ref() == Some(snapshot) {
            return decode(snapshot);
        }
        previous = Some(*snapshot);
        None
    })
}

pub(super) fn file_slots<const SLOTS: usize>(path: &Path) -> Option<[u64; SLOTS]> {
    let bytes = std::fs::read(path).ok()?;
    let head = bytes.get(..SLOTS * size_of::<u64>())?;
    Some(core::array::from_fn(|slot| {
        u64::from_ne_bytes(
            head[slot * 8..][..8]
                .try_into()
                .expect("eight bytes per slot"),
        )
    }))
}

pub(super) struct CountersMap<const SLOTS: usize> {
    slots: *mut AtomicU64,
}

impl<const SLOTS: usize> CountersMap<SLOTS> {
    pub(super) const BYTES: usize = SLOTS * size_of::<u64>();

    pub(super) fn stage(path: &Path) -> std::io::Result<()> {
        std::fs::write(path, vec![0u8; Self::BYTES])
    }

    pub(super) fn open(path: &Path, write: bool) -> Option<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(write)
            .open(path)
            .ok()?;
        if file.metadata().ok()?.len() < Self::BYTES as u64 {
            return None;
        }
        let prot = if write {
            libc::PROT_READ | libc::PROT_WRITE
        } else {
            libc::PROT_READ
        };
        use std::os::fd::AsRawFd;
        let page = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                Self::BYTES,
                prot,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        (page != libc::MAP_FAILED).then(|| Self { slots: page.cast() })
    }

    pub(super) fn slots(&self) -> &[AtomicU64; SLOTS] {
        unsafe { &*self.slots.cast() }
    }

    pub(super) fn snapshot(&self) -> [u64; SLOTS] {
        let slots = self.slots();
        core::array::from_fn(|slot| slots[slot].load(Relaxed))
    }

    pub(super) fn into_raw(self) -> *mut AtomicU64 {
        core::mem::ManuallyDrop::new(self).slots
    }
}

impl<const SLOTS: usize> Drop for CountersMap<SLOTS> {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.slots.cast(), Self::BYTES) };
    }
}
