use std::io::{self, Read, Write};
use std::os::fd::FromRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use crate::oracle::{
    Access, CandidateState, CandidateWrite, LiveMemory as OracleLiveMemory, MaterializedAllocation,
    MemoryRegion, ObjectState, PostState, PreFailure, PreVerdict, RegionKind, TrackedObject,
};

use austen_lib::gen_dsl::rand_like::MAX_SIZE_LIMIT;
use austen_lib::gen_dsl::{
    Env, GenDef, GenModule, Incoming, ParseOutput, PublishedGlobal, ReconstructError, Region,
    SizeArgError, Sym, SymMap, TapeParser, TestCaseSize, Value, bind_size_args,
    incoming_from_published,
};
use austen_lib::spec::{FunctionSpec, Post, SpecModule};
use austen_lib::wire::{
    ALLOW_UNSTABLE_ADDRESSES_ENV, ByteUse, CONSTRAINT_FALSE_KIND, COVER_ENV, CoverPage,
    DEBUG_CHILD_ENV, DEFAULT_HEAP_BYTES, EXTERNAL_INPUT_BYTES, EdgeSet, GENERATOR_ENV,
    HEAP_BASE_ENV, HEAP_BYTES_ENV, HF_AUTO_ENV, HF_PERSISTENT_ENV, HF_STDIN_FALLBACK_ENV,
    HarnessFunction, HarnessModule, HeapBase, INCOMING_ENV, IncomingGlobal, IncomingInfo, Mode,
    ORACLE_STATUS_ENV, OracleStatusAccess, OracleStatusMap, POINTER_READING, PreStatus, TALLY_ENV,
    TALLY_MAGIC, TallyAccess, TallyMap, Trust, ValidationFailure, ValidationObservation,
    ValidationParse, ValidationPrecondition, decode_validation_frames, parse_function_selector,
    parse_size_selector,
    tally::{SLOT_DISPATCHED, SLOT_MAGIC, SLOT_POST_FAILED, SLOT_SEEN, SLOT_SELECTED},
    with_source, with_source_accounted,
};
use libafl_bolts::shmem::{ShMemProvider, UnixShMemProvider};
#[cfg(not(target_os = "linux"))]
use libafl_targets::MaybePersistentForkserverParent;
use libafl_targets::{
    __afl_map_size, EDGES_MAP_ALLOCATED_SIZE, EDGES_MAP_PTR, ForkserverState, INPUT_LENGTH_PTR,
    INPUT_PTR, MAX_EDGES_FOUND, SHM_FUZZING, map_input_shared_memory, map_shared_memory,
    sanitizer_cov_pc_table, start_forkserver,
};

pub mod oracle;

#[cfg(target_os = "linux")]
mod persistent;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AustenTarget {
    pub n_functions: usize,
    pub arities: *const usize,
    pub call: unsafe extern "C" fn(fn_id: usize, args: *const i128, n_args: usize) -> i128,
}

pub const LINK_FACTS_VERSION: u32 = 1;

pub const LINK_FACTS_CHECK_HEAP_OWNERSHIP: u32 = 1;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalLinkFact {
    pub sym: u64,
    pub address: u64,
    pub size: u64,
}

#[repr(C)]
pub struct LinkFacts {
    pub size: usize,
    pub version: u32,
    pub flags: u32,
    pub layout: *const u8,
    pub layout_len: usize,
    pub globals: *const GlobalLinkFact,
    pub global_count: usize,
    pub target: *const AustenTarget,
    pub driver: Option<InProcessDriver>,
    pub hooks: *const PersistentHooks,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LinkFactsError {
    NullFacts,
    UnalignedFacts,
    FactsAddressOverflow,
    IncompatibleSize { found: usize, expected: usize },
    IncompatibleVersion { found: u32, expected: u32 },
    InvalidFlags { found: u32 },
    NullTarget,
    UnalignedTarget,
    TargetAddressOverflow,
    LayoutTooLarge { bytes: usize },
    NullLayout,
    LayoutAddressOverflow,
    LayoutNotUtf8,
    GlobalTableSizeOverflow { count: usize },
    GlobalTableTooLarge { bytes: usize },
    NullGlobalTable,
    UnalignedGlobalTable,
    GlobalTableAddressOverflow,
    UnalignedHooks,
    HooksAddressOverflow,
}

impl std::fmt::Display for LinkFactsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NullFacts => f.write_str("NULL link facts"),
            Self::UnalignedFacts => f.write_str("unaligned link facts"),
            Self::FactsAddressOverflow => f.write_str("link facts address range overflows"),
            Self::IncompatibleSize { found, expected } => write!(
                f,
                "incompatible link facts size {found}; expected exactly {expected}"
            ),
            Self::IncompatibleVersion { found, expected } => write!(
                f,
                "incompatible link facts version {found}; expected {expected}"
            ),
            Self::InvalidFlags { found } => write!(
                f,
                "invalid link facts flags {found:#x}; supported bits: {LINK_FACTS_CHECK_HEAP_OWNERSHIP:#x}"
            ),
            Self::NullTarget => f.write_str("NULL target"),
            Self::UnalignedTarget => f.write_str("unaligned target"),
            Self::TargetAddressOverflow => f.write_str("target address range overflows"),
            Self::LayoutTooLarge { bytes } => {
                write!(f, "global layout id is too large ({bytes} bytes)")
            }
            Self::NullLayout => f.write_str("NULL global layout id"),
            Self::LayoutAddressOverflow => f.write_str("global layout id address range overflows"),
            Self::LayoutNotUtf8 => f.write_str("global layout id is not UTF-8"),
            Self::GlobalTableSizeOverflow { count } => {
                write!(f, "global table size overflows for {count} entries")
            }
            Self::GlobalTableTooLarge { bytes } => {
                write!(f, "global table is too large ({bytes} bytes)")
            }
            Self::NullGlobalTable => f.write_str("NULL global table"),
            Self::UnalignedGlobalTable => f.write_str("unaligned global table"),
            Self::GlobalTableAddressOverflow => f.write_str("global table address range overflows"),
            Self::UnalignedHooks => f.write_str("unaligned persistent hooks"),
            Self::HooksAddressOverflow => f.write_str("persistent hooks address range overflows"),
        }
    }
}

struct ValidatedLinkFacts<'a> {
    target: AustenTarget,
    check_heap_ownership: bool,
    driver: Option<InProcessDriver>,
    hooks: Option<&'a PersistentHooks>,
    globals: LinkedGlobals,
}

struct LinkedGlobals {
    layout: String,
    entries: Vec<GlobalLinkFact>,
}

impl AustenTarget {
    fn links(&self, fn_id: usize) -> bool {
        fn_id < self.n_functions
    }

    unsafe fn arity(&self, fn_id: usize) -> usize {
        unsafe { *self.arities.add(fn_id) }
    }

    unsafe fn call(&self, fn_id: usize, args: *const i128, n_args: usize) -> i128 {
        unsafe { (self.call)(fn_id, args, n_args) }
    }
}

pub type InProcessCallback = extern "C" fn(*const u8, usize) -> core::ffi::c_int;

pub type InProcessDriver = unsafe extern "C" fn(
    *mut core::ffi::c_int,
    *mut *mut *mut core::ffi::c_char,
    InProcessCallback,
) -> core::ffi::c_int;

pub type PersistentHook = unsafe extern "C" fn();

#[repr(transparent)]
pub struct ClearNewCoverage(pub PersistentHook);

#[repr(transparent)]
pub struct ResetLocalCoverage(pub PersistentHook);

#[repr(transparent)]
pub struct ResetStackDepth(pub PersistentHook);

#[repr(transparent)]
pub struct CountCounters(pub PersistentHook);

#[repr(transparent)]
pub struct CheckStackDepth(pub PersistentHook);

#[repr(C)]
pub struct PersistentHooks {
    pub clear_new_coverage: ClearNewCoverage,
    pub reset_local_coverage: ResetLocalCoverage,
    pub reset_stack_depth: ResetStackDepth,
    pub count_counters: CountCounters,
    pub check_stack_depth: CheckStackDepth,
}

const _: () = {
    assert!(std::mem::size_of::<PersistentHooks>() == 5 * std::mem::size_of::<PersistentHook>());
    assert!(std::mem::align_of::<PersistentHooks>() == std::mem::align_of::<PersistentHook>());
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionParseError {
    Header(austen_lib::wire::InputHeaderError),
    UnknownFunction { target_id: u8 },
    UnlinkedFunction { target_id: u8 },
}

impl std::fmt::Display for FunctionParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Header(error) => error.fmt(f),
            Self::UnknownFunction { target_id } => {
                write!(f, "function selector {target_id} names no staged function")
            }
            Self::UnlinkedFunction { target_id } => {
                write!(f, "function selector {target_id} names no linked function")
            }
        }
    }
}

impl std::error::Error for FunctionParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Header(error) => Some(error),
            Self::UnknownFunction { .. } | Self::UnlinkedFunction { .. } => None,
        }
    }
}

impl From<austen_lib::wire::InputHeaderError> for FunctionParseError {
    fn from(error: austen_lib::wire::InputHeaderError) -> Self {
        Self::Header(error)
    }
}

#[derive(Clone, Copy)]
pub struct FunctionParser<'m> {
    suite: &'m HarnessModule,
    target: AustenTarget,
}

impl<'m> FunctionParser<'m> {
    pub const fn new(suite: &'m HarnessModule, target: AustenTarget) -> Self {
        Self { suite, target }
    }

    pub fn parse<'b>(
        &self,
        bytes: &'b [u8],
    ) -> Result<(&'m HarnessFunction, &'b [u8]), FunctionParseError> {
        let (target_id, after_function) = parse_function_selector(bytes)?;
        let function = self
            .suite
            .functions
            .iter()
            .find(|function| function.target_id == target_id)
            .ok_or(FunctionParseError::UnknownFunction { target_id })?;
        if !self.target.links(usize::from(target_id)) {
            return Err(FunctionParseError::UnlinkedFunction { target_id });
        }
        Ok((function, after_function))
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SizeParser;

impl SizeParser {
    pub const fn new() -> Self {
        Self
    }

    pub fn parse<'b>(
        &self,
        bytes: &'b [u8],
    ) -> Result<(TestCaseSize, &'b [u8]), austen_lib::wire::InputHeaderError> {
        parse_size_selector(bytes)
    }
}

fn validate_wire_definition(generator: &GenDef, module: &GenModule) -> Result<(), String> {
    austen_lib::gen_dsl::validate_size_args(generator, module).map_err(|error| match error {
        SizeArgError::NotDeclared(size) => {
            format!("size argument {size:?} is not declared among the generator inputs")
        }
        SizeArgError::CollidesWithGlobal(size) => {
            format!("size argument {size:?} collides with a module global")
        }
        SizeArgError::WrongType(size) => format!(
            "size argument {size:?} cannot represent {MAX_SIZE_LIMIT} at the required type {:?}",
            austen_lib::gen_dsl::uintptr_bt()
        ),
    })?;
    for (param, _) in &generator.iargs {
        if !generator.size_args().any(|size| size == param) && module.find_global(param).is_none() {
            return Err(format!(
                "non-size argument {param:?} has no matching module global"
            ));
        }
    }
    Ok(())
}

fn resolve_wire_def(
    suite: &HarnessModule,
    function: &HarnessFunction,
    wire_defs: &mut SymMap<usize>,
) -> Result<(), String> {
    if wire_defs.contains_key(&function.generator) {
        return Ok(());
    }
    let generator = suite
        .generator_module
        .function_generator(&function.generator)
        .map_err(|error| {
            format!(
                "generator suite function {:?} has invalid function generator {:?}: {error}",
                function.name, function.generator
            )
        })?;
    validate_wire_definition(generator, &suite.generator_module).map_err(|error| {
        format!(
            "generator suite function {:?} has invalid wire definition: {error}",
            function.name
        )
    })?;
    let wire_at = suite
        .generator_module
        .defs
        .iter()
        .position(|(name, _)| name == &function.generator)
        .expect("function_generator resolved this definition");
    wire_defs.insert(function.generator.clone(), wire_at);
    Ok(())
}

fn validate_suite(suite: &HarnessModule, target: AustenTarget) -> Result<SymMap<usize>, String> {
    if suite.functions.is_empty() {
        return Err("generator suite contains no functions".into());
    }
    let mut seen = [false; 256];
    let mut wire_defs = SymMap::default();
    for function in &suite.functions {
        let target_id = usize::from(function.target_id);
        if seen[target_id] {
            return Err(format!(
                "generator suite contains duplicate target id {}",
                function.target_id
            ));
        }
        seen[target_id] = true;
        if !target.links(target_id) {
            return Err(format!(
                "generator suite function {:?} has target id {}, but the linked shim has {} functions",
                function.name, function.target_id, target.n_functions
            ));
        }
        resolve_wire_def(suite, function, &mut wire_defs)?;
        oracle_function(suite, function).ok_or_else(|| {
                format!(
                    "generator suite function {:?} at target id {} has no matching retained specification",
                    function.name, function.target_id
                )
            })?;
    }
    suite
        .generator_module
        .validate()
        .map_err(|error| format!("invalid generator module: {error}"))?;
    Ok(wire_defs)
}

struct Setup {
    incoming: Incoming,
    regions: Vec<MemoryRegion>,
    trust: Trust,
    module: HarnessModule,
    wire_defs: SymMap<usize>,
    target: AustenTarget,
    check_heap_ownership: bool,
    narrating: bool,
    cover_out: Option<std::path::PathBuf>,
    dirty: AtomicBool,
}

impl Setup {
    fn run_input(&self, mode: &str, bytes: &[u8], narrate_when: Narrate) {
        if self.dirty.load(Ordering::Relaxed) {
            reset_input_memory(&self.incoming, &self.dirty);
        }
        tally(TallyMilestone::Seen);
        let narrating = narrate_when == Narrate::Always || self.narrating;
        let outcome = run_one_with_incoming(
            bytes,
            self.trust,
            &self.incoming,
            &self.regions,
            &self.module,
            &self.wire_defs,
            self.target,
            self.check_heap_ownership,
            narrating,
            &self.dirty,
            || publish_cover(self),
        );
        if narrating {
            narrate(mode, bytes.len(), outcome);
        }
    }
}

fn reset_input_memory(incoming: &Incoming, dirty: &AtomicBool) {
    zero_heap(incoming.heap());
    for (_, object) in incoming.globals_in_order() {
        let bytes = object
            .startup()
            .expect("the harness snapshots every tracked global before dispatch");
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                object.region.base as *mut u8,
                bytes.len(),
            );
        }
    }
    dirty.store(false, Ordering::Relaxed);
}

fn zero_heap(heap: Region) {
    #[cfg(target_os = "linux")]
    if zero_heap_by_residency(heap) {
        return;
    }
    unsafe { std::ptr::write_bytes(heap.base as *mut u8, 0, heap.size as usize) };
}

#[cfg(target_os = "linux")]
const RESIDENCY_PAGES: usize = 4096;

#[cfg(target_os = "linux")]
fn zero_heap_by_residency(heap: Region) -> bool {
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let Ok(page) = usize::try_from(page) else {
        return false;
    };
    let Ok(size) = usize::try_from(heap.size) else {
        return false;
    };
    if page == 0
        || size == 0
        || !heap.base.is_multiple_of(page as u64)
        || !size.is_multiple_of(page)
    {
        return false;
    }
    let pages = size / page;
    if pages > RESIDENCY_PAGES {
        return false;
    }
    let mut resident = [0u8; RESIDENCY_PAGES];
    let scanned = unsafe {
        libc::mincore(
            heap.base as *mut libc::c_void,
            size,
            resident.as_mut_ptr().cast(),
        )
    };
    if scanned != 0 {
        return false;
    }
    reset_heap_pages(heap, page, &resident[..pages])
}

#[cfg(target_os = "linux")]
fn reset_heap_pages(heap: Region, page: usize, resident: &[u8]) -> bool {
    let mut at = 0;
    while at < resident.len() {
        let in_memory = resident[at] & 1 != 0;
        let from = at;
        while at < resident.len() && (resident[at] & 1 != 0) == in_memory {
            at += 1;
        }
        let base = unsafe { (heap.base as *mut u8).add(from * page) };
        let size = (at - from) * page;
        if in_memory {
            unsafe { std::ptr::write_bytes(base, 0, size) };
        } else {
            if unsafe { libc::madvise(base.cast(), size, libc::MADV_DONTNEED) } != 0 {
                return false;
            }
        }
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Narrate {
    Always,
    WhenAsked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunOutcome {
    NoSuchFunction,
    NotDispatched,
    Dispatched,
}

impl RunOutcome {
    fn token(self) -> &'static str {
        match self {
            Self::NoSuchFunction => "no-such-function",
            Self::NotDispatched => "not-dispatched",
            Self::Dispatched => "dispatched",
        }
    }
}

fn parse_input(
    bytes: &[u8],
    size: TestCaseSize,
    trust: Trust,
    incoming: &Incoming,
    generator: &GenDef,
    ctx: &GenModule,
    post_defaults: &austen_lib::gen_dsl::DefaultTypes,
) -> Result<ParseOutput, ReconstructError> {
    let parser = TapeParser::new(ctx, incoming);
    let mut initial_env = Env::new();
    bind_size_args(&mut initial_env, generator, size);
    with_source(bytes, |draws| match trust {
        Trust::Producer => parser.parse_trusted_from_with_defaults(
            &generator.body,
            initial_env,
            draws,
            post_defaults,
        ),
        Trust::Checked => {
            parser.parse_from_with_defaults(&generator.body, initial_env, draws, post_defaults)
        }
    })
}

fn parse_checked_accounted(
    bytes: &[u8],
    size: TestCaseSize,
    incoming: &Incoming,
    generator: &GenDef,
    ctx: &GenModule,
    post_defaults: &austen_lib::gen_dsl::DefaultTypes,
) -> (Result<ParseOutput, ReconstructError>, ByteUse) {
    let parser = TapeParser::new(ctx, incoming);
    let mut initial_env = Env::new();
    bind_size_args(&mut initial_env, generator, size);
    with_source_accounted(bytes, |draws| {
        parser.parse_from_with_defaults(&generator.body, initial_env, draws, post_defaults)
    })
}

fn parse_candidate_accounted(
    bytes: &[u8],
    size: TestCaseSize,
    incoming: &Incoming,
    generator: &GenDef,
    ctx: &GenModule,
    post_defaults: &austen_lib::gen_dsl::DefaultTypes,
) -> (Result<ParseOutput, ReconstructError>, ByteUse) {
    let parser = TapeParser::new(ctx, incoming);
    let mut initial_env = Env::new();
    bind_size_args(&mut initial_env, generator, size);
    with_source_accounted(bytes, |draws| {
        parser.parse_candidate_from_with_defaults(
            &generator.body,
            initial_env,
            draws,
            post_defaults,
        )
    })
}

#[derive(Clone, Copy)]
enum Discipline {
    Dispatch { trust: Trust },
    Validate,
}

impl Discipline {
    fn parse_rejection_status(self) -> PreStatus {
        match self {
            Self::Dispatch {
                trust: Trust::Producer,
            } => PreStatus::InternalError,
            Self::Dispatch {
                trust: Trust::Checked,
            }
            | Self::Validate => PreStatus::ParseRejected,
        }
    }
}

struct Judgment<'m> {
    function: Option<&'m HarnessFunction>,
    test_case_size: Option<TestCaseSize>,
    byte_use: Option<ByteUse>,
    checked_rejection: Option<ReconstructError>,
    rung: Rung<'m>,
}

#[allow(clippy::large_enum_variant)]
enum Rung<'m> {
    NoFunction,
    MissingGenerator,
    SizeRejected(austen_lib::wire::InputHeaderError),
    ParseRejected(ReconstructError),
    ArityMismatch,
    MissingSpecification,
    Judged {
        verdict: PreVerdict,
        pass: Option<Dispatchable<'m>>,
    },
}

struct Dispatchable<'m> {
    out: ParseOutput,
    args: Vec<i128>,
    spec_function: &'m FunctionSpec,
    fn_id: usize,
    allocations: Vec<MaterializedAllocation>,
    heap_claims: Vec<oracle::OwnershipExtent>,
}

#[allow(clippy::too_many_arguments)]
fn judge_input<'m>(
    bytes: &[u8],
    discipline: Discipline,
    incoming: &Incoming,
    regions: &[MemoryRegion],
    module: &'m HarnessModule,
    wire_defs: &SymMap<usize>,
    target: AustenTarget,
    memory: &dyn oracle::MemoryView,
) -> Judgment<'m> {
    let mut judgment = Judgment {
        function: None,
        test_case_size: None,
        byte_use: None,
        checked_rejection: None,
        rung: Rung::NoFunction,
    };
    let (function, after_function) = match FunctionParser::new(module, target).parse(bytes) {
        Ok(parts) => parts,
        Err(_) => return judgment,
    };
    judgment.function = Some(function);
    let fn_id = usize::from(function.target_id);
    let (test_case_size, tape) = match SizeParser::new().parse(after_function) {
        Ok(parts) => parts,
        Err(error) => {
            judgment.rung = Rung::SizeRejected(error);
            return judgment;
        }
    };
    judgment.test_case_size = Some(test_case_size);
    if let Discipline::Dispatch { .. } = discipline {
        tally(TallyMilestone::Selected);
    }
    let Some(generator) = wire_defs
        .get(&function.generator)
        .and_then(|at| module.generator_module.defs.get(*at))
        .filter(|(name, generator)| name == &function.generator && generator.spec)
        .map(|(_, generator)| generator)
    else {
        oracle_pre(PreStatus::InternalError);
        judgment.rung = Rung::MissingGenerator;
        return judgment;
    };
    let mut out = match discipline {
        Discipline::Dispatch { trust } => {
            let parsed = parse_input(
                tape,
                test_case_size,
                trust,
                incoming,
                generator,
                &module.generator_module,
                &function.post_defaults,
            );
            match parsed {
                Ok(out) => out,
                Err(error) => {
                    oracle_pre(discipline.parse_rejection_status());
                    judgment.rung = Rung::ParseRejected(error);
                    return judgment;
                }
            }
        }
        Discipline::Validate => {
            let (parsed, byte_use) = parse_checked_accounted(
                tape,
                test_case_size,
                incoming,
                generator,
                &module.generator_module,
                &function.post_defaults,
            );
            judgment.byte_use = Some(byte_use);
            match parsed {
                Ok(out) => out,
                Err(ReconstructError::FailedConstraint) => {
                    let (recovered, byte_use) = parse_candidate_accounted(
                        tape,
                        test_case_size,
                        incoming,
                        generator,
                        &module.generator_module,
                        &function.post_defaults,
                    );
                    judgment.byte_use = Some(byte_use);
                    match recovered {
                        Ok(out) => {
                            judgment.checked_rejection = Some(ReconstructError::FailedConstraint);
                            out
                        }
                        Err(_) => {
                            oracle_pre(PreStatus::ParseRejected);
                            judgment.rung = Rung::ParseRejected(ReconstructError::FailedConstraint);
                            return judgment;
                        }
                    }
                }
                Err(error) => {
                    oracle_pre(PreStatus::ParseRejected);
                    judgment.rung = Rung::ParseRejected(error);
                    return judgment;
                }
            }
        }
    };
    let Some(args) = dispatch_args(&out, fn_id, target) else {
        oracle_pre(PreStatus::InternalError);
        judgment.rung = Rung::ArityMismatch;
        return judgment;
    };
    let Some(spec_function) = oracle_function(module, function) else {
        oracle_pre(PreStatus::InternalError);
        judgment.rung = Rung::MissingSpecification;
        return judgment;
    };
    let (env, writes) = match discipline {
        Discipline::Dispatch { .. } => {
            let post_reads_bindings = !spec_function.ensures.is_assert_true();
            let env = if post_reads_bindings {
                out.env.clone()
            } else {
                std::mem::take(&mut out.env)
            };
            (env, out.writes.clone())
        }
        Discipline::Validate => (
            std::mem::take(&mut out.env),
            std::mem::take(&mut out.writes),
        ),
    };
    let candidate = CandidateState::new(env, candidate_writes(writes), regions, memory)
        .with_defaults(out.defaults.clone())
        .with_allocation_roots(&out.allocation_roots);
    let checked =
        oracle::check_precondition_with_allocations(&module.spec, spec_function, candidate);
    oracle_pre(pre_status(&checked.verdict));
    let pass = if matches!(discipline, Discipline::Dispatch { .. })
        && matches!(&checked.verdict, PreVerdict::Passed)
    {
        checked.apply_global_authorities(&mut out.env);
        Some(Dispatchable {
            out,
            args,
            spec_function,
            fn_id,
            allocations: checked.allocations,
            heap_claims: checked.heap_claims,
        })
    } else {
        None
    };
    let verdict = checked.verdict;
    judgment.rung = Rung::Judged { verdict, pass };
    judgment
}

#[allow(clippy::too_many_arguments)]
fn run_one_with_incoming<F>(
    bytes: &[u8],
    trust: Trust,
    incoming: &Incoming,
    regions: &[MemoryRegion],
    module: &HarnessModule,
    wire_defs: &SymMap<usize>,
    target: AustenTarget,
    check_heap_ownership: bool,
    narrating: bool,
    dirty: &AtomicBool,
    before_post_abort: F,
) -> RunOutcome
where
    F: FnOnce(),
{
    let memory = OracleLiveMemory;
    let judgment = judge_input(
        bytes,
        Discipline::Dispatch { trust },
        incoming,
        regions,
        module,
        wire_defs,
        target,
        &memory,
    );
    match judgment.rung {
        Rung::NoFunction | Rung::SizeRejected(_) => RunOutcome::NoSuchFunction,
        Rung::MissingGenerator
        | Rung::ParseRejected(_)
        | Rung::ArityMismatch
        | Rung::MissingSpecification
        | Rung::Judged { pass: None, .. } => RunOutcome::NotDispatched,
        Rung::Judged {
            pass: Some(pass), ..
        } => {
            let call = materialize_and_call(
                &pass.out,
                &pass.args,
                pass.fn_id,
                target,
                incoming.heap(),
                &pass.allocations,
                dirty,
            );
            check_oracle_post(
                &module.spec,
                pass.spec_function,
                pass.out.env,
                pass.out.defaults,
                call,
                regions,
                &memory,
                &pass.heap_claims,
                check_heap_ownership,
                narrating,
                before_post_abort,
            );
            RunOutcome::Dispatched
        }
    }
}

fn dispatch_args(out: &ParseOutput, fn_id: usize, target: AustenTarget) -> Option<Vec<i128>> {
    let args = out.args()?;
    if !target.links(fn_id) || args.len() != unsafe { target.arity(fn_id) } {
        return None;
    }
    Some(args)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetFreeAction {
    Ignore,
    ReleaseSynthetic,
    Delegate,
    Abort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetAllocation {
    Live(usize),
    Foreign,
    Invalid,
}

struct TargetAllocationLedger {
    heap: Option<Region>,
    active: bool,
    objects: Vec<TrackedObject>,
    cleanup: Vec<u64>,
}

impl TargetAllocationLedger {
    const EMPTY: TargetAllocationLedger = TargetAllocationLedger {
        heap: None,
        active: false,
        objects: Vec::new(),
        cleanup: Vec::new(),
    };

    fn activate(&mut self, heap: Region, allocations: &[MaterializedAllocation]) {
        self.heap = Some(heap);
        self.active = true;
        self.objects.clear();
        self.cleanup.clear();
        self.objects
            .extend(allocations.iter().map(|allocation| allocation.tracked()));
    }

    fn record_allocation(&mut self, addr: u64, size: u64, owned: bool) {
        if !self.active || addr == 0 {
            return;
        }
        self.objects.push(if owned {
            TrackedObject::owned_allocation(addr, size)
        } else {
            TrackedObject::allocation(addr, size)
        });
        if owned {
            self.cleanup.push(addr);
        }
    }

    fn allocation(&self, addr: u64) -> TargetAllocation {
        if self.active {
            let mut interior_or_released = false;
            for (index, object) in self.objects.iter().enumerate().rev() {
                if matches!(object.state, ObjectState::Live(_)) && object.base == addr {
                    return TargetAllocation::Live(index);
                }
                interior_or_released |= object.base == addr || object.contains(addr);
            }
            if interior_or_released {
                return TargetAllocation::Invalid;
            }
        }
        if self.heap.is_some_and(|heap| heap.contains(addr)) {
            TargetAllocation::Invalid
        } else {
            TargetAllocation::Foreign
        }
    }

    fn action(&mut self, addr: u64) -> TargetFreeAction {
        if addr == 0 {
            return TargetFreeAction::Ignore;
        }
        let resolution = self.allocation(addr);
        self.release_resolved(addr, resolution)
    }

    fn release_resolved(&mut self, addr: u64, resolution: TargetAllocation) -> TargetFreeAction {
        match resolution {
            TargetAllocation::Live(index) => {
                if let Some(index) = self.cleanup.iter().position(|base| *base == addr) {
                    self.cleanup.swap_remove(index);
                }
                let object = &mut self.objects[index];
                let synthetic = matches!(object.state, ObjectState::Live(Access::Raw(_)));
                object.state = ObjectState::Released;
                if synthetic {
                    TargetFreeAction::ReleaseSynthetic
                } else {
                    TargetFreeAction::Delegate
                }
            }
            TargetAllocation::Foreign => TargetFreeAction::Delegate,
            TargetAllocation::Invalid => TargetFreeAction::Abort,
        }
    }

    fn realloc_action(&mut self, addr: u64, size: libc::size_t) -> TargetReallocAction {
        if addr == 0 {
            return TargetReallocAction::Allocate;
        }
        match self.allocation(addr) {
            TargetAllocation::Invalid => TargetReallocAction::Abort,
            resolution if size == 0 => {
                TargetReallocAction::Release(self.release_resolved(addr, resolution))
            }
            TargetAllocation::Live(index) => {
                let object = &self.objects[index];
                TargetReallocAction::Resize {
                    synthetic_size: matches!(object.state, ObjectState::Live(Access::Raw(_)))
                        .then_some(object.size as usize),
                }
            }
            TargetAllocation::Foreign => TargetReallocAction::Resize {
                synthetic_size: None,
            },
        }
    }

    fn record_realloc(
        &mut self,
        old: u64,
        result: TargetAllocationResult,
        owned: bool,
    ) -> *mut libc::c_void {
        if !result.pointer.is_null() {
            let _ = self.action(old);
        }
        result.record(self, owned)
    }

    fn deactivate(&mut self) -> CallAllocations {
        if !self.active {
            std::process::abort();
        }
        self.active = false;
        CallAllocations {
            objects: std::mem::take(&mut self.objects),
            cleanup: std::mem::take(&mut self.cleanup),
        }
    }
}

struct TargetAllocationResult {
    pointer: *mut libc::c_void,
    size: Option<libc::size_t>,
}

impl TargetAllocationResult {
    fn malloc(size: libc::size_t) -> Self {
        Self {
            pointer: unsafe { libc::malloc(size) },
            size: Some(size),
        }
    }

    fn calloc(count: libc::size_t, size: libc::size_t) -> Self {
        Self {
            pointer: unsafe { libc::calloc(count, size) },
            size: count.checked_mul(size),
        }
    }

    fn aligned_alloc(alignment: libc::size_t, size: libc::size_t) -> Self {
        Self {
            pointer: unsafe { libc::aligned_alloc(alignment, size) },
            size: Some(size),
        }
    }

    fn record(self, ledger: &mut TargetAllocationLedger, owned: bool) -> *mut libc::c_void {
        if let Some(size) = self.size {
            ledger.record_allocation(self.pointer as usize as u64, size as u64, owned);
        }
        self.pointer
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetReallocAction {
    Allocate,
    Release(TargetFreeAction),
    Resize { synthetic_size: Option<usize> },
    Abort,
}

impl TargetReallocAction {
    unsafe fn execute(
        self,
        pointer: *mut libc::c_void,
        size: libc::size_t,
    ) -> TargetAllocationResult {
        let result = match self {
            Self::Allocate => return TargetAllocationResult::malloc(size),
            Self::Release(action) => {
                unsafe { apply_target_free(pointer, action) };
                std::ptr::null_mut()
            }
            Self::Resize {
                synthetic_size: Some(old_size),
            } => {
                let result = unsafe { libc::malloc(size) };
                if !result.is_null() {
                    unsafe { libc::memcpy(result, pointer, size.min(old_size)) };
                }
                result
            }
            Self::Resize {
                synthetic_size: None,
            } => unsafe { libc::realloc(pointer, size) },
            Self::Abort => std::process::abort(),
        };
        TargetAllocationResult {
            pointer: result,
            size: Some(size),
        }
    }
}

static TARGET_ALLOCATION_LEDGER: Mutex<TargetAllocationLedger> =
    Mutex::new(TargetAllocationLedger::EMPTY);

fn target_allocation_ledger() -> MutexGuard<'static, TargetAllocationLedger> {
    TARGET_ALLOCATION_LEDGER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn austen_harness_target_free(pointer: *mut libc::c_void) {
    let action = target_allocation_ledger().action(pointer as usize as u64);
    unsafe { apply_target_free(pointer, action) };
}

unsafe fn apply_target_free(pointer: *mut libc::c_void, action: TargetFreeAction) {
    match action {
        TargetFreeAction::Ignore | TargetFreeAction::ReleaseSynthetic => {}
        TargetFreeAction::Delegate => unsafe { libc::free(pointer) },
        TargetFreeAction::Abort => std::process::abort(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn austen_harness_target_malloc(size: libc::size_t) -> *mut libc::c_void {
    let result = TargetAllocationResult::malloc(size);
    result.record(&mut target_allocation_ledger(), false)
}

#[unsafe(no_mangle)]
pub extern "C" fn austen_harness_target_malloc_owned(size: libc::size_t) -> *mut libc::c_void {
    let result = TargetAllocationResult::malloc(size);
    result.record(&mut target_allocation_ledger(), true)
}

#[unsafe(no_mangle)]
pub extern "C" fn austen_harness_target_calloc(
    count: libc::size_t,
    size: libc::size_t,
) -> *mut libc::c_void {
    let result = TargetAllocationResult::calloc(count, size);
    result.record(&mut target_allocation_ledger(), false)
}

#[unsafe(no_mangle)]
pub extern "C" fn austen_harness_target_calloc_owned(
    count: libc::size_t,
    size: libc::size_t,
) -> *mut libc::c_void {
    let result = TargetAllocationResult::calloc(count, size);
    result.record(&mut target_allocation_ledger(), true)
}

#[unsafe(no_mangle)]
pub extern "C" fn austen_harness_target_aligned_alloc(
    alignment: libc::size_t,
    size: libc::size_t,
) -> *mut libc::c_void {
    let result = TargetAllocationResult::aligned_alloc(alignment, size);
    result.record(&mut target_allocation_ledger(), false)
}

#[unsafe(no_mangle)]
pub extern "C" fn austen_harness_target_aligned_alloc_owned(
    alignment: libc::size_t,
    size: libc::size_t,
) -> *mut libc::c_void {
    let result = TargetAllocationResult::aligned_alloc(alignment, size);
    result.record(&mut target_allocation_ledger(), true)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn austen_harness_target_realloc(
    pointer: *mut libc::c_void,
    size: libc::size_t,
) -> *mut libc::c_void {
    unsafe { target_realloc(pointer, size, false) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn austen_harness_target_realloc_owned(
    pointer: *mut libc::c_void,
    size: libc::size_t,
) -> *mut libc::c_void {
    unsafe { target_realloc(pointer, size, true) }
}

unsafe fn target_realloc(
    pointer: *mut libc::c_void,
    size: libc::size_t,
    owned: bool,
) -> *mut libc::c_void {
    let addr = pointer as usize as u64;
    let action = target_allocation_ledger().realloc_action(addr, size);
    let result = unsafe { action.execute(pointer, size) };
    target_allocation_ledger().record_realloc(addr, result, owned)
}

struct CallAllocations {
    objects: Vec<TrackedObject>,
    cleanup: Vec<u64>,
}

impl Drop for CallAllocations {
    fn drop(&mut self) {
        for base in self.cleanup.drain(..) {
            unsafe { libc::free(base as *mut libc::c_void) };
        }
    }
}

struct CallResult {
    ret: i128,
    allocations: CallAllocations,
}

fn materialize_and_call(
    out: &ParseOutput,
    args: &[i128],
    fn_id: usize,
    target: AustenTarget,
    heap: Region,
    allocations: &[MaterializedAllocation],
    dirty: &AtomicBool,
) -> CallResult {
    dirty.store(true, Ordering::Relaxed);
    for w in &out.writes {
        unsafe {
            std::ptr::copy_nonoverlapping(w.bytes.as_ptr(), w.addr as *mut u8, w.bytes.len());
        }
    }
    target_allocation_ledger().activate(heap, allocations);
    tally(TallyMilestone::Dispatched);
    let ret = unsafe { target.call(fn_id, args.as_ptr(), args.len()) };
    let allocations = target_allocation_ledger().deactivate();
    CallResult { ret, allocations }
}

fn oracle_function<'a>(
    suite: &'a HarnessModule,
    function: &HarnessFunction,
) -> Option<&'a FunctionSpec> {
    suite
        .spec
        .functions
        .get(usize::from(function.target_id))
        .filter(|spec_function| spec_function.name == function.name)
}

pub fn candidate_writes(writes: Vec<austen_lib::gen_dsl::AsgnWrite>) -> Vec<CandidateWrite> {
    writes
        .into_iter()
        .map(|write| CandidateWrite {
            addr: write.addr,
            bytes: write.bytes,
        })
        .collect()
}

fn pre_status(verdict: &PreVerdict) -> PreStatus {
    match verdict {
        PreVerdict::Passed => PreStatus::Passed,
        PreVerdict::Failed(_) => PreStatus::Failed,
        PreVerdict::Unsupported(_) => PreStatus::Unsupported,
        PreVerdict::InternalError(_) => PreStatus::InternalError,
    }
}

pub fn oracle_regions(incoming: &Incoming) -> Vec<MemoryRegion> {
    let heap = incoming.heap();
    let mut regions = Vec::with_capacity(1 + incoming.globals().len());
    regions.push(MemoryRegion {
        base: heap.base,
        size: heap.size,
        writable: true,
        kind: RegionKind::Heap,
    });
    regions.extend(
        incoming
            .globals_in_order()
            .map(|(sym, object)| MemoryRegion {
                base: object.region.base,
                size: object.region.size,
                writable: object.writable,
                kind: RegionKind::Global(sym.clone()),
            }),
    );
    regions
}

#[allow(clippy::too_many_arguments)]
fn check_oracle_post<F>(
    module: &SpecModule,
    function: &FunctionSpec,
    env: SymMap<Value>,
    defaults: austen_lib::spec::DefaultChoices,
    call: CallResult,
    regions: &[MemoryRegion],
    memory: &dyn oracle::MemoryView,
    heap_claims: &[oracle::OwnershipExtent],
    check_heap_ownership: bool,
    narrating: bool,
    before_abort: F,
) where
    F: FnOnce(),
{
    let state = PostState::new(env, call.ret, regions, memory)
        .with_defaults(defaults)
        .with_objects(&call.allocations.objects)
        .with_heap_claims(heap_claims)
        .with_leak_check(check_heap_ownership);
    let verdict = oracle::check_postcondition(module, function, state);
    drop(call);
    match verdict {
        oracle::PostVerdict::Passed => {}
        oracle::PostVerdict::Failed(failure) => {
            tally(TallyMilestone::PostFailed);
            before_abort();
            eprintln!("austen-harness: postcondition failed: {failure:?}");
            std::process::abort();
        }
        oracle::PostVerdict::Unsupported(error) => {
            oracle_post_unsupported();
            if narrating {
                eprintln!(
                    "austen-harness: postcondition unsupported: {}",
                    error.reason
                );
            }
        }
    }
}

fn decode_generator(json: &str) -> Result<HarnessModule, serde_json::Error> {
    austen_lib::gen_dsl::json::from_str(json)
}

fn load_generator() -> HarnessModule {
    let Some(path) = std::env::var_os(GENERATOR_ENV) else {
        eprintln!("austen-harness: {GENERATOR_ENV} is not set; nothing to parse inputs with");
        std::process::exit(2);
    };
    let json = match std::fs::read_to_string(&path) {
        Ok(json) => json,
        Err(e) => {
            eprintln!("austen-harness: read {GENERATOR_ENV} {path:?}: {e}");
            std::process::exit(2);
        }
    };
    match decode_generator(&json) {
        Ok(module) => module,
        Err(e) => {
            eprintln!("austen-harness: parse {GENERATOR_ENV} {path:?}: {e}");
            std::process::exit(2);
        }
    }
}

unsafe fn read_afl_input() -> &'static [u8] {
    unsafe {
        let len = *INPUT_LENGTH_PTR as usize;
        core::slice::from_raw_parts(INPUT_PTR, len)
    }
}

fn narrate(mode: &str, bytes: usize, outcome: RunOutcome) {
    eprintln!(
        "austen-harness: ran {mode} input ({bytes} bytes, {})",
        outcome.token()
    );
}

pub const MAX_LOOP: u32 = 10_000;

fn exit_when_the_forkserver_parent_does() {
    #[cfg(target_os = "linux")]
    {
        let parent = unsafe { libc::getppid() };
        unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) };
        if unsafe { libc::getppid() } != parent || parent == 1 {
            std::process::exit(0);
        }
    }
}

fn run_forkserver(setup: &Setup, driver: Option<InProcessDriver>) -> ! {
    let mut shm = attach_afl_coverage(
        driver,
        "a forkserver campaign steers by — build it without `--engine=libfuzzer` \
         to drive it with AFL",
        refuse_forkserver,
    );
    if map_input_shared_memory(&mut shm).is_err() {
        refuse_forkserver("the input map could not be attached (AFL_NO_SHM_FUZZ?)")
    }
    unsafe { SHM_FUZZING = 1 };

    let map = unsafe { EDGES_MAP_PTR };

    #[cfg(target_os = "linux")]
    {
        let channel = match persistent::Channel::new() {
            Ok(channel) => channel,
            Err(e) => refuse_forkserver(&format!("the direct channel could not be set up: {e}")),
        };
        let run_child = || -> std::convert::Infallible {
            let mut child = PersistentChild::enter(map);
            let own_pid = unsafe { libc::getpid() }.cast_unsigned();
            let mut first = true;
            loop {
                if child.serve_one(setup) {
                    std::process::exit(0);
                }
                if std::mem::take(&mut first)
                    && channel.await_release() == persistent::Next::DriverGone
                {
                    std::process::exit(0);
                }
                match channel.finish_input(own_pid) {
                    persistent::Next::Input => {}
                    persistent::Next::DriverGone => std::process::exit(0),
                }
            }
        };
        let mut parent = persistent::DirectParent::new(&channel, &run_child, setup.narrating);
        match start_forkserver(&mut parent) {
            Ok(ForkserverState::Child) => match run_child() {},
            Ok(ForkserverState::NoAfl) => {
                refuse_forkserver("the forkserver file descriptors are absent")
            }
            Err(e) => refuse_forkserver(&format!("the forkserver session ended: {e}")),
        }
    }
    #[cfg(not(target_os = "linux"))]
    match start_forkserver(&mut MaybePersistentForkserverParent::new()) {
        Ok(ForkserverState::Child) => {
            let mut child = PersistentChild::enter(map);
            loop {
                if child.serve_one(setup) {
                    std::process::exit(0);
                }
                unsafe { libc::raise(libc::SIGSTOP) };
            }
        }
        Ok(ForkserverState::NoAfl) => {
            refuse_forkserver("the forkserver file descriptors are absent")
        }
        Err(e) => refuse_forkserver(&format!("the forkserver session ended: {e}")),
    }
}

struct PersistentChild {
    map: *mut u8,
    cycles: u32,
}

impl PersistentChild {
    fn enter(map: *mut u8) -> PersistentChild {
        exit_when_the_forkserver_parent_does();
        unsafe { core::ptr::write_bytes(map, 0, __afl_map_size) };
        PersistentChild { map, cycles: 0 }
    }

    fn serve_one(&mut self, setup: &Setup) -> bool {
        unsafe { *self.map = 1 };
        let bytes = unsafe { read_afl_input() };
        setup.run_input("forkserver", bytes, Narrate::WhenAsked);
        self.cycles += 1;
        self.cycles >= MAX_LOOP
    }
}

fn run_afl_one_shot(setup: &Setup, driver: Option<InProcessDriver>) -> ! {
    attach_afl_coverage(
        driver,
        "AFL showmap reads — build it without `--engine=libfuzzer` to measure it \
         with AFL",
        refuse_afl_one_shot,
    );

    let bytes = read_stdin();
    unsafe { *EDGES_MAP_PTR = 1 };
    setup.run_input("afl one-shot", &bytes, Narrate::WhenAsked);
    std::process::exit(0);
}

fn attach_afl_coverage(
    driver: Option<InProcessDriver>,
    preempted: &str,
    refuse: fn(&str) -> !,
) -> UnixShMemProvider {
    if driver.is_some() {
        refuse(&format!(
            "this harness links an in-process engine's runtime, whose coverage \
             callbacks preempt the ones {preempted}"
        ))
    }
    let Ok(mut shm) = UnixShMemProvider::new() else {
        refuse("no shared-memory provider")
    };
    if map_shared_memory(&mut shm).is_err() {
        refuse("the coverage map could not be attached")
    }
    shm
}

fn refuse_afl_one_shot(why: &str) -> ! {
    eprintln!(
        "austen-harness: not running an AFL one-shot input ({why}); \
         refusing to dispatch against a private coverage map instead"
    );
    std::process::exit(2);
}

fn refuse_forkserver(why: &str) -> ! {
    eprintln!(
        "austen-harness: not running a forkserver ({why}); \
         refusing to dispatch a standalone input instead"
    );
    std::process::exit(2);
}

const HF_INPUT_FD: libc::c_int = 1021;
const HF_PERSISTENT_FD: libc::c_int = 1023;
const HF_READY_TAG: u8 = b'R';

fn refuse_persistent(why: &str) -> ! {
    eprintln!(
        "austen-harness: {} declares honggfuzz's persistent transport, but {why}; \
         refusing to wait on descriptors nobody is serving",
        HF_PERSISTENT_ENV
    );
    std::process::exit(2);
}

fn refuse_hf_auto(why: &str) -> ! {
    eprintln!(
        "austen-harness: {HF_AUTO_ENV} could not select a safe honggfuzz \
         transport ({why}); refusing to guess between descriptors and stdin"
    );
    std::process::exit(2);
}

#[derive(Debug)]
enum HfDescriptorError {
    PersistenceNotEnabled,
    Invalid(String),
}

fn verify_hf_descriptors() -> Result<(), HfDescriptorError> {
    let check = |fd, want, kind| {
        let mut st: libc::stat = unsafe { core::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut st) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if st.st_mode & libc::S_IFMT != want {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "fd {fd} is open but is not {kind}, so this honggfuzz does not lay \
                     its descriptors out where libhfuzz/fetch.c says"
                ),
            ));
        }
        Ok(())
    };

    match check(HF_PERSISTENT_FD, libc::S_IFSOCK, "a socket") {
        Ok(()) => {}
        Err(error) if error.raw_os_error() == Some(libc::EBADF) => {
            return Err(HfDescriptorError::PersistenceNotEnabled);
        }
        Err(error) => {
            return Err(HfDescriptorError::Invalid(
                if error.kind() == io::ErrorKind::InvalidData {
                    error.to_string()
                } else {
                    format!("fd {HF_PERSISTENT_FD} could not be inspected ({error})")
                },
            ));
        }
    }

    check(HF_INPUT_FD, libc::S_IFREG, "a regular file").map_err(|error| {
        HfDescriptorError::Invalid(format!(
            "fd {HF_INPUT_FD} is not a usable regular input file ({error})"
        ))
    })
}

fn mark_hf_stdin_fallback() -> Result<(), String> {
    let path = std::env::var_os(HF_STDIN_FALLBACK_ENV)
        .ok_or_else(|| format!("{HF_STDIN_FALLBACK_ENV} does not name the fallback marker"))?;
    let path = std::path::PathBuf::from(path);
    match std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
    {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(format!("could not create {} ({error})", path.display())),
    }
}

fn run_hf_persistent(setup: &Setup, hooks: Option<&PersistentHooks>) -> ! {
    match verify_hf_descriptors() {
        Ok(()) => run_hf_persistent_verified(setup, hooks),
        Err(HfDescriptorError::PersistenceNotEnabled) => refuse_persistent(&format!(
            "fd {HF_PERSISTENT_FD} is not open — was -P dropped from honggfuzz's argv?"
        )),
        Err(HfDescriptorError::Invalid(why)) => refuse_persistent(&why),
    }
}

fn run_hf_auto(setup: &Setup, hooks: Option<&PersistentHooks>) -> ! {
    match verify_hf_descriptors() {
        Ok(()) => run_hf_persistent_verified(setup, hooks),
        Err(HfDescriptorError::PersistenceNotEnabled) => {
            if let Err(why) = mark_hf_stdin_fallback() {
                refuse_hf_auto(&why);
            }
            run_standalone(setup)
        }
        Err(HfDescriptorError::Invalid(why)) => refuse_hf_auto(&why),
    }
}

fn run_hf_persistent_verified(setup: &Setup, hooks: Option<&PersistentHooks>) -> ! {
    let input = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            EXTERNAL_INPUT_BYTES,
            libc::PROT_READ,
            libc::MAP_SHARED,
            HF_INPUT_FD,
            0,
        )
    };
    if input == libc::MAP_FAILED {
        refuse_persistent(&format!(
            "the input window could not be mapped ({})",
            io::Error::last_os_error()
        ));
    }
    let input = input as *const u8;

    let mut sock = unsafe { std::fs::File::from_raw_fd(HF_PERSISTENT_FD) };

    if let Some(hooks) = hooks {
        unsafe { (hooks.clear_new_coverage.0)() };
    }

    let mut cycles: u32 = 0;
    let mut raw = [0u8; 8];
    loop {
        if sock.write_all(&[HF_READY_TAG]).is_err() || sock.read_exact(&mut raw).is_err() {
            std::process::exit(0);
        }
        let len = u64::from_ne_bytes(raw) as usize;
        if len > EXTERNAL_INPUT_BYTES {
            refuse_persistent(&format!(
                "honggfuzz offered {len} bytes against the {} this runtime maps; \
                 the staged runtime is out of step with the adapter's -F",
                EXTERNAL_INPUT_BYTES
            ));
        }

        let bytes = unsafe { core::slice::from_raw_parts(input, len) };
        if let Some(hooks) = hooks {
            unsafe {
                (hooks.reset_local_coverage.0)();
                (hooks.reset_stack_depth.0)();
            }
        }
        setup.run_input("persistent", bytes, Narrate::WhenAsked);
        if let Some(hooks) = hooks {
            unsafe {
                (hooks.count_counters.0)();
                (hooks.check_stack_depth.0)();
            }
        }
        cycles += 1;
        if cycles >= MAX_LOOP {
            std::process::exit(0);
        }
    }
}

static mut TALLY: *mut core::sync::atomic::AtomicU64 = std::ptr::null_mut();

static mut ORACLE_STATUS: *const OracleStatusMap = std::ptr::null();

fn setup_tally() {
    let Some(path) = std::env::var_os(TALLY_ENV) else {
        return;
    };
    let Some(page) = TallyMap::open(std::path::Path::new(&path), TallyAccess::ReadWrite) else {
        return;
    };
    let slots = page.into_raw();
    unsafe {
        (*slots.add(SLOT_MAGIC)).store(TALLY_MAGIC, core::sync::atomic::Ordering::Relaxed);
        TALLY = slots;
    }
}

fn setup_oracle_status() {
    let Some(path) = std::env::var_os(ORACLE_STATUS_ENV) else {
        return;
    };
    let Some(page) =
        OracleStatusMap::open(std::path::Path::new(&path), OracleStatusAccess::ReadWrite)
    else {
        return;
    };
    let page = Box::leak(Box::new(page));
    unsafe { ORACLE_STATUS = page };
}

fn with_oracle_page(record: impl FnOnce(&OracleStatusMap)) {
    let pointer = unsafe { ORACLE_STATUS };
    if let Some(page) = unsafe { pointer.as_ref() } {
        record(page);
    }
}

fn oracle_pre(status: PreStatus) {
    with_oracle_page(|page| page.record_pre(status));
}

fn oracle_post_unsupported() {
    with_oracle_page(OracleStatusMap::record_post_unsupported);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TallyMilestone {
    Seen,
    Selected,
    Dispatched,
    PostFailed,
}

impl TallyMilestone {
    fn slot(self) -> usize {
        match self {
            Self::Seen => SLOT_SEEN,
            Self::Selected => SLOT_SELECTED,
            Self::Dispatched => SLOT_DISPATCHED,
            Self::PostFailed => SLOT_POST_FAILED,
        }
    }

    fn below(self) -> Option<Self> {
        match self {
            Self::Seen => None,
            Self::Selected => Some(Self::Seen),
            Self::Dispatched => Some(Self::Selected),
            Self::PostFailed => Some(Self::Dispatched),
        }
    }
}

fn tally(milestone: TallyMilestone) {
    let slots = unsafe { TALLY };
    if slots.is_null() {
        return;
    }
    use core::sync::atomic::Ordering::Relaxed;
    let reached = unsafe { (*slots.add(milestone.slot())).fetch_add(1, Relaxed) } + 1;

    debug_assert!(
        milestone
            .below()
            .is_none_or(|below| { reached <= unsafe { (*slots.add(below.slot())).load(Relaxed) } }),
        "tally milestone {milestone:?} outran the rung below it"
    );
}

fn map_heap(hint: u64, size: u64) -> *mut libc::c_void {
    #[allow(unused_mut)]
    let mut flags = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS;
    #[cfg(target_os = "linux")]
    if hint != 0 {
        flags |= libc::MAP_FIXED_NOREPLACE;
    }
    let base = unsafe {
        libc::mmap(
            hint as *mut libc::c_void,
            size as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            flags,
            -1,
            0,
        )
    };
    if hint != 0 && base != libc::MAP_FAILED && base as u64 != hint {
        unsafe { libc::munmap(base, size as usize) };
        return libc::MAP_FAILED;
    }
    base
}

fn setup_heap() -> Region {
    let size = std::env::var(HEAP_BYTES_ENV)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_HEAP_BYTES);
    let placement = HeapBase::from_env().unwrap_or_else(|e| {
        eprintln!("austen-harness: {e}");
        std::process::exit(2);
    });
    let (hint, required) = match placement {
        HeapBase::Hint(addr) => (addr, false),
        HeapBase::Required(addr) => (addr, true),
        HeapBase::Kernel => (0, false),
    };

    let mut base = map_heap(hint, size);
    if base == libc::MAP_FAILED && hint != 0 {
        if required {
            eprintln!(
                "austen-harness: cannot map the pointer heap at fixed base {hint:#x} \
                 ({size} bytes); choose another nonzero {HEAP_BASE_ENV}, or unset it and use \
                 --allow-unstable-addresses (standalone harness: \
                 {ALLOW_UNSTABLE_ADDRESSES_ENV}=1) to permit unstable placement"
            );
            std::process::exit(2);
        }
        eprintln!(
            "austen-harness: fixed heap base {hint:#x} unavailable; using a \
             kernel-chosen base (absolute-pointer tapes will not replay \
             across processes)"
        );
        base = map_heap(0, size);
    }
    if base == libc::MAP_FAILED {
        eprintln!("austen-harness: mmap of {size} bytes for the pointer heap failed");
        std::process::exit(2);
    }
    Region {
        base: base as u64,
        size,
    }
}

fn resolve_incoming(
    heap: Region,
    ctx: &GenModule,
    linked: &LinkedGlobals,
) -> Result<Incoming, String> {
    incoming_from_published(
        ctx,
        "linked",
        &linked.layout,
        heap,
        linked.entries.iter().map(|fact| PublishedGlobal {
            sym: Sym {
                name: None,
                num: fact.sym,
            },
            address: fact.address,
            size: fact.size,
        }),
    )
}

fn publish_incoming(incoming: &Incoming, layout: &str) {
    let Some(path) = std::env::var_os(INCOMING_ENV) else {
        return;
    };
    let globals = incoming
        .globals_in_order()
        .map(|(sym, object)| IncomingGlobal {
            sym: sym.clone(),
            address: object.region.base,
            size: object.region.size,
        })
        .collect();
    let info = IncomingInfo::new(layout.to_string(), incoming.heap(), globals);
    if publish_atomically(std::path::Path::new(&path), info.to_line().as_bytes()).is_err() {
        eprintln!("austen-harness: failed to publish incoming info to {path:?}");
        std::process::exit(2);
    }
}

fn snapshot_globals(incoming: &mut Incoming) -> Result<(), String> {
    let snapshots = incoming
        .globals()
        .iter()
        .map(|(sym, object)| {
            let region = object.region;
            let size = usize::try_from(region.size).map_err(|_| {
                format!(
                    "global {sym:?} at {:#x} is too large to snapshot",
                    region.base
                )
            })?;
            let bytes =
                unsafe { std::slice::from_raw_parts(region.base as *const u8, size) }.to_vec();
            Ok((sym.clone(), bytes))
        })
        .collect::<Result<Vec<_>, String>>()?;
    for (sym, bytes) in snapshots {
        incoming
            .set_global_startup(&sym, bytes)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn publish_atomically(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn austen_oracle_capability() -> u32 {
    std::hint::black_box(1)
}

fn run_core(
    target: AustenTarget,
    driver: Option<InProcessDriver>,
    hooks: Option<&PersistentHooks>,
    linked_globals: LinkedGlobals,
    check_heap_ownership: bool,
) -> ! {
    let _ = austen_oracle_capability();
    let mode = read_mode();
    let heap = setup_heap();
    setup_tally();
    setup_oracle_status();
    let cover_out = std::env::var_os(COVER_ENV).map(std::path::PathBuf::from);
    if cover_out.is_some() && mode != Mode::Standalone {
        refuse_cover(mode);
    }
    let module = load_generator();
    let wire_defs = validate_suite(&module, target).unwrap_or_else(|e| {
        eprintln!("austen-harness: invalid generator suite: {e}");
        std::process::exit(2);
    });
    let mut incoming = resolve_incoming(heap, &module.generator_module, &linked_globals)
        .unwrap_or_else(|e| {
            eprintln!("austen-harness: invalid linked globals: {e}");
            std::process::exit(2);
        });
    snapshot_globals(&mut incoming).unwrap_or_else(|e| {
        eprintln!("austen-harness: cannot snapshot linked globals: {e}");
        std::process::exit(2);
    });
    publish_incoming(&incoming, &linked_globals.layout);
    let setup = Setup {
        regions: oracle_regions(&incoming),
        incoming,
        trust: read_trust(),
        module,
        wire_defs,
        target,
        check_heap_ownership,
        narrating: std::env::var_os(DEBUG_CHILD_ENV).is_some(),
        cover_out,
        dirty: AtomicBool::new(false),
    };

    match mode {
        Mode::Echo => echo(&setup),
        Mode::Validate => validate_stream(&setup),
        Mode::Forkserver => run_forkserver(&setup, driver),
        Mode::AflOneShot => run_afl_one_shot(&setup, driver),
        Mode::HfPersistent => run_hf_persistent(&setup, hooks),
        Mode::HfAuto => run_hf_auto(&setup, hooks),
        Mode::LibFuzzer => run_libfuzzer(&setup, driver),
        Mode::Standalone => run_standalone(&setup),
    }
}

fn validate_stream(setup: &Setup) -> ! {
    let framed = read_stdin();
    let inputs = decode_validation_frames(&framed).unwrap_or_else(|error| {
        eprintln!("austen-harness: invalid validation stream: {error}");
        std::process::exit(2);
    });
    let memory = OracleLiveMemory;
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    for (candidate, bytes) in inputs.into_iter().enumerate() {
        let observation =
            validate_candidate(setup, candidate as u64, bytes, &setup.regions, &memory);

        if serde_json::to_writer(&mut stdout, &observation).is_err()
            || stdout.write_all(b"\n").is_err()
        {
            eprintln!("austen-harness: failed to write validation observation");
            std::process::exit(2);
        }
    }
    std::process::exit(0);
}

fn parse_validation_failure(error: &ReconstructError) -> ValidationFailure {
    let failure = |kind: &str| ValidationFailure::new(kind, error.to_string());
    match error {
        ReconstructError::Default(default) => match default {
            austen_lib::gen_dsl::DefaultError::UnsupportedBaseType { bt } => {
                failure("unsupported-default-type").field("base_type", format!("{bt:?}"))
            }
            austen_lib::gen_dsl::DefaultError::UnrepresentableWidth { bits } => {
                failure("default-width-out-of-range").field("bits", u64::from(*bits))
            }
            austen_lib::gen_dsl::DefaultError::MissingChoice { bt } => {
                failure("missing-input-default").field("base_type", format!("{bt:?}"))
            }
            austen_lib::gen_dsl::DefaultError::ConflictingChoice { bt } => {
                failure("conflicting-input-default").field("base_type", format!("{bt:?}"))
            }
            austen_lib::gen_dsl::DefaultError::ValueOutOfRange { bt, value, bits } => {
                failure("default-value-out-of-range")
                    .field("base_type", format!("{bt:?}"))
                    .field("value", value.to_string())
                    .field("bits", u64::from(*bits))
            }
        },
        ReconstructError::UnsupportedBaseType { bt } => {
            failure("unsupported-base-type").field("base_type", format!("{bt:?}"))
        }
        ReconstructError::UnsizedCtype { ct } => {
            failure("unsized-c-type").field("c_type", format!("{ct:?}"))
        }
        ReconstructError::AsgnOutOfBounds { addr, size } => failure("write-out-of-bounds")
            .field("address", *addr)
            .field("size", *size),
        ReconstructError::AsgnOverlap { addr, size } => failure("write-overlap")
            .field("address", *addr)
            .field("size", *size),
        ReconstructError::UnrepresentableWidth { label, bits } => failure("width-out-of-range")
            .field("label", u64::from(label.0))
            .field("bits", u64::from(*bits)),
        ReconstructError::Unevaluatable { context } => {
            failure("unevaluatable").field("context", *context)
        }
        ReconstructError::FailedConstraint => failure(CONSTRAINT_FALSE_KIND),
        ReconstructError::UnknownGenerator { fsym } => {
            failure("unknown-generator").field("symbol", format!("{fsym:?}"))
        }
        ReconstructError::CallArityMismatch {
            fsym,
            expected,
            actual,
        } => failure("call-arity-mismatch")
            .field("symbol", format!("{fsym:?}"))
            .field("expected", *expected as u64)
            .field("actual", *actual as u64),
        ReconstructError::CallDepthExceeded { fsym } => {
            failure("call-depth-exceeded").field("symbol", format!("{fsym:?}"))
        }
        ReconstructError::UnsupportedMapDomain => failure("unsupported-map-domain"),
        ReconstructError::MapDomainTooLarge { lo, hi } => failure("map-domain-too-large")
            .field("lower", lo.to_string())
            .field("upper", hi.to_string()),
        ReconstructError::ScopeMerge(source) => {
            failure("scope-merge").field("source", source.to_string())
        }
        ReconstructError::StructChoice { label } => {
            failure("aggregate-choice-mismatch").field("label", u64::from(label.0))
        }
        ReconstructError::Truncated { label } => {
            failure("truncated").field("label", u64::from(label.0))
        }
        ReconstructError::ValueOutOfRange { label, value, bits } => failure("value-out-of-range")
            .field("label", u64::from(label.0))
            .field("value", value.to_string())
            .field("bits", u64::from(*bits)),
        ReconstructError::ChoiceMismatch {
            label,
            expected,
            actual,
        } => failure("choice-mismatch")
            .field("label", u64::from(label.0))
            .field("expected", expected.to_string())
            .field("actual", actual.to_string()),
        ReconstructError::TrailingBytes { count } => {
            failure("trailing-bytes").field("count", *count as u64)
        }
        ReconstructError::TrailingValues { count } => {
            failure("trailing-values").field("count", *count as u64)
        }
    }
}

fn size_validation_failure(error: austen_lib::wire::InputHeaderError) -> ValidationFailure {
    match error {
        austen_lib::wire::InputHeaderError::MissingSize { actual } => ValidationFailure::new(
            "truncated-test-case-size",
            format!(
                "test-case size selector requires {} bytes, but only {actual} remain",
                austen_lib::wire::SIZE_SELECTOR_BYTES,
            ),
        )
        .field("expected", austen_lib::wire::SIZE_SELECTOR_BYTES as u64)
        .field("available", actual as u64),
        austen_lib::wire::InputHeaderError::MissingFunction => ValidationFailure::new(
            "missing-function-selector",
            "input has no function selector byte",
        ),
    }
}

fn pre_validation_failure(failure: &PreFailure) -> ValidationFailure {
    let detail = |kind: &str| ValidationFailure::new(kind, format!("{failure:?}"));
    match failure {
        PreFailure::NullWrite { write } => detail("null-write").field("write", *write as u64),
        PreFailure::UnauthorizedWrite { write } => {
            detail("unauthorized-write").field("write", *write as u64)
        }
        PreFailure::OverlappingWrites { first, second } => detail("overlapping-writes")
            .field("first", *first as u64)
            .field("second", *second as u64),
        PreFailure::UnclaimedWrite { write } => {
            detail("unclaimed-write").field("write", *write as u64)
        }
        PreFailure::MissingBinding { symbol } => {
            detail("missing-binding").field("symbol", format!("{symbol:?}"))
        }
        PreFailure::ConstraintFalse { step } => {
            detail(CONSTRAINT_FALSE_KIND).field("step", *step as u64)
        }
        PreFailure::NullTake { step } => detail("null-take").field("step", *step as u64),
        PreFailure::UnauthorizedTake { step } => {
            detail("unauthorized-take").field("step", *step as u64)
        }
        PreFailure::OverlappingTake { step } => {
            detail("overlapping-take").field("step", *step as u64)
        }
        PreFailure::ResourceValueMismatch { step, symbol } => detail("resource-value-mismatch")
            .field("step", *step as u64)
            .field("symbol", format!("{symbol:?}")),
    }
}

fn validate_candidate(
    setup: &Setup,
    candidate: u64,
    bytes: &[u8],
    regions: &[MemoryRegion],
    memory: &dyn oracle::MemoryView,
) -> ValidationObservation {
    let mut observation = ValidationObservation {
        candidate,
        input_bytes: bytes.len() as u64,
        function_id: None,
        function: None,
        test_case_size: None,
        trust: Trust::Checked.name().into(),
        pointers: POINTER_READING.into(),
        byte_use: ByteUse::default(),
        parse: ValidationParse::NoFunction,
        precondition: ValidationPrecondition::NotRun,
    };
    let judgment = judge_input(
        bytes,
        Discipline::Validate,
        &setup.incoming,
        regions,
        &setup.module,
        &setup.wire_defs,
        setup.target,
        memory,
    );
    if let Some(function) = judgment.function {
        observation.function_id = Some(function.target_id);
        observation.function = Some(function.name.clone());
    }
    observation.test_case_size = judgment.test_case_size.map(TestCaseSize::get);
    if let Some(byte_use) = judgment.byte_use {
        observation.byte_use = byte_use;
    }
    observation.parse = match (&judgment.rung, &judgment.checked_rejection) {
        (Rung::NoFunction, _) => ValidationParse::NoFunction,
        (Rung::SizeRejected(error), _) => ValidationParse::Rejected {
            failure: size_validation_failure(*error),
        },
        (Rung::ParseRejected(error), _) | (_, Some(error)) => ValidationParse::Rejected {
            failure: parse_validation_failure(error),
        },
        (
            Rung::MissingGenerator
            | Rung::ArityMismatch
            | Rung::MissingSpecification
            | Rung::Judged { .. },
            None,
        ) => ValidationParse::Passed,
    };
    observation.precondition = match judgment.rung {
        Rung::NoFunction | Rung::SizeRejected(_) | Rung::ParseRejected(_) => {
            ValidationPrecondition::NotRun
        }
        Rung::MissingGenerator => ValidationPrecondition::InternalError {
            failure: ValidationFailure::new(
                "missing-function-generator",
                "staged function has no matching function generator",
            ),
        },
        Rung::ArityMismatch => ValidationPrecondition::InternalError {
            failure: ValidationFailure::new(
                "target-arity-mismatch",
                "parsed arguments disagree with the linked target arity",
            ),
        },
        Rung::MissingSpecification => ValidationPrecondition::InternalError {
            failure: ValidationFailure::new(
                "missing-function-specification",
                "staged function has no matching retained specification",
            ),
        },
        Rung::Judged { verdict, .. } => match verdict {
            PreVerdict::Passed => ValidationPrecondition::Passed,
            PreVerdict::Failed(failure) => ValidationPrecondition::Failed {
                failure: pre_validation_failure(&failure),
            },
            PreVerdict::Unsupported(error) => ValidationPrecondition::Unsupported {
                failure: ValidationFailure::new("unsupported", error.reason),
            },
            PreVerdict::InternalError(error) => ValidationPrecondition::InternalError {
                failure: ValidationFailure::new("internal-error", error.reason),
            },
        },
    };
    observation
}

static mut SETUP: *const Setup = std::ptr::null();

fn run_libfuzzer(setup: &Setup, driver: Option<InProcessDriver>) -> ! {
    let Some(driver) = driver else {
        eprintln!(
            "austen-harness: {} is set, but this harness carries no in-process \
             driver — it was linked without libclang_rt.fuzzer_no_main, so there \
             is no loop to hand the inputs to. Rebuild it with \
             `austen test --engine=libfuzzer`, or unset {} to run one input from \
             stdin.",
            austen_lib::wire::LIBFUZZER_ENV,
            austen_lib::wire::LIBFUZZER_ENV,
        );
        std::process::exit(2);
    };

    unsafe { SETUP = setup as *const Setup };

    let args: Vec<std::ffi::CString> = std::env::args_os()
        .map(|arg| std::ffi::CString::new(arg.as_encoded_bytes()).unwrap_or_default())
        .collect();
    let mut ptrs: Vec<*mut core::ffi::c_char> =
        args.iter().map(|a| a.as_ptr().cast_mut()).collect();
    ptrs.push(std::ptr::null_mut());
    let mut argc = args.len() as core::ffi::c_int;
    let mut argv = ptrs.as_mut_ptr();

    let code = unsafe { driver(&mut argc, &mut argv, run_one_input) };
    std::process::exit(code);
}

extern "C" fn run_one_input(data: *const u8, size: usize) -> core::ffi::c_int {
    let setup: &Setup = unsafe { &*SETUP };

    let bytes = if size == 0 {
        &[][..]
    } else {
        unsafe { core::slice::from_raw_parts(data, size) }
    };

    setup.run_input("libfuzzer", bytes, Narrate::WhenAsked);
    0
}

fn run_standalone(setup: &Setup) -> ! {
    let bytes = read_stdin();
    if setup.cover_out.is_some() {
        unsafe { core::ptr::write_bytes(EDGES_MAP_PTR, 0, EDGES_MAP_ALLOCATED_SIZE) };
    }
    setup.run_input("standalone", &bytes, Narrate::Always);
    publish_cover(setup);
    std::process::exit(0);
}

fn refuse_cover(mode: Mode) -> ! {
    eprintln!(
        "austen-harness: {COVER_ENV} asks this run to publish the edges it \
         touched, which only a standalone run can do — this one is {mode:?}, \
         where the coverage map is the fuzzer's rather than ours"
    );
    std::process::exit(2);
}

fn pc_table_entries() -> Option<u64> {
    const PER_ENTRY: usize = size_of::<libafl_targets::PcTableEntry>() / size_of::<usize>();
    let words: usize = sanitizer_cov_pc_table().map(<[_]>::len).sum();
    (words > 0).then_some((words / PER_ENTRY) as u64)
}

fn publish_cover(setup: &Setup) {
    let Some(path) = setup.cover_out.as_deref() else {
        return;
    };
    let map = unsafe { core::slice::from_raw_parts(EDGES_MAP_PTR, EDGES_MAP_ALLOCATED_SIZE) };
    let page = CoverPage {
        map_bytes: EDGES_MAP_ALLOCATED_SIZE as u64,
        registered: unsafe { MAX_EDGES_FOUND } as u64,
        sites: pc_table_entries(),
        edges: EdgeSet::from_map(map),
    };
    if publish_atomically(path, &page.to_bytes()).is_err() {
        eprintln!("austen-harness: failed to publish coverage to {path:?}");
    }
}

fn read_mode() -> Mode {
    Mode::from_env().unwrap_or_else(|e| {
        eprintln!("austen-harness: {e}");
        std::process::exit(2);
    })
}

fn read_trust() -> Trust {
    Trust::from_env().unwrap_or_else(|e| {
        eprintln!("austen-harness: {e}");
        std::process::exit(2);
    })
}

fn read_stdin() -> Vec<u8> {
    let mut bytes = Vec::new();
    if io::stdin().read_to_end(&mut bytes).is_err() {
        eprintln!("austen-harness: failed to read stdin");
        std::process::exit(2);
    }
    bytes
}

fn echo_rejection(
    function: Option<&HarnessFunction>,
    test_case_size: Option<TestCaseSize>,
    tape: Option<&[u8]>,
    stage: &'static str,
    message: String,
) -> ! {
    println!(
        "{}",
        serde_json::json!({
            "function_id": function.map(|function| function.target_id),
            "function": function.map(|function| function.name.as_str()),
            "test_case_size": test_case_size.map(TestCaseSize::get),
            "tape": tape,
            "error": {
                "stage": stage,
                "message": message,
            },
        })
    );
    eprintln!("austen-harness: echo {stage} failed: {message}");
    std::process::exit(1);
}

fn echo(setup: &Setup) -> ! {
    let bytes = read_stdin();

    let (function, after_function) =
        match FunctionParser::new(&setup.module, setup.target).parse(&bytes) {
            Ok(parts) => parts,
            Err(error) => {
                echo_rejection(None, None, None, "selection", error.to_string());
            }
        };
    let (test_case_size, tape) = match SizeParser::new().parse(after_function) {
        Ok(parts) => parts,
        Err(error) => {
            echo_rejection(
                Some(function),
                None,
                None,
                "size selection",
                size_validation_failure(error).message,
            );
        }
    };
    let wire_at = setup
        .wire_defs
        .get(&function.generator)
        .expect("startup validation indexed every staged wire definition");
    let generator = &setup
        .module
        .generator_module
        .defs
        .get(*wire_at)
        .expect("startup validation retained every indexed wire definition")
        .1;

    match parse_input(
        tape,
        test_case_size,
        setup.trust,
        &setup.incoming,
        generator,
        &setup.module.generator_module,
        &function.post_defaults,
    ) {
        Ok(out) => {
            let writes: Vec<_> = out
                .writes
                .iter()
                .map(|w| serde_json::json!({ "addr": w.addr, "bytes": w.bytes }))
                .collect();
            println!(
                "{}",
                serde_json::json!({
                    "function_id": function.target_id,
                    "function": function.name,
                    "test_case_size": test_case_size,
                    "tape": tape,
                    "args": out.args(),
                    "writes": writes,
                    "env": env_json(&out.env),
                    "defaults": out.defaults,
                    "post": post_json(&function.post),
                })
            );
        }
        Err(e) => {
            echo_rejection(
                Some(function),
                Some(test_case_size),
                Some(tape),
                "parse",
                e.to_string(),
            );
        }
    }
    std::process::exit(0);
}

fn env_json(env: &SymMap<Value>) -> serde_json::Value {
    serde_json::Value::Array(
        env.iter()
            .map(|(sym, value)| serde_json::json!({ "sym": sym, "value": value }))
            .collect(),
    )
}

fn post_json(post: &Post) -> serde_json::Value {
    let (plan, reason) = match post {
        Post::Check => ("check", None),
        Post::Unsupported { reason } => ("unsupported", Some(reason.as_str())),
    };
    serde_json::json!({
        "plan": plan,
        "reason": reason,
    })
}

unsafe fn validate_link_facts<'a>(
    facts: *const LinkFacts,
) -> Result<ValidatedLinkFacts<'a>, LinkFactsError> {
    if facts.is_null() {
        return Err(LinkFactsError::NullFacts);
    }
    if !(facts as usize).is_multiple_of(std::mem::align_of::<LinkFacts>()) {
        return Err(LinkFactsError::UnalignedFacts);
    }
    (facts as usize)
        .checked_add(std::mem::size_of::<LinkFacts>())
        .ok_or(LinkFactsError::FactsAddressOverflow)?;

    let written_size = unsafe { facts.cast::<usize>().read() };
    let expected_size = std::mem::size_of::<LinkFacts>();
    if written_size != expected_size {
        return Err(LinkFactsError::IncompatibleSize {
            found: written_size,
            expected: expected_size,
        });
    }
    let facts = unsafe { &*facts };
    if facts.version != LINK_FACTS_VERSION {
        return Err(LinkFactsError::IncompatibleVersion {
            found: facts.version,
            expected: LINK_FACTS_VERSION,
        });
    }
    if facts.flags & !LINK_FACTS_CHECK_HEAP_OWNERSHIP != 0 {
        return Err(LinkFactsError::InvalidFlags { found: facts.flags });
    }

    if facts.target.is_null() {
        return Err(LinkFactsError::NullTarget);
    }
    if !(facts.target as usize).is_multiple_of(std::mem::align_of::<AustenTarget>()) {
        return Err(LinkFactsError::UnalignedTarget);
    }
    (facts.target as usize)
        .checked_add(std::mem::size_of::<AustenTarget>())
        .ok_or(LinkFactsError::TargetAddressOverflow)?;

    if facts.layout_len > isize::MAX as usize {
        return Err(LinkFactsError::LayoutTooLarge {
            bytes: facts.layout_len,
        });
    }
    let layout_bytes = if facts.layout_len == 0 {
        &[][..]
    } else {
        if facts.layout.is_null() {
            return Err(LinkFactsError::NullLayout);
        }
        (facts.layout as usize)
            .checked_add(facts.layout_len)
            .ok_or(LinkFactsError::LayoutAddressOverflow)?;
        unsafe { std::slice::from_raw_parts(facts.layout, facts.layout_len) }
    };
    let layout = std::str::from_utf8(layout_bytes)
        .map_err(|_| LinkFactsError::LayoutNotUtf8)?
        .to_string();

    let globals_bytes = facts
        .global_count
        .checked_mul(std::mem::size_of::<GlobalLinkFact>())
        .ok_or(LinkFactsError::GlobalTableSizeOverflow {
            count: facts.global_count,
        })?;
    if globals_bytes > isize::MAX as usize {
        return Err(LinkFactsError::GlobalTableTooLarge {
            bytes: globals_bytes,
        });
    }
    let entries = if facts.global_count == 0 {
        Vec::new()
    } else {
        if facts.globals.is_null() {
            return Err(LinkFactsError::NullGlobalTable);
        }
        if !(facts.globals as usize).is_multiple_of(std::mem::align_of::<GlobalLinkFact>()) {
            return Err(LinkFactsError::UnalignedGlobalTable);
        }
        (facts.globals as usize)
            .checked_add(globals_bytes)
            .ok_or(LinkFactsError::GlobalTableAddressOverflow)?;
        unsafe { std::slice::from_raw_parts(facts.globals, facts.global_count) }.to_vec()
    };

    let hooks = if facts.hooks.is_null() {
        None
    } else {
        if !(facts.hooks as usize).is_multiple_of(std::mem::align_of::<PersistentHooks>()) {
            return Err(LinkFactsError::UnalignedHooks);
        }
        (facts.hooks as usize)
            .checked_add(std::mem::size_of::<PersistentHooks>())
            .ok_or(LinkFactsError::HooksAddressOverflow)?;
        Some(unsafe { &*facts.hooks })
    };

    Ok(ValidatedLinkFacts {
        target: unsafe { *facts.target },
        check_heap_ownership: facts.flags & LINK_FACTS_CHECK_HEAP_OWNERSHIP != 0,
        driver: facts.driver,
        hooks,
        globals: LinkedGlobals { layout, entries },
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn austen_harness_main_with_link_facts(facts: *const LinkFacts) -> ! {
    let facts = unsafe { validate_link_facts(facts) }
        .unwrap_or_else(|error| panic!("austen_harness_main_with_link_facts: {error}"));
    run_core(
        facts.target,
        facts.driver,
        facts.hooks,
        facts.globals,
        facts.check_heap_ownership,
    )
}
