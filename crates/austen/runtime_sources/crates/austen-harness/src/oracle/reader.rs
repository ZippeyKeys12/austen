use austen_lib::spec::{
    ArgType, BaseType, Ensures, FunctionSpec, IndexTerm, Init, Lat, PointerOrigin, PredicateDef,
    Request, RequestName, Scalar, Sctype, SpecModule, Sym, SymMap, Value, check_owned_request,
    check_predicate_call, check_quantified_permission, check_quantified_request,
};

use super::memory::pointer_origin;
use super::{
    Access, Bounds, CandidateState, CandidateWrite, MaterializedAllocation, MemoryAccessError,
    MemoryRegion, MemoryView, ObjectState, OwnershipExtent, PostState, RegionKind, TrackedObject,
};
use austen_lib::spec::eval::{
    bits, boolean, check_constraint_with_defaults as check_constraint, decode,
    eval_with_defaults as eval, finite_domain_with_defaults as finite_domain, pointer_scalar,
    size_of, struct_layout,
};

const MAX_PREDICATE_DEPTH: usize = 128;
const EXTENT_PREFLIGHT_THRESHOLD: i128 = 4096;
static EMPTY_DEFAULTS: austen_lib::spec::DefaultChoices = austen_lib::spec::DefaultChoices::new();

mod provenance;
mod quantified;
use provenance::{ValueProvenance, seed_provenance};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    pub reason: String,
}

impl Unsupported {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalError {
    pub reason: String,
}

impl InternalError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreFailure {
    NullWrite { write: usize },
    UnauthorizedWrite { write: usize },
    OverlappingWrites { first: usize, second: usize },
    UnclaimedWrite { write: usize },
    MissingBinding { symbol: Sym },
    ConstraintFalse { step: usize },
    NullTake { step: usize },
    UnauthorizedTake { step: usize },
    OverlappingTake { step: usize },
    ResourceValueMismatch { step: usize, symbol: Sym },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostFailure {
    ConstraintFalse { step: usize },
    NullTake { step: usize },
    UnauthorizedTake { step: usize },
    OverlappingTake { step: usize },
    OutOfBoundsTake { step: usize },
    FreedTake { step: usize },
    LeakedOwnership { intervals: Vec<OwnershipExtent> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreVerdict {
    Passed,
    Failed(PreFailure),
    Unsupported(Unsupported),
    InternalError(InternalError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostVerdict {
    Passed,
    Failed(PostFailure),
    Unsupported(Unsupported),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalPointerAuthority {
    pub root: u64,
    pub global: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreconditionCheck {
    pub verdict: PreVerdict,
    pub allocations: Vec<MaterializedAllocation>,
    pub heap_claims: Vec<OwnershipExtent>,
    pub global_authorities: Vec<GlobalPointerAuthority>,
}

impl PreconditionCheck {
    pub fn apply_global_authorities(&self, bindings: &mut SymMap<Value>) {
        if !matches!(&self.verdict, PreVerdict::Passed) {
            debug_assert!(self.global_authorities.is_empty());
            return;
        }
        if self.global_authorities.is_empty() {
            return;
        }

        for value in bindings.values_mut() {
            for scalar in value.scalars_mut() {
                if let Scalar::Pointer { addr, origin } = scalar
                    && *origin == PointerOrigin::Heap
                    && let Some(authority) = self
                        .global_authorities
                        .iter()
                        .find(|authority| authority.root == *addr)
                {
                    *origin = PointerOrigin::Global(authority.global);
                }
            }
        }
    }
}

pub fn check_precondition(
    module: &SpecModule,
    function: &FunctionSpec,
    state: CandidateState<'_>,
) -> PreVerdict {
    check_precondition_with_allocations(module, function, state).verdict
}

pub fn check_precondition_with_allocations(
    module: &SpecModule,
    function: &FunctionSpec,
    state: CandidateState<'_>,
) -> PreconditionCheck {
    let rejected = |verdict| PreconditionCheck {
        verdict,
        allocations: Vec::new(),
        heap_claims: Vec::new(),
        global_authorities: Vec::new(),
    };
    if let Err(error) = validate_regions(state.regions) {
        return rejected(PreVerdict::InternalError(error));
    }
    let ordered_writes = match validate_writes(&state.writes, state.regions) {
        Ok(ordered) => ordered,
        Err(failure) => return rejected(PreVerdict::Failed(failure)),
    };

    let mut env = state.bindings;
    bind_globals(&mut env, state.regions);
    if let Some(internal) = &function.internal
        && let Some(symbol) = missing_argument(internal, &env)
    {
        return rejected(PreVerdict::Failed(PreFailure::MissingBinding { symbol }));
    }

    let overlay = Overlay {
        base: state.memory,
        writes: &state.writes,
        ordered: ordered_writes,
    };
    let mut provenance = seed_provenance(&env, state.allocation_roots);
    let mut reader = Reader::new(
        module,
        state.regions,
        &overlay,
        Phase::Pre {
            allocation_roots: state.allocation_roots,
        },
    );
    reader.defaults = &state.defaults;
    if let Some(internal) = &function.internal {
        if let Err(error) = reader.walk(internal.lat(), &mut env, &mut provenance, 0) {
            return rejected(pre_error(error));
        }
        for (index, write) in state.writes.iter().enumerate() {
            let Ok(size) = u64::try_from(write.bytes.len()) else {
                return rejected(PreVerdict::InternalError(InternalError::new(
                    "candidate write length does not fit u64",
                )));
            };
            let extent = OwnershipExtent {
                base: write.addr,
                size,
            };
            if reader.claims.get(index) != Some(&extent) && !reader.claims.contains(&extent) {
                return rejected(PreVerdict::Failed(PreFailure::UnclaimedWrite {
                    write: index,
                }));
            }
        }
    } else if !state.writes.is_empty() {
        return rejected(PreVerdict::Failed(PreFailure::UnclaimedWrite { write: 0 }));
    }
    let allocations = materialized_allocations(
        state.allocation_roots,
        &reader.allocation_claims,
        state.regions,
    );
    reader
        .global_authorities
        .sort_unstable_by_key(|authority| authority.root);
    PreconditionCheck {
        verdict: PreVerdict::Passed,
        allocations,
        heap_claims: reader
            .allocation_claims
            .iter()
            .map(|claim| claim.extent)
            .collect(),
        global_authorities: reader.global_authorities,
    }
}

pub fn check_postcondition(
    module: &SpecModule,
    function: &FunctionSpec,
    state: PostState<'_>,
) -> PostVerdict {
    let ensures = &function.ensures;
    if ensures.is_assert_true() {
        return post_ownership_verdict(state.leak_check, state.heap_claims, state.objects, &[]);
    }
    if let Err(error) = validate_regions(state.regions) {
        return PostVerdict::Unsupported(Unsupported::new(error.reason));
    }
    let mut env = state.bindings;
    bind_globals(&mut env, state.regions);
    let mut reader = Reader::new(
        module,
        state.regions,
        state.memory,
        Phase::Post {
            objects: state.objects,
            require_owned: state.leak_check,
        },
    );
    reader.defaults = &state.defaults;
    let value = match reader.return_value(ensures, state.return_carrier) {
        Ok(value) => value,
        Err(error) => return post_error(error),
    };
    env.insert(ensures.var.0.clone(), value);
    let mut provenance = SymMap::new();
    match reader.walk(&ensures.body, &mut env, &mut provenance, 0) {
        Ok(()) => post_ownership_verdict(
            state.leak_check,
            state.heap_claims,
            state.objects,
            &reader.claims,
        ),
        Err(error) => post_error(error),
    }
}

fn post_ownership_verdict(
    leak_check: bool,
    heap_claims: &[OwnershipExtent],
    objects: &[TrackedObject],
    post_claims: &[OwnershipExtent],
) -> PostVerdict {
    if !leak_check {
        return PostVerdict::Passed;
    }
    let mut residual = match ResidualOwnership::new(heap_claims, objects) {
        Ok(residual) => residual,
        Err(error) => return PostVerdict::Unsupported(error),
    };
    for claim in post_claims {
        if let Err(error) = residual.subtract(claim.base, claim.size) {
            return PostVerdict::Unsupported(error);
        }
    }
    if residual.intervals.is_empty() {
        PostVerdict::Passed
    } else {
        PostVerdict::Failed(PostFailure::LeakedOwnership {
            intervals: residual.intervals,
        })
    }
}

fn pre_error(error: WalkError) -> PreVerdict {
    match error {
        WalkError::Failure(failure) => PreVerdict::Failed(match failure {
            CommonFailure::ConstraintFalse { step } => PreFailure::ConstraintFalse { step },
            CommonFailure::NullTake { step } => PreFailure::NullTake { step },
            CommonFailure::UnauthorizedTake { step } => PreFailure::UnauthorizedTake { step },
            CommonFailure::OverlappingTake { step } => PreFailure::OverlappingTake { step },
            CommonFailure::FreedTake { step } => {
                return PreVerdict::InternalError(InternalError::new(format!(
                    "pre-state unexpectedly observed a freed allocation at step {step}"
                )));
            }
            CommonFailure::OutOfBoundsTake { step } => {
                return PreVerdict::InternalError(InternalError::new(format!(
                    "pre-state unexpectedly observed target allocation bounds at step {step}"
                )));
            }
            CommonFailure::ResourceValueMismatch { step, symbol } => {
                PreFailure::ResourceValueMismatch { step, symbol }
            }
        }),
        WalkError::Unsupported(error) => PreVerdict::Unsupported(error),
        WalkError::Internal(error) => PreVerdict::InternalError(error),
    }
}

fn post_error(error: WalkError) -> PostVerdict {
    match error {
        WalkError::Failure(failure) => PostVerdict::Failed(match failure {
            CommonFailure::ConstraintFalse { step } => PostFailure::ConstraintFalse { step },
            CommonFailure::NullTake { step } => PostFailure::NullTake { step },
            CommonFailure::UnauthorizedTake { step } => PostFailure::UnauthorizedTake { step },
            CommonFailure::OverlappingTake { step } => PostFailure::OverlappingTake { step },
            CommonFailure::FreedTake { step } => PostFailure::FreedTake { step },
            CommonFailure::OutOfBoundsTake { step } => PostFailure::OutOfBoundsTake { step },
            CommonFailure::ResourceValueMismatch { step, .. } => {
                return PostVerdict::Unsupported(Unsupported::new(format!(
                    "post-state resource binding unexpectedly reused at step {step}"
                )));
            }
        }),
        WalkError::Unsupported(error) => PostVerdict::Unsupported(error),
        WalkError::Internal(error) => PostVerdict::Unsupported(Unsupported::new(error.reason)),
    }
}

fn validate_regions(regions: &[MemoryRegion]) -> Result<(), InternalError> {
    for (index, region) in regions.iter().enumerate() {
        if region.base == 0 && region.size != 0 {
            return Err(InternalError::new(format!(
                "authorized region {index} has a null base"
            )));
        }
        if region.end().is_none() {
            return Err(InternalError::new(format!(
                "authorized region {index} wraps the address space"
            )));
        }
        for (other, prior) in regions[..index].iter().enumerate() {
            if overlaps(region.base, region.size, prior.base, prior.size) {
                return Err(InternalError::new(format!(
                    "authorized regions {other} and {index} overlap"
                )));
            }
        }
    }
    Ok(())
}

fn validate_writes(
    writes: &[CandidateWrite],
    regions: &[MemoryRegion],
) -> Result<bool, PreFailure> {
    let mut end = 0;
    let mut ordered = true;
    for (index, write) in writes.iter().enumerate() {
        if write.addr == 0 {
            return Err(PreFailure::NullWrite { write: index });
        }
        let size = u64::try_from(write.bytes.len())
            .map_err(|_| PreFailure::UnauthorizedWrite { write: index })?;
        if !regions
            .iter()
            .any(|region| region.writable && region.covers(write.addr, size))
        {
            return Err(PreFailure::UnauthorizedWrite { write: index });
        }
        if write.addr < end {
            ordered = false;
            if size != 0 {
                for (prior, other) in writes[..index].iter().enumerate() {
                    if overlaps(
                        write.addr,
                        size,
                        other.addr,
                        u64::try_from(other.bytes.len()).unwrap_or(u64::MAX),
                    ) {
                        return Err(PreFailure::OverlappingWrites {
                            first: prior,
                            second: index,
                        });
                    }
                }
            }
        }
        end = end.max(write.addr + size);
    }
    Ok(ordered)
}

fn overlaps(a: u64, an: u64, b: u64, bn: u64) -> bool {
    an != 0
        && bn != 0
        && a.checked_add(an)
            .zip(b.checked_add(bn))
            .is_some_and(|(ae, be)| a < be && b < ae)
}

fn materialized_allocations(
    roots: &[u64],
    claims: &[AllocationClaim],
    regions: &[MemoryRegion],
) -> Vec<MaterializedAllocation> {
    let mut claims = claims.to_vec();
    claims.sort_unstable_by(|a, b| {
        (a.heap_region, a.extent.base, a.extent.size, &a.roots).cmp(&(
            b.heap_region,
            b.extent.base,
            b.extent.size,
            &b.roots,
        ))
    });
    claims.dedup();

    let mut roots = roots
        .iter()
        .copied()
        .filter_map(|base| {
            claims
                .iter()
                .find(|claim| claim.extent.base == base)
                .map(|claim| AllocationRoot {
                    base,
                    heap_region: claim.heap_region,
                })
        })
        .collect::<Vec<_>>();
    roots.sort_unstable_by_key(|root| (root.heap_region, root.base));
    roots.dedup();

    let mut allocations = Vec::with_capacity(roots.len());
    for (index, root) in roots.iter().enumerate() {
        let Some(region_end) = regions.get(root.heap_region).and_then(MemoryRegion::end) else {
            continue;
        };
        let boundary = roots[index + 1..]
            .iter()
            .find(|next| next.heap_region == root.heap_region)
            .map_or(region_end, |next| next.base);
        let related = claims
            .iter()
            .filter(|claim| {
                claim.heap_region == root.heap_region
                    && (claim.extent.base == root.base
                        || claim.roots.binary_search(&root.base).is_ok())
            })
            .collect::<Vec<_>>();

        if related.iter().any(|claim| {
            claim.extent.base < root.base
                || claim.extent.base >= boundary
                || claim
                    .extent
                    .base
                    .checked_add(claim.extent.size)
                    .is_none_or(|end| end > boundary || end > region_end)
        }) {
            continue;
        }

        let mut end = root.base;
        let adjacent = related.iter().all(|claim| {
            let contiguous = claim.extent.base == end;
            end = claim.extent.base + claim.extent.size;
            contiguous
        });

        if adjacent {
            allocations.push(MaterializedAllocation {
                base: root.base,
                size: end - root.base,
            });
        }
    }
    allocations
}

type RootSet = Vec<u64>;

fn merge_roots(into: &mut RootSet, roots: impl IntoIterator<Item = u64>) {
    into.extend(roots);
    into.sort_unstable();
    into.dedup();
}

fn bind_globals(env: &mut SymMap<Value>, regions: &[MemoryRegion]) {
    for region in regions {
        if let RegionKind::Global(symbol) = &region.kind {
            env.insert(
                symbol.clone(),
                Value::Scalar(Scalar::global_pointer(region.base, symbol.num)),
            );
        }
    }
}

fn missing_argument(internal: &ArgType<()>, env: &SymMap<Value>) -> Option<Sym> {
    let mut node = internal;
    loop {
        node = match node {
            ArgType::Computational((symbol, _), rest) | ArgType::Ghost((symbol, _), rest) => {
                if !env.contains_key(symbol) {
                    return Some(symbol.clone());
                }
                rest
            }
            ArgType::L(_) => return None,
        };
    }
}

struct Overlay<'a> {
    base: &'a dyn MemoryView,
    writes: &'a [CandidateWrite],
    ordered: bool,
}

impl MemoryView for Overlay<'_> {
    fn read_into(&self, addr: u64, size: u64, buf: &mut Vec<u8>) -> Result<(), MemoryAccessError> {
        self.base.read_into(addr, size, buf)?;
        let end = addr
            .checked_add(size)
            .ok_or_else(|| MemoryAccessError::new("virtual read wraps the address space"))?;
        let writes = if self.ordered && self.writes.len() > 16 {
            let first = self
                .writes
                .partition_point(|write| write.addr + write.bytes.len() as u64 <= addr);
            let rest = &self.writes[first..];
            &rest[..rest.partition_point(|write| write.addr < end)]
        } else {
            self.writes
        };
        for write in writes {
            let write_end = write
                .addr
                .checked_add(write.bytes.len() as u64)
                .ok_or_else(|| MemoryAccessError::new("candidate write wraps address space"))?;
            let start = addr.max(write.addr);
            let overlap_end = end.min(write_end);
            if start < overlap_end {
                let dst = usize::try_from(start - addr)
                    .map_err(|_| MemoryAccessError::new("virtual read offset is too large"))?;
                let src = usize::try_from(start - write.addr)
                    .map_err(|_| MemoryAccessError::new("candidate write offset is too large"))?;
                let len = usize::try_from(overlap_end - start)
                    .map_err(|_| MemoryAccessError::new("virtual overlay is too large"))?;
                buf.get_mut(dst..dst + len)
                    .ok_or_else(|| MemoryAccessError::new("virtual read buffer is too short"))?
                    .copy_from_slice(
                        write.bytes.get(src..src + len).ok_or_else(|| {
                            MemoryAccessError::new("candidate write is too short")
                        })?,
                    );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
enum Phase<'a> {
    Pre {
        allocation_roots: &'a [u64],
    },
    Post {
        objects: &'a [TrackedObject],
        require_owned: bool,
    },
}

impl<'a> Phase<'a> {
    fn is_pre(self) -> bool {
        matches!(self, Phase::Pre { .. })
    }

    fn allocation_roots(self) -> &'a [u64] {
        match self {
            Phase::Pre { allocation_roots } => allocation_roots,
            Phase::Post { .. } => &[],
        }
    }

    fn objects(self) -> &'a [TrackedObject] {
        match self {
            Phase::Pre { .. } => &[],
            Phase::Post { objects, .. } => objects,
        }
    }

    fn require_owned(self) -> bool {
        matches!(
            self,
            Phase::Post {
                require_owned: true,
                ..
            }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Located {
    Raw { region: Option<usize> },
    Held,
    Probe,
    Foreign,
}

fn restore<V>(map: &mut SymMap<V>, key: &Sym, previous: Option<V>) {
    match previous {
        Some(value) => {
            map.insert(key.clone(), value);
        }
        None => {
            map.remove(key);
        }
    }
}

#[derive(Debug)]
enum CommonFailure {
    ConstraintFalse { step: usize },
    NullTake { step: usize },
    UnauthorizedTake { step: usize },
    OverlappingTake { step: usize },
    FreedTake { step: usize },
    OutOfBoundsTake { step: usize },
    ResourceValueMismatch { step: usize, symbol: Sym },
}

#[derive(Debug)]
enum WalkError {
    Failure(CommonFailure),
    Unsupported(Unsupported),
    Internal(InternalError),
}

impl From<MemoryAccessError> for WalkError {
    fn from(value: MemoryAccessError) -> Self {
        WalkError::Internal(InternalError::new(value.reason))
    }
}

#[derive(Default)]
struct ResidualOwnership {
    intervals: Vec<OwnershipExtent>,
}

impl ResidualOwnership {
    fn new(
        heap_claims: &[OwnershipExtent],
        objects: &[TrackedObject],
    ) -> Result<Self, Unsupported> {
        let mut residual = Self::default();
        for claim in heap_claims {
            residual.push(claim.base, claim.size)?;
        }
        residual.normalize();
        for object in objects {
            if object.state == ObjectState::Released {
                residual.subtract(object.base, object.size)?;
            }
        }
        for object in objects {
            if object.state == ObjectState::Live(Access::Held) {
                residual.push(object.base, object.size)?;
            }
        }
        residual.normalize();
        Ok(residual)
    }

    fn end(base: u64, size: u64) -> Result<u64, Unsupported> {
        base.checked_add(size)
            .ok_or_else(|| Unsupported::new("heap ownership extent wraps the address space"))
    }

    fn push(&mut self, base: u64, size: u64) -> Result<(), Unsupported> {
        if size == 0 {
            return Ok(());
        }
        Self::end(base, size)?;
        self.intervals.push(OwnershipExtent { base, size });
        Ok(())
    }

    fn normalize(&mut self) {
        self.intervals.sort_unstable_by_key(|extent| extent.base);
        let mut merged: Vec<OwnershipExtent> = Vec::with_capacity(self.intervals.len());
        for extent in self.intervals.drain(..) {
            if let Some(last) = merged.last_mut() {
                let last_end = last.base + last.size;
                if extent.base <= last_end {
                    last.size = (extent.base + extent.size).max(last_end) - last.base;
                    continue;
                }
            }
            merged.push(extent);
        }
        self.intervals = merged;
    }

    fn subtract(&mut self, base: u64, size: u64) -> Result<(), Unsupported> {
        if size == 0 {
            return Ok(());
        }
        let end = Self::end(base, size)?;
        let first = self
            .intervals
            .partition_point(|interval| interval.base + interval.size <= base);
        let after = first + self.intervals[first..].partition_point(|interval| interval.base < end);
        if first == after {
            return Ok(());
        }
        let head = self.intervals[first];
        let tail = self.intervals[after - 1];
        let tail_end = tail.base + tail.size;
        let kept = [
            (base > head.base).then(|| OwnershipExtent {
                base: head.base,
                size: base - head.base,
            }),
            (end < tail_end).then(|| OwnershipExtent {
                base: end,
                size: tail_end - end,
            }),
        ];
        self.intervals
            .splice(first..after, kept.into_iter().flatten());
        Ok(())
    }
}

fn contains_closed(region: &MemoryRegion, addr: u64) -> bool {
    addr.checked_sub(region.base)
        .is_some_and(|offset| offset <= region.size)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AllocationClaim {
    extent: OwnershipExtent,
    heap_region: usize,
    roots: RootSet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AllocationRoot {
    base: u64,
    heap_region: usize,
}

struct BoundValue {
    value: Value,
    provenance: ValueProvenance,
}

struct RootedPointer {
    addr: u64,
    origin: PointerOrigin,
    roots: RootSet,
}

#[derive(Clone, Copy)]
struct EvalContext<'a> {
    env: &'a SymMap<Value>,
    provenance: &'a SymMap<ValueProvenance>,
}

struct Reader<'a> {
    defaults: &'a austen_lib::spec::DefaultChoices,
    module: &'a SpecModule,
    regions: &'a [MemoryRegion],
    memory: &'a dyn MemoryView,
    phase: Phase<'a>,
    taken: Vec<OwnershipExtent>,
    taken_end: u64,
    claims: Vec<OwnershipExtent>,
    allocation_claims: Vec<AllocationClaim>,
    global_authorities: Vec<GlobalPointerAuthority>,
    scratch: Vec<u8>,
    step: usize,
}

impl Reader<'_> {
    fn origin(&self, addr: u64) -> PointerOrigin {
        pointer_origin(addr, self.regions)
    }

    fn eval(&self, term: &IndexTerm, env: &SymMap<Value>) -> Option<Value> {
        eval(
            term,
            env,
            self.module,
            &|addr| self.origin(addr),
            self.defaults,
        )
    }

    fn check_constraint(
        &self,
        constraint: &austen_lib::spec::LogicalConstraint,
        env: &SymMap<Value>,
    ) -> Option<bool> {
        check_constraint(
            constraint,
            env,
            self.module,
            &|addr| self.origin(addr),
            self.defaults,
        )
    }

    fn finite_domain(
        &self,
        bounds: &austen_lib::spec::SymbolicMapBounds,
        env: &SymMap<Value>,
    ) -> Option<std::ops::Range<i128>> {
        finite_domain(
            bounds,
            env,
            self.module,
            &|addr| self.origin(addr),
            self.defaults,
        )
    }

    fn decode(&self, bytes: &[u8], ct: &Sctype, expected: &BaseType) -> Option<Value> {
        decode(bytes, ct, expected, self.module, &|addr| self.origin(addr))
    }
}

impl<'a> Reader<'a> {
    fn new(
        module: &'a SpecModule,
        regions: &'a [MemoryRegion],
        memory: &'a dyn MemoryView,
        phase: Phase<'a>,
    ) -> Self {
        Self {
            defaults: &EMPTY_DEFAULTS,
            module,
            regions,
            memory,
            phase,
            taken: Vec::new(),
            taken_end: 0,
            claims: Vec::new(),
            allocation_claims: Vec::new(),
            global_authorities: Vec::new(),
            scratch: Vec::new(),
            step: 0,
        }
    }

    fn walk<I>(
        &mut self,
        chain: &Lat<I>,
        env: &mut SymMap<Value>,
        provenance: &mut SymMap<ValueProvenance>,
        depth: usize,
    ) -> Result<(), WalkError> {
        let mut node = chain;
        loop {
            node = match node {
                Lat::Define((symbol, term), rest) => {
                    match self.eval(term, env) {
                        Some(value) => {
                            let origin = self.term_provenance(term, env, provenance);
                            env.insert(symbol.clone(), value);
                            provenance.insert(symbol.clone(), origin);
                        }
                        None if term.bt == BaseType::Unit => {}
                        None => return self.unsupported("logical define is not evaluable"),
                    }
                    rest
                }
                Lat::Resource((symbol, (request, bt)), rest) => {
                    let prior = self
                        .phase
                        .is_pre()
                        .then(|| env.get(symbol).cloned())
                        .flatten();
                    if let Some(bound) = self.request(request, bt, env, provenance, depth)? {
                        if prior.as_ref().is_some_and(|prior| prior != &bound.value) {
                            return Err(WalkError::Failure(CommonFailure::ResourceValueMismatch {
                                step: self.step,
                                symbol: symbol.clone(),
                            }));
                        }
                        env.insert(symbol.clone(), bound.value);
                        provenance.insert(symbol.clone(), bound.provenance);
                    } else if !self.phase.is_pre()
                        && matches!(request.name(), RequestName::Owned(_, Init::Uninit))
                    {
                        env.remove(symbol);
                        provenance.remove(symbol);
                    }
                    rest
                }
                Lat::Constraint(constraint, rest) => match self.check_constraint(constraint, env) {
                    Some(true) => rest,
                    Some(false) => {
                        return Err(WalkError::Failure(CommonFailure::ConstraintFalse {
                            step: self.step,
                        }));
                    }
                    None => return self.unsupported("logical constraint is not evaluable"),
                },
                Lat::I(_) => return Ok(()),
            };
            if depth == 0 {
                self.step += 1;
            }
        }
    }

    fn request(
        &mut self,
        request: &Request,
        bt: &BaseType,
        env: &mut SymMap<Value>,
        provenance: &mut SymMap<ValueProvenance>,
        depth: usize,
    ) -> Result<Option<BoundValue>, WalkError> {
        match request {
            Request::P {
                name,
                pointer,
                iargs,
            } => {
                let mut roots = self.term_roots(pointer, env, provenance);
                let (addr, origin) = self.pointer(pointer, env)?;
                self.add_exact_pointer_root(&mut roots, (addr, origin));
                self.single(
                    name,
                    RootedPointer {
                        addr,
                        origin,
                        roots,
                    },
                    iargs,
                    bt,
                    EvalContext { env, provenance },
                    depth,
                )
            }
            Request::Q {
                name,
                pointer,
                q,
                step,
                permission,
                iargs,
            } => {
                let (key, value) = check_quantified_request(bt, &q.1).map_err(|defect| {
                    WalkError::Unsupported(Unsupported::new(defect.to_string()))
                })?;
                let bounds = check_quantified_permission(permission, &q.0).map_err(|defect| {
                    WalkError::Unsupported(Unsupported::new(defect.to_string()))
                })?;
                let roots = self.term_roots(pointer, env, provenance);
                let (base, origin) = self.pointer(pointer, env)?;
                let stride = size_of(self.module, step).ok_or_else(|| {
                    WalkError::Unsupported(Unsupported::new(
                        "quantified resource has an unsized stride",
                    ))
                })?;
                let domain = self.finite_domain(&bounds, env).ok_or_else(|| {
                    WalkError::Unsupported(Unsupported::new(
                        "quantified resource permission is not a finite range",
                    ))
                })?;
                if domain
                    .end
                    .checked_sub(domain.start)
                    .is_none_or(|n| n > EXTENT_PREFLIGHT_THRESHOLD)
                {
                    if !self.phase.is_pre()
                        && bounds.index_bt == *key
                        && bounds.lower.bt == *key
                        && bounds.upper.bt == *key
                        && let RequestName::Owned(ct, init) = name
                        && let Some(range) = quantified::Range::new(base, stride, key, &domain)
                        && self.range_permitted(permission, q, &domain, env)
                        && self.take_quantified(ct, *init, value, origin, range)?
                    {
                        return Ok(None);
                    }
                }
                let previous = env.remove(&q.0);
                let previous_provenance = provenance.remove(&q.0);
                let mut entries = Vec::new();
                let mut result_provenance = Vec::new();
                let mut missing_held_uninit = false;
                for index in domain {
                    env.insert(q.0.clone(), index_value(key, index)?);
                    let permitted =
                        self.eval(permission, env)
                            .and_then(boolean)
                            .ok_or_else(|| {
                                WalkError::Unsupported(Unsupported::new(
                                    "quantified resource permission is not evaluable",
                                ))
                            })?;
                    if !permitted {
                        continue;
                    }
                    let addr = base.wrapping_add(stride.wrapping_mul(index as u64));
                    let mut element_roots = roots.clone();
                    self.add_exact_pointer_root(&mut element_roots, (addr, origin));
                    if let Some(result) = self.single(
                        name,
                        RootedPointer {
                            addr,
                            origin,
                            roots: element_roots,
                        },
                        iargs,
                        value,
                        EvalContext { env, provenance },
                        depth,
                    )? {
                        result_provenance.push((index, result.provenance));
                        entries.push((index, result.value));
                    } else if matches!(name, RequestName::Owned(_, Init::Uninit)) {
                        missing_held_uninit = true;
                    }
                }
                restore(env, &q.0, previous);
                restore(provenance, &q.0, previous_provenance);
                if missing_held_uninit {
                    return Ok(None);
                }
                let value = Value::Map {
                    entries,
                    default: None,
                };
                Ok(Some(BoundValue {
                    value,
                    provenance: ValueProvenance::Map {
                        entries: result_provenance,
                        default: None,
                    },
                }))
            }
        }
    }

    fn single(
        &mut self,
        name: &RequestName,
        pointer: RootedPointer,
        iargs: &[austen_lib::spec::IndexTerm],
        bt: &BaseType,
        caller: EvalContext<'_>,
        depth: usize,
    ) -> Result<Option<BoundValue>, WalkError> {
        match name {
            RequestName::Owned(ct, init) => self.take(&pointer, ct, *init, bt).map(|value| {
                value.map(|value| {
                    let provenance =
                        ValueProvenance::from_value(&value, self.phase.allocation_roots());
                    BoundValue { value, provenance }
                })
            }),
            RequestName::PName(name) => self.predicate(name, pointer, iargs, bt, caller, depth),
        }
    }

    fn predicate(
        &mut self,
        name: &Sym,
        pointer: RootedPointer,
        iargs: &[austen_lib::spec::IndexTerm],
        expected: &BaseType,
        caller: EvalContext<'_>,
        depth: usize,
    ) -> Result<Option<BoundValue>, WalkError> {
        if depth >= MAX_PREDICATE_DEPTH {
            return self.unsupported("predicate calls exceed the oracle depth limit");
        }
        let predicate = self.module.predicate(name).ok_or_else(|| {
            WalkError::Unsupported(Unsupported::new("resource names an undeclared predicate"))
        })?;
        let clauses = predicate_support(predicate, iargs, expected)?;
        let mut env = SymMap::new();
        let mut provenance = SymMap::new();
        bind_globals(&mut env, self.regions);
        env.insert(
            predicate.pointer.clone(),
            Value::Scalar(Scalar::Pointer {
                addr: pointer.addr,
                origin: pointer.origin,
            }),
        );
        provenance.insert(
            predicate.pointer.clone(),
            ValueProvenance::Roots(pointer.roots),
        );
        for ((formal, _), actual) in predicate.iargs.iter().zip(iargs) {
            let value = self.eval(actual, caller.env).ok_or_else(|| {
                WalkError::Unsupported(Unsupported::new("predicate argument is not evaluable"))
            })?;
            let origin = self.term_provenance(actual, caller.env, caller.provenance);
            env.insert(formal.clone(), value);
            provenance.insert(formal.clone(), origin);
        }
        for clause in clauses {
            let guard = self
                .eval(&clause.guard, &env)
                .and_then(boolean)
                .ok_or_else(|| {
                    WalkError::Unsupported(Unsupported::new("predicate guard is not evaluable"))
                })?;
            if !guard {
                continue;
            }
            self.walk(&clause.packing_ft, &mut env, &mut provenance, depth + 1)?;
            let Lat::I(result) = terminal(&clause.packing_ft) else {
                unreachable!()
            };
            return if result.bt == BaseType::Unit {
                Ok(None)
            } else {
                let value = self.eval(result, &env).ok_or_else(|| {
                    WalkError::Unsupported(Unsupported::new("predicate result is not evaluable"))
                })?;
                let provenance = self.term_provenance(result, &env, &provenance);
                Ok(Some(BoundValue { value, provenance }))
            };
        }
        self.unsupported("predicate has no matching clause")
    }

    fn take(
        &mut self,
        pointer: &RootedPointer,
        ct: &Sctype,
        init: Init,
        expected: &BaseType,
    ) -> Result<Option<Value>, WalkError> {
        check_owned_request(ct, expected)
            .map_err(|defect| WalkError::Unsupported(Unsupported::new(defect.to_string())))?;
        let size = size_of(self.module, ct).ok_or_else(|| {
            WalkError::Unsupported(Unsupported::new("Owned resource has an unsized C type"))
        })?;
        let extent = OwnershipExtent {
            base: pointer.addr,
            size,
        };
        if pointer.addr == 0 {
            return Err(WalkError::Failure(CommonFailure::NullTake {
                step: self.step,
            }));
        }
        if size == 0 {
            return self.decode(&[], ct, expected).map(Some).ok_or_else(|| {
                WalkError::Unsupported(Unsupported::new(
                    "zero-sized Owned resource has no canonical value",
                ))
            });
        }
        let effective_origin = self
            .effective_origin(pointer, extent)
            .map_err(WalkError::Failure)?;
        let located = self.check_extent(extent, effective_origin)?;
        let direct = matches!(located, Located::Raw { .. } | Located::Held);
        self.record_extent(extent);
        if self.phase.is_pre()
            && effective_origin == PointerOrigin::Heap
            && let Located::Raw {
                region: Some(heap_region),
            } = located
            && matches!(
                self.regions.get(heap_region).map(|region| &region.kind),
                Some(RegionKind::Heap)
            )
        {
            self.allocation_claims.push(AllocationClaim {
                extent,
                heap_region,
                roots: pointer.roots.clone(),
            });
        }
        if init == Init::Uninit && located == Located::Held {
            return Ok(None);
        }
        if located == Located::Held {
            return self
                .read_held_initialized(extent.base, ct, expected)
                .map(Some);
        }
        if direct {
            self.memory
                .read_into(extent.base, extent.size, &mut self.scratch)?;
        } else {
            self.memory
                .read_foreign(extent.base, extent.size, &mut self.scratch)?;
        }
        if !self.phase.is_pre() {
            if direct {
                self.memory.write_back(extent.base, &self.scratch)?;
            } else {
                self.memory.write_back_foreign(extent.base, &self.scratch)?;
            }
        }
        self.decode(&self.scratch, ct, expected)
            .map(Some)
            .ok_or_else(|| {
                WalkError::Unsupported(Unsupported::new(
                    "Owned resource bytes do not decode at the binding type",
                ))
            })
    }

    fn check_extent(
        &self,
        extent: OwnershipExtent,
        origin: PointerOrigin,
    ) -> Result<Located, WalkError> {
        let located = self.locate(extent, origin).map_err(WalkError::Failure)?;
        if !matches!(located, Located::Raw { .. } | Located::Held)
            && (self.phase.is_pre() || self.phase.require_owned())
        {
            return Err(WalkError::Failure(CommonFailure::UnauthorizedTake {
                step: self.step,
            }));
        }
        if extent.base < self.taken_end
            && self
                .taken
                .iter()
                .any(|other| overlaps(extent.base, extent.size, other.base, other.size))
        {
            return Err(WalkError::Failure(CommonFailure::OverlappingTake {
                step: self.step,
            }));
        }
        Ok(located)
    }

    fn record_extent(&mut self, extent: OwnershipExtent) {
        self.claims.push(extent);
        self.taken.push(extent);
        self.taken_end = self.taken_end.max(extent.base.saturating_add(extent.size));
    }

    fn read_held_initialized(
        &mut self,
        addr: u64,
        ct: &Sctype,
        expected: &BaseType,
    ) -> Result<Value, WalkError> {
        match (ct, expected) {
            (Sctype::Struct(tag), BaseType::Struct(expected_tag)) if tag == expected_tag => {
                let layout = struct_layout(self.module, tag).ok_or_else(|| {
                    WalkError::Unsupported(Unsupported::new("Owned resource has an unsized C type"))
                })?;
                let mut fields = Vec::with_capacity(layout.members.len());
                for member in layout.members {
                    let member_addr = addr.checked_add(member.offset).ok_or_else(|| {
                        WalkError::Unsupported(Unsupported::new(
                            "Owned resource member address wraps",
                        ))
                    })?;
                    let member_bt = member.ct.to_base_type().ok_or_else(|| {
                        WalkError::Unsupported(Unsupported::new(
                            "Owned resource member has no logical C value type",
                        ))
                    })?;
                    let value = self.read_held_initialized(member_addr, &member.ct, &member_bt)?;
                    fields.push((member.id, value));
                }
                Ok(Value::Struct {
                    tag: tag.clone(),
                    fields,
                })
            }
            (Sctype::Array(element, Some(length)), BaseType::Map(key, value))
                if **key == BaseType::Bits(austen_lib::spec::Sign::Unsigned, 64) =>
            {
                let stride = size_of(self.module, element).ok_or_else(|| {
                    WalkError::Unsupported(Unsupported::new("Owned resource has an unsized C type"))
                })?;
                let mut entries = Vec::with_capacity(*length);
                for index in 0..*length {
                    let offset = stride
                        .checked_mul(u64::try_from(index).map_err(|_| {
                            WalkError::Unsupported(Unsupported::new(
                                "Owned array index does not fit u64",
                            ))
                        })?)
                        .and_then(|offset| addr.checked_add(offset))
                        .ok_or_else(|| {
                            WalkError::Unsupported(Unsupported::new(
                                "Owned resource element address wraps",
                            ))
                        })?;
                    entries.push((
                        index as i128,
                        self.read_held_initialized(offset, element, value)?,
                    ));
                }
                Ok(Value::Map {
                    entries,
                    default: None,
                })
            }
            (Sctype::Struct(_), _) | (Sctype::Array(_, _), _) => {
                self.unsupported("Owned resource bytes do not decode at the binding type")
            }
            _ => {
                let size = size_of(self.module, ct).ok_or_else(|| {
                    WalkError::Unsupported(Unsupported::new("Owned resource has an unsized C type"))
                })?;
                self.memory.read_into(addr, size, &mut self.scratch)?;
                if size != 0 {
                    self.memory.write_back(addr, &self.scratch)?;
                }
                self.decode(&self.scratch, ct, expected).ok_or_else(|| {
                    WalkError::Unsupported(Unsupported::new(
                        "Owned resource bytes do not decode at the binding type",
                    ))
                })
            }
        }
    }

    fn pointer(
        &self,
        term: &austen_lib::spec::IndexTerm,
        env: &SymMap<Value>,
    ) -> Result<(u64, PointerOrigin), WalkError> {
        match self.eval(term, env) {
            Some(Value::Scalar(Scalar::Pointer { addr, origin, .. })) => Ok((addr, origin)),
            _ => self.unsupported("resource pointer is not evaluable"),
        }
    }

    fn term_roots(
        &self,
        term: &IndexTerm,
        env: &SymMap<Value>,
        provenance: &SymMap<ValueProvenance>,
    ) -> RootSet {
        self.term_provenance(term, env, provenance).roots()
    }

    fn term_provenance(
        &self,
        term: &IndexTerm,
        env: &SymMap<Value>,
        provenance: &SymMap<ValueProvenance>,
    ) -> ValueProvenance {
        provenance::term_provenance(
            term,
            env,
            provenance,
            self.module,
            self.regions,
            self.phase.allocation_roots(),
            self.defaults,
        )
    }

    fn add_exact_pointer_root(&self, roots: &mut RootSet, pointer: (u64, PointerOrigin)) {
        if pointer.1 == PointerOrigin::Heap && self.phase.allocation_roots().contains(&pointer.0) {
            merge_roots(roots, [pointer.0]);
        }
    }

    fn tracked(&self) -> impl Iterator<Item = (Option<usize>, TrackedObject)> + '_ {
        self.regions
            .iter()
            .enumerate()
            .map(|(index, region)| (Some(index), region.tracked()))
            .chain(self.phase.objects().iter().map(|object| (None, *object)))
    }

    fn effective_origin(
        &mut self,
        pointer: &RootedPointer,
        extent: OwnershipExtent,
    ) -> Result<PointerOrigin, CommonFailure> {
        if !self.phase.is_pre()
            || extent.size == 0
            || pointer.origin != PointerOrigin::Heap
            || pointer.roots.is_empty()
        {
            return Ok(pointer.origin);
        }

        let mut learned = None;
        for root in &pointer.roots {
            let Some(global) = self
                .global_authorities
                .iter()
                .find(|authority| authority.root == *root)
                .map(|authority| authority.global)
            else {
                continue;
            };
            if learned.replace(global).is_some_and(|prior| prior != global) {
                return Err(CommonFailure::UnauthorizedTake { step: self.step });
            }
        }

        if let Some(global) = learned {
            self.guard_no_heap_claim(&pointer.roots)?;
            let Some(region) = self.writable_global_containing_roots(&pointer.roots).find(
                |region| matches!(&region.kind, RegionKind::Global(symbol) if symbol.num == global),
            ) else {
                return Err(CommonFailure::UnauthorizedTake { step: self.step });
            };
            if region.covers(extent.base, extent.size) {
                self.learn_global_authorities(&pointer.roots, global);
            }
            return Ok(PointerOrigin::Global(global));
        }

        let mut candidate = None;
        for region in self
            .writable_global_containing_roots(&pointer.roots)
            .filter(|region| region.covers(extent.base, extent.size))
        {
            let RegionKind::Global(symbol) = &region.kind else {
                unreachable!()
            };
            if candidate.replace(symbol.num).is_some() {
                return Err(CommonFailure::UnauthorizedTake { step: self.step });
            }
        }
        if let Some(global) = candidate {
            self.guard_no_heap_claim(&pointer.roots)?;
            self.learn_global_authorities(&pointer.roots, global);
            Ok(PointerOrigin::Global(global))
        } else {
            Ok(pointer.origin)
        }
    }

    fn writable_global_containing_roots<'r>(
        &'r self,
        roots: &'r [u64],
    ) -> impl Iterator<Item = &'r MemoryRegion> {
        self.regions.iter().filter(move |region| {
            region.writable
                && matches!(region.kind, RegionKind::Global(_))
                && roots.iter().all(|root| contains_closed(region, *root))
        })
    }

    fn roots_have_heap_claim(&self, roots: &[u64]) -> bool {
        self.allocation_claims
            .iter()
            .any(|claim| claim.roots.iter().any(|root| roots.contains(root)))
    }

    fn guard_no_heap_claim(&self, roots: &[u64]) -> Result<(), CommonFailure> {
        if self.roots_have_heap_claim(roots) {
            return Err(CommonFailure::UnauthorizedTake { step: self.step });
        }
        Ok(())
    }

    fn learn_global_authorities(&mut self, roots: &[u64], global: u64) {
        for root in roots {
            if !self
                .global_authorities
                .iter()
                .any(|authority| authority.root == *root)
            {
                self.global_authorities.push(GlobalPointerAuthority {
                    root: *root,
                    global,
                });
            }
        }
    }

    fn locate(
        &self,
        extent: OwnershipExtent,
        origin: PointerOrigin,
    ) -> Result<Located, CommonFailure> {
        let post = !self.phase.is_pre();
        let mut raw = None;
        let mut held = false;
        let mut probed = false;
        let mut out_of_bounds = false;
        let mut origin_out_of_bounds = false;
        let mut released = false;
        for (region, object) in self.tracked() {
            match object.state {
                ObjectState::Live(access) => {
                    let covers = object.covers(extent.base, extent.size);
                    let overlaps = || object.overlaps(extent.base, extent.size);
                    if extent.size != 0 && !covers {
                        match access.bounds() {
                            Bounds::AccessOnly => {}
                            Bounds::Extent => out_of_bounds |= overlaps(),
                            Bounds::Origin(id) => {
                                origin_out_of_bounds |=
                                    post && (origin == PointerOrigin::Global(id) || overlaps());
                            }
                        }
                    }
                    match access {
                        Access::Raw(authority)
                            if raw.is_none() && authority == origin && covers =>
                        {
                            raw = Some(region);
                        }
                        Access::Held if covers => held = true,
                        Access::Probe if extent.size != 0 && covers => probed = true,
                        _ => {}
                    }
                }
                ObjectState::Released => {
                    released |= object.overlaps(extent.base, extent.size);
                }
            }
        }
        let step = self.step;
        if origin_out_of_bounds {
            Err(CommonFailure::OutOfBoundsTake { step })
        } else if held {
            Ok(Located::Held)
        } else if probed {
            Ok(Located::Probe)
        } else if out_of_bounds {
            Err(CommonFailure::OutOfBoundsTake { step })
        } else if released {
            Err(CommonFailure::FreedTake { step })
        } else {
            Ok(raw.map_or(Located::Foreign, |region| Located::Raw { region }))
        }
    }

    fn return_value(&mut self, ensures: &Ensures, carrier: i128) -> Result<Value, WalkError> {
        let bt = &ensures.var.1;
        match bt {
            BaseType::Unit => Ok(Value::Unit),
            BaseType::Bool => Ok(Value::Scalar(Scalar::Bool(carrier != 0))),
            BaseType::Integer => Ok(Value::Scalar(Scalar::Int(carrier.into()))),
            BaseType::Bits(sign, width) => bits(*sign, *width, carrier)
                .map(Value::Scalar)
                .ok_or_else(|| {
                    WalkError::Unsupported(Unsupported::new(
                        "return carrier does not fit the return binding",
                    ))
                }),
            BaseType::Loc(_) => Ok(Value::Scalar(pointer_scalar(carrier as u64, &|addr| {
                self.origin(addr)
            }))),
            BaseType::Struct(tag) => {
                let layout = struct_layout(self.module, tag).ok_or_else(|| {
                    WalkError::Unsupported(Unsupported::new(
                        "returned struct has no declared layout",
                    ))
                })?;
                let addr = u64::try_from(carrier).map_err(|_| {
                    WalkError::Unsupported(Unsupported::new(
                        "returned struct carrier is not an address",
                    ))
                })?;
                self.memory
                    .read_into(addr, layout.size, &mut self.scratch)?;
                self.decode(&self.scratch, &Sctype::Struct(tag.clone()), bt)
                    .ok_or_else(|| {
                        WalkError::Unsupported(Unsupported::new(
                            "returned struct does not decode at the return binding",
                        ))
                    })
            }
            _ => self.unsupported("return binding type is not supported"),
        }
    }

    fn unsupported<T>(&self, reason: impl Into<String>) -> Result<T, WalkError> {
        Err(WalkError::Unsupported(Unsupported::new(reason)))
    }
}

fn predicate_support<'a>(
    predicate: &'a PredicateDef,
    iargs: &[austen_lib::spec::IndexTerm],
    expected: &BaseType,
) -> Result<&'a [austen_lib::spec::Clause], WalkError> {
    check_predicate_call(predicate, iargs, expected)
        .map_err(|defect| WalkError::Unsupported(Unsupported::new(defect.to_string())))
}

fn terminal<I>(chain: &Lat<I>) -> &Lat<I> {
    let mut node = chain;
    loop {
        node = match node {
            Lat::Define(_, rest) | Lat::Resource(_, rest) | Lat::Constraint(_, rest) => rest,
            Lat::I(_) => return node,
        }
    }
}

fn index_value(bt: &BaseType, value: i128) -> Result<Value, WalkError> {
    match bt {
        BaseType::Bits(sign, width) => {
            bits(*sign, *width, value)
                .map(Value::Scalar)
                .ok_or_else(|| {
                    WalkError::Unsupported(Unsupported::new(
                        "quantifier index does not fit its declared type",
                    ))
                })
        }
        _ => Ok(Value::Scalar(Scalar::Int(value.into()))),
    }
}
