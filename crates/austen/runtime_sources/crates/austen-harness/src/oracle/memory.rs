use std::fmt;

use austen_lib::spec::{DefaultChoices, PointerOrigin, Sym, SymMap, Value};

unsafe extern "C" {
    fn austen_oracle_copy_object_representation_v1(
        destination: *mut u8,
        source: *const libc::c_void,
        length: libc::size_t,
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateWrite {
    pub addr: u64,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterializedAllocation {
    pub base: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnershipExtent {
    pub base: u64,
    pub size: u64,
}

impl MaterializedAllocation {
    pub const fn tracked(self) -> TrackedObject {
        TrackedObject {
            base: self.base,
            size: self.size,
            state: ObjectState::Live(Access::Raw(PointerOrigin::Heap)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Raw(PointerOrigin),
    Held,
    Probe,
}

impl Access {
    pub const fn bounds(self) -> Bounds {
        match self {
            Self::Raw(PointerOrigin::Global(id)) => Bounds::Origin(id),
            Self::Raw(_) => Bounds::AccessOnly,
            Self::Held | Self::Probe => Bounds::Extent,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bounds {
    AccessOnly,
    Extent,
    Origin(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectState {
    Live(Access),
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackedObject {
    pub base: u64,
    pub size: u64,
    pub state: ObjectState,
}

impl TrackedObject {
    pub const fn allocation(base: u64, size: u64) -> Self {
        Self {
            base,
            size,
            state: ObjectState::Live(Access::Probe),
        }
    }

    pub const fn owned_allocation(base: u64, size: u64) -> Self {
        Self {
            base,
            size,
            state: ObjectState::Live(Access::Held),
        }
    }

    pub const fn released(self) -> Self {
        Self {
            state: ObjectState::Released,
            ..self
        }
    }

    pub fn contains(&self, addr: u64) -> bool {
        addr >= self.base && addr - self.base < self.size
    }

    pub fn covers(&self, addr: u64, size: u64) -> bool {
        addr.checked_sub(self.base)
            .is_some_and(|offset| offset <= self.size && size <= self.size - offset)
    }

    pub fn overlaps(&self, addr: u64, size: u64) -> bool {
        size != 0 && (self.contains(addr) || (self.base >= addr && self.base - addr < size))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegionKind {
    Heap,
    Global(Sym),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRegion {
    pub base: u64,
    pub size: u64,
    pub writable: bool,
    pub kind: RegionKind,
}

impl MemoryRegion {
    pub const fn heap(base: u64, size: u64) -> Self {
        Self {
            base,
            size,
            writable: true,
            kind: RegionKind::Heap,
        }
    }

    pub const fn end(&self) -> Option<u64> {
        self.base.checked_add(self.size)
    }

    pub fn covers(&self, addr: u64, size: u64) -> bool {
        let Some(end) = addr.checked_add(size) else {
            return false;
        };
        let Some(outer_end) = self.end() else {
            return false;
        };
        addr >= self.base && end <= outer_end
    }

    pub fn tracked(&self) -> TrackedObject {
        TrackedObject {
            base: self.base,
            size: self.size,
            state: ObjectState::Live(Access::Raw(self.origin())),
        }
    }

    pub fn origin(&self) -> PointerOrigin {
        match &self.kind {
            RegionKind::Heap => PointerOrigin::Heap,
            RegionKind::Global(symbol) => PointerOrigin::Global(symbol.num),
        }
    }
}

pub(super) fn pointer_origin(addr: u64, regions: &[MemoryRegion]) -> PointerOrigin {
    if addr != 0
        && let Some(region) = regions.iter().find(|region| region.covers(addr, 1))
    {
        region.origin()
    } else {
        PointerOrigin::Opaque
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryAccessError {
    pub reason: String,
}

impl MemoryAccessError {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl fmt::Display for MemoryAccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for MemoryAccessError {}

pub trait MemoryView {
    fn read_into(&self, addr: u64, size: u64, buf: &mut Vec<u8>) -> Result<(), MemoryAccessError>;

    fn write_back(&self, _addr: u64, _bytes: &[u8]) -> Result<(), MemoryAccessError> {
        Ok(())
    }

    fn read_foreign(
        &self,
        addr: u64,
        size: u64,
        _buf: &mut Vec<u8>,
    ) -> Result<(), MemoryAccessError> {
        Err(MemoryAccessError::new(format!(
            "no fault-free probe for the foreign extent at {addr:#x}+{size}"
        )))
    }

    fn write_back_foreign(&self, addr: u64, bytes: &[u8]) -> Result<(), MemoryAccessError> {
        Err(MemoryAccessError::new(format!(
            "no fault-free probe for the foreign extent at {addr:#x}+{}",
            bytes.len()
        )))
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LiveMemory;

impl MemoryView for LiveMemory {
    fn read_into(&self, addr: u64, size: u64, buf: &mut Vec<u8>) -> Result<(), MemoryAccessError> {
        let len = usize::try_from(size)
            .map_err(|_| MemoryAccessError::new("memory extent does not fit usize"))?;
        buf.clear();
        if len == 0 {
            return Ok(());
        }
        buf.resize(len, 0);
        unsafe {
            austen_oracle_copy_object_representation_v1(
                buf.as_mut_ptr(),
                addr as *const libc::c_void,
                len,
            );
        }
        Ok(())
    }

    fn write_back(&self, addr: u64, bytes: &[u8]) -> Result<(), MemoryAccessError> {
        unsafe { std::ptr::copy(bytes.as_ptr(), addr as *mut u8, bytes.len()) };
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn read_foreign(
        &self,
        addr: u64,
        size: u64,
        buf: &mut Vec<u8>,
    ) -> Result<(), MemoryAccessError> {
        let len = usize::try_from(size)
            .map_err(|_| MemoryAccessError::new("memory extent does not fit usize"))?;
        buf.clear();
        if len == 0 {
            return Ok(());
        }
        buf.resize(len, 0);
        let local = libc::iovec {
            iov_base: buf.as_mut_ptr().cast(),
            iov_len: len,
        };
        let remote = libc::iovec {
            iov_base: addr as *mut libc::c_void,
            iov_len: len,
        };
        let copied = unsafe { libc::process_vm_readv(libc::getpid(), &local, 1, &remote, 1, 0) };
        foreign_result("read", copied, len, addr)
    }

    #[cfg(target_os = "linux")]
    fn write_back_foreign(&self, addr: u64, bytes: &[u8]) -> Result<(), MemoryAccessError> {
        if bytes.is_empty() {
            return Ok(());
        }
        let local = libc::iovec {
            iov_base: bytes.as_ptr().cast_mut().cast(),
            iov_len: bytes.len(),
        };
        let remote = libc::iovec {
            iov_base: addr as *mut libc::c_void,
            iov_len: bytes.len(),
        };
        let copied = unsafe { libc::process_vm_writev(libc::getpid(), &local, 1, &remote, 1, 0) };
        foreign_result("write back", copied, bytes.len(), addr)
    }
}

#[cfg(target_os = "linux")]
fn foreign_result(
    action: &str,
    copied: isize,
    len: usize,
    addr: u64,
) -> Result<(), MemoryAccessError> {
    if copied < 0 {
        return Err(MemoryAccessError::new(format!(
            "cannot {action} the foreign extent at {addr:#x}+{len}: {}",
            std::io::Error::last_os_error()
        )));
    }
    if copied as usize != len {
        return Err(MemoryAccessError::new(format!(
            "foreign extent at {addr:#x}+{len} is only accessible for {copied} bytes"
        )));
    }
    Ok(())
}

pub struct CandidateState<'a> {
    pub defaults: DefaultChoices,
    pub bindings: SymMap<Value>,
    pub writes: Vec<CandidateWrite>,
    pub regions: &'a [MemoryRegion],
    pub memory: &'a dyn MemoryView,
    pub allocation_roots: &'a [u64],
}

impl<'a> CandidateState<'a> {
    pub fn new(
        bindings: SymMap<Value>,
        writes: Vec<CandidateWrite>,
        regions: &'a [MemoryRegion],
        memory: &'a dyn MemoryView,
    ) -> Self {
        Self {
            defaults: DefaultChoices::default(),
            bindings,
            writes,
            regions,
            memory,
            allocation_roots: &[],
        }
    }

    pub fn with_allocation_roots(mut self, allocation_roots: &'a [u64]) -> Self {
        self.allocation_roots = allocation_roots;
        self
    }

    pub fn with_defaults(mut self, defaults: DefaultChoices) -> Self {
        self.defaults = defaults;
        self
    }
}

pub struct PostState<'a> {
    pub defaults: DefaultChoices,
    pub bindings: SymMap<Value>,
    pub return_carrier: i128,
    pub regions: &'a [MemoryRegion],
    pub memory: &'a dyn MemoryView,
    pub objects: &'a [TrackedObject],
    pub heap_claims: &'a [OwnershipExtent],
    pub leak_check: bool,
}

impl<'a> PostState<'a> {
    pub fn new(
        bindings: SymMap<Value>,
        return_carrier: i128,
        regions: &'a [MemoryRegion],
        memory: &'a dyn MemoryView,
    ) -> Self {
        Self {
            defaults: DefaultChoices::default(),
            bindings,
            return_carrier,
            regions,
            memory,
            objects: &[],
            heap_claims: &[],
            leak_check: false,
        }
    }

    pub fn with_defaults(mut self, defaults: DefaultChoices) -> Self {
        self.defaults = defaults;
        self
    }

    pub fn with_objects(mut self, objects: &'a [TrackedObject]) -> Self {
        self.objects = objects;
        self
    }

    pub fn with_heap_claims(mut self, heap_claims: &'a [OwnershipExtent]) -> Self {
        self.heap_claims = heap_claims;
        self
    }

    pub fn with_leak_check(mut self, leak_check: bool) -> Self {
        self.leak_check = leak_check;
        self
    }
}
