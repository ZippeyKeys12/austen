use crate::gen_dsl::Region;
use crate::gen_dsl::lang::{
    Env, GenModule, GlobalDef, PointerOrigin, Scalar, Sctype, StructDef, Sym, SymMap, Value,
};

pub const GLOBAL_LAYOUT_PREFIX: &str = "austen-global-layout-v1:";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GlobalLayoutError {
    #[error("global {sym:?} has unsized C type {ty:?}")]
    UnsizedGlobal { sym: Sym, ty: Sctype },
}

pub fn global_layout_tables(globals: &[(Sym, GlobalDef)], structs: &[(Sym, StructDef)]) -> String {
    let structs = if globals.is_empty() { &[][..] } else { structs };
    let encoded = serde_json::to_string(&(globals, structs))
        .expect("global and struct declarations have infallible JSON serialization");
    format!("{GLOBAL_LAYOUT_PREFIX}{encoded}")
}

pub fn global_layout(ctx: &GenModule) -> Result<String, GlobalLayoutError> {
    for (sym, global) in &ctx.globals {
        match global.ty.byte_size_in(&ctx.structs) {
            Some(size) if size > 0 => {}
            _ => {
                return Err(GlobalLayoutError::UnsizedGlobal {
                    sym: sym.clone(),
                    ty: global.ty.clone(),
                });
            }
        }
    }
    Ok(global_layout_tables(&ctx.globals, &ctx.structs))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalObject {
    pub region: Region,
    pub writable: bool,
    startup: Option<Vec<u8>>,
}

impl GlobalObject {
    pub const fn new(address: u64, size: u64) -> Self {
        Self {
            region: Region {
                base: address,
                size,
            },
            writable: true,
            startup: None,
        }
    }

    pub const fn with_writability(mut self, writable: bool) -> Self {
        self.writable = writable;
        self
    }

    pub const fn address(&self) -> u64 {
        self.region.base
    }

    pub fn startup(&self) -> Option<&[u8]> {
        self.startup.as_deref()
    }
}

impl From<Region> for GlobalObject {
    fn from(region: Region) -> Self {
        Self {
            region,
            writable: true,
            startup: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    Heap,
    Global(u64),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IncomingError {
    #[error("the incoming heap wraps the address space")]
    HeapOverflow,
    #[error("global {sym:?} has a NULL address")]
    NullGlobal { sym: Sym },
    #[error("global {sym:?} has zero size")]
    EmptyGlobal { sym: Sym },
    #[error("global {sym:?} wraps the address space")]
    GlobalOverflow { sym: Sym },
    #[error("global symbol {sym:?} occurs more than once")]
    DuplicateGlobal { sym: Sym },
    #[error("cannot attach startup bytes for undeclared global {sym:?}")]
    UnknownGlobalSnapshot { sym: Sym },
    #[error("startup snapshot for global {sym:?} has {actual} bytes, but its extent is {expected}")]
    GlobalSnapshotSize {
        sym: Sym,
        expected: u64,
        actual: usize,
    },
    #[error("global {sym:?} overlaps the incoming heap")]
    GlobalOverlapsHeap { sym: Sym },
    #[error("globals {left:?} and {right:?} overlap")]
    GlobalsOverlap { left: Sym, right: Sym },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incoming {
    heap: Region,
    globals: SymMap<GlobalObject>,
    global_order: Vec<Sym>,
}

impl Incoming {
    pub fn new(
        heap: Region,
        globals: impl IntoIterator<Item = (Sym, GlobalObject)>,
    ) -> Result<Self, IncomingError> {
        if heap.base.checked_add(heap.size).is_none() {
            return Err(IncomingError::HeapOverflow);
        }
        let mut checked: SymMap<GlobalObject> = SymMap::new();
        let mut global_order = Vec::new();
        for (sym, object) in globals {
            if object.region.base == 0 {
                return Err(IncomingError::NullGlobal { sym });
            }
            if object.region.size == 0 {
                return Err(IncomingError::EmptyGlobal { sym });
            }
            if object.region.base.checked_add(object.region.size).is_none() {
                return Err(IncomingError::GlobalOverflow { sym });
            }
            if checked.contains_key(&sym) {
                return Err(IncomingError::DuplicateGlobal { sym });
            }
            if heap.size != 0 && heap.overlaps(&object.region) {
                return Err(IncomingError::GlobalOverlapsHeap { sym });
            }
            if let Some((other, _)) = checked
                .iter()
                .find(|(_, prior)| prior.region.overlaps(&object.region))
            {
                return Err(IncomingError::GlobalsOverlap {
                    left: other.clone(),
                    right: sym,
                });
            }
            global_order.push(sym.clone());
            checked.insert(sym, object);
        }
        Ok(Self {
            heap,
            globals: checked,
            global_order,
        })
    }

    pub fn heap_only(heap: Region) -> Result<Self, IncomingError> {
        Self::new(heap, [])
    }

    pub const fn heap(&self) -> Region {
        self.heap
    }

    pub fn globals(&self) -> &SymMap<GlobalObject> {
        &self.globals
    }

    pub fn globals_in_order(&self) -> impl Iterator<Item = (&Sym, &GlobalObject)> {
        self.global_order.iter().map(|sym| {
            (
                sym,
                self.globals
                    .get(sym)
                    .expect("the order vector and global map are built together"),
            )
        })
    }

    pub fn global(&self, sym: &Sym) -> Option<&GlobalObject> {
        self.globals.get(sym)
    }

    pub fn write_regions(&self) -> impl Iterator<Item = (Grant, Region)> + '_ {
        std::iter::once((Grant::Heap, self.heap)).chain(self.globals.iter().filter_map(
            |(sym, object)| {
                object
                    .writable
                    .then_some((Grant::Global(sym.num), object.region))
            },
        ))
    }

    pub fn set_global_startup(&mut self, sym: &Sym, bytes: Vec<u8>) -> Result<(), IncomingError> {
        let Some(object) = self.globals.get_mut(sym) else {
            return Err(IncomingError::UnknownGlobalSnapshot { sym: sym.clone() });
        };
        if object.region.size != bytes.len() as u64 {
            return Err(IncomingError::GlobalSnapshotSize {
                sym: sym.clone(),
                expected: object.region.size,
                actual: bytes.len(),
            });
        }
        object.startup = Some(bytes);
        Ok(())
    }

    fn global_bindings(&self) -> impl Iterator<Item = (&Sym, Value)> {
        self.globals.iter().map(|(sym, object)| {
            (
                sym,
                Value::Scalar(Scalar::global_pointer(object.address(), sym.num)),
            )
        })
    }

    pub fn bind_globals(&self, env: &mut Env) {
        for (sym, value) in self.global_bindings() {
            env.insert(sym, value);
        }
    }

    pub fn bind_globals_map(&self, env: &mut SymMap<Value>) {
        for (sym, value) in self.global_bindings() {
            env.insert(sym.clone(), value);
        }
    }

    pub fn grant_write(&self, extent: &Region) -> Option<Grant> {
        if extent.base == 0 {
            return None;
        }
        self.write_regions()
            .find_map(|(grant, region)| region.covers(extent).then_some(grant))
    }

    pub fn grant_read(&self, origin: PointerOrigin, extent: &Region) -> Option<Grant> {
        match origin {
            PointerOrigin::Heap => self.heap.covers(extent).then_some(Grant::Heap),
            PointerOrigin::Global(sym) => self
                .global_by_num(sym)
                .filter(|object| object.region.covers(extent))
                .map(|_| Grant::Global(sym)),
            PointerOrigin::Opaque => None,
        }
    }

    pub fn global_by_num(&self, num: u64) -> Option<&GlobalObject> {
        self.globals.get(&Sym { name: None, num })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedGlobal {
    pub sym: Sym,
    pub address: u64,
    pub size: u64,
}

pub fn incoming_from_published(
    ctx: &GenModule,
    peer: &str,
    layout: &str,
    heap: Region,
    published: impl IntoIterator<Item = PublishedGlobal>,
) -> Result<Incoming, String> {
    let expected_layout = global_layout(ctx).map_err(|error| error.to_string())?;
    if layout != expected_layout {
        return Err(format!(
            "global layout mismatch: {peer} {layout:?}, staged {expected_layout:?}"
        ));
    }
    let published: Vec<PublishedGlobal> = published.into_iter().collect();
    if published.len() != ctx.globals.len() {
        return Err(format!(
            "global table length mismatch: {peer} {}, staged {}",
            published.len(),
            ctx.globals.len()
        ));
    }
    let globals = ctx
        .globals
        .iter()
        .zip(published)
        .map(|((expected_sym, global), entry)| {
            let named_differently = entry
                .sym
                .name
                .as_deref()
                .is_some_and(|name| Some(name) != expected_sym.name.as_deref());
            if entry.sym.num != expected_sym.num || named_differently {
                return Err(format!(
                    "global table order mismatch: {peer} {:?}, staged {expected_sym:?}",
                    entry.sym
                ));
            }
            let expected_size = global.ty.byte_size_in(&ctx.structs).ok_or_else(|| {
                format!(
                    "global {expected_sym:?} has no complete storage size in the staged context"
                )
            })?;
            if entry.size != expected_size {
                return Err(format!(
                    "global {expected_sym:?} size mismatch: {peer} {}, staged {expected_size}",
                    entry.size
                ));
            }
            Ok((
                expected_sym.clone(),
                GlobalObject::new(entry.address, entry.size),
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Incoming::new(heap, globals).map_err(|error| error.to_string())
}

impl From<Region> for Incoming {
    fn from(heap: Region) -> Self {
        Incoming::heap_only(heap).expect("a heap-only incoming region must not wrap")
    }
}
