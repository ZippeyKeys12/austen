mod address;
pub(crate) mod blame;
pub(crate) mod trie;
mod walk;

use austen_lib::gen_dsl::choice::translate::{
    AsgnError, RootCallError, asgn_target_with_evaluator, check_asgn_granted_with_evaluator,
    choice_bits, enter_root_with_evaluator,
};
use austen_lib::gen_dsl::{
    BaseType, Congruence, Env, EnvMark, Evaluator, GenFailure, GenModule, GenTerm, GenTermKind,
    Grant, Incoming, IndexTerm, PointerOrigin, PrimitiveContext, PrimitiveDraw, PrimitiveGenerator,
    Region, Rewind, Rng, Scalar, Scope, Sctype, Sym, SymSet, TestCaseSize, Value,
    good_pointer_alignment,
};

use austen_lib::gen_dsl::{HeapMark, MarkedHeap};
use austen_lib::gen_dsl::{RejectReason, RejectTally};
pub(crate) use blame::ownership_capacity;
use blame::{Failure, FailureKind, OwnDomain, PointerReq};
use trie::{Trie, TrieMark};

pub(crate) enum Stop {
    Fail(Failure),
    Hard(GenFailure),
    Interrupted,
}

impl From<GenFailure> for Stop {
    fn from(f: GenFailure) -> Self {
        Stop::Hard(f)
    }
}

pub(crate) struct Attempt<'a, 'e, R> {
    pub rng: &'a mut R,
    pub heap: MarkedHeap,
    pub bounds: Region,
    pub incoming: &'a Incoming,
    pub capacity: u64,
    pub writes: Vec<Region>,
    pub initial_size: TestCaseSize,
    pub primitive: &'a dyn PrimitiveGenerator,
    pub max_backtracks: u32,
    pub steps: u64,
    pub ctx: &'a GenModule,
    pub evaluator: &'a Evaluator<'e>,
    pub tally: &'a mut RejectTally,
    pub site_index: &'a austen_lib::gen_dsl::SiteIndex,
    pub sites: &'a mut austen_lib::gen_dsl::SiteTally,
    pub current_term: Option<&'a GenTerm>,
    pub stop: &'a dyn Fn() -> bool,
    pub deadline: Option<std::time::Instant>,
}

impl<'a, R> Attempt<'a, '_, R> {
    fn reject(&mut self, reason: RejectReason) {
        self.tally.bump(reason);
        if let Some(term) = self.current_term {
            self.site_index.observe(self.sites, term, false);
        }
    }

    fn fail(
        &mut self,
        kind: FailureKind,
        reason: RejectReason,
        syms: impl IntoIterator<Item = Sym>,
    ) -> Stop {
        self.reject(reason);
        Stop::Fail(Failure::blaming(kind, reason, syms))
    }

    fn fail_unblamed(&mut self, kind: FailureKind, reason: RejectReason) -> Stop {
        self.reject(reason);
        Stop::Fail(Failure::unblamed(kind, reason))
    }

    fn step(&mut self) -> Result<(), Stop> {
        self.check_stop()?;
        if self
            .deadline
            .is_some_and(|at| std::time::Instant::now() >= at)
        {
            return Err(self.fail_unblamed(FailureKind::Timeout, RejectReason::GeneratorTimeout));
        }
        match self.steps.checked_sub(1) {
            Some(left) => {
                self.steps = left;
                Ok(())
            }
            None => Err(self.fail_unblamed(FailureKind::Timeout, RejectReason::SearchBudget)),
        }
    }

    fn check_stop(&self) -> Result<(), Stop> {
        if let Some(error) = self.evaluator.default_error() {
            return Err(Stop::Hard(error.into()));
        }
        if (self.stop)() {
            Err(Stop::Interrupted)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy)]
struct Checkpoint {
    env: EnvMark,
    writes: usize,
    heap: HeapMark,
    trie: TrieMark,
}

impl Checkpoint {
    fn save<R>(att: &Attempt<'_, '_, R>, trie: &Trie, env: &Env) -> Self {
        Checkpoint {
            env: env.mark(),
            writes: att.writes.len(),
            heap: att.heap.save(),
            trie: trie.mark(),
        }
    }

    fn restore<R>(self, att: &mut Attempt<'_, '_, R>, trie: &mut Trie, env: &mut Env) {
        env.rewind(self.env);
        att.writes.truncate(self.writes);
        att.heap.restore(self.heap);
        trie.rewind(self.trie);
    }
}

fn assign<R>(
    att: &mut Attempt<'_, '_, R>,
    term: &GenTerm,
    addr: &IndexTerm,
    ct: &Sctype,
    value: &IndexTerm,
    env: &Env,
) -> Result<(), Stop> {
    let target = match asgn_target_with_evaluator(addr, ct, env, att.ctx, att.evaluator) {
        Ok(target) => target,
        Err(AsgnError::UnsizedCtype { ct }) => {
            return Err(Stop::Hard(GenFailure::UnsizedCtype { ct }));
        }
        Err(_) => {
            return Err(att.fail(
                FailureKind::Assert,
                RejectReason::AsgnUnevaluatable,
                addr.free_vars().into_iter().chain(value.free_vars()),
            ));
        }
    };
    let extent = target.extent;
    let grant = att.incoming.grant_write(&extent);
    if !(extent.size == 0 && extent.base != 0) {
        match grant {
            Some(Grant::Global(_)) => {}
            Some(Grant::Heap) => {
                if !att.heap.covers(&extent) {
                    att.reject(RejectReason::AsgnOutOfBounds);
                    return Err(Stop::Fail(allocation_failure(
                        addr,
                        extent,
                        env,
                        att.capacity,
                    )));
                }
            }
            None if matches!(target.origin, PointerOrigin::Global(_)) => {
                return Err(att.fail(
                    FailureKind::Assert,
                    RejectReason::AsgnOutOfBounds,
                    addr.free_vars(),
                ));
            }
            None => {
                att.reject(RejectReason::AsgnOutOfBounds);
                return Err(Stop::Fail(allocation_failure(
                    addr,
                    extent,
                    env,
                    att.capacity,
                )));
            }
        }
    }
    match check_asgn_granted_with_evaluator(
        target,
        grant,
        ct,
        value,
        env,
        &mut att.writes,
        att.ctx,
        att.evaluator,
    ) {
        Ok(_) => {
            att.site_index.observe(att.sites, term, true);
            Ok(())
        }
        Err(AsgnError::Overlap { .. }) => {
            att.reject(RejectReason::AsgnOverlap);
            Err(Stop::Fail(pointer_failure(
                FailureKind::Assert,
                RejectReason::AsgnOverlap,
                addr,
                extent,
                env,
                att.capacity,
            )))
        }
        Err(AsgnError::Unevaluatable { .. }) => Err(att.fail(
            FailureKind::Assert,
            RejectReason::AsgnUnevaluatable,
            addr.free_vars().into_iter().chain(value.free_vars()),
        )),
        Err(other) => unreachable!("asgn already screened: {other:?}"),
    }
}

fn remap_call(
    mut f: Failure,
    def: &austen_lib::gen_dsl::GenDef,
    args: &[IndexTerm],
    callee_env: &Env,
    env: &Env,
    capacity: u64,
) -> Failure {
    if f.reason == RejectReason::AsgnOverlap
        && def
            .iargs
            .iter()
            .zip(args)
            .any(|((param, _), arg)| f.blames(param) && arg.as_sym().is_none())
    {
        f.kind = FailureKind::Assign;
    }

    let assignment = f.kind == FailureKind::Assign;
    f.remap_call(|p, requirement| {
        let at = def.iargs.iter().position(|(q, _)| q == p)?;
        let mut blamed = Vec::new();
        let direct_base = if let Some(requirement) = requirement
            && let Some(from) = callee_env.get(p).and_then(address::of)
        {
            address::for_each_base(&args[at], env, |sym, to| {
                blamed.push((sym, Some(requirement.rebase(from, to, capacity))));
            })
        } else {
            false
        };
        if !assignment || !direct_base {
            blamed.extend(args[at].free_vars().into_iter().map(|s| (s, None)));
        }
        Some(blamed)
    });
    f
}

fn exhausted<R>(
    att: &mut Attempt<'_, '_, R>,
    mut failure: Failure,
    var: &Sym,
    value: &GenTerm,
    deterministic: bool,
) -> Stop {
    att.reject(RejectReason::LetExhausted);
    if deterministic {
        failure.relabel(var, value.free_vars());
    } else {
        failure.take_blame(var);
    }
    Stop::Fail(failure)
}

fn draw<R: Rng>(
    att: &mut Attempt<'_, '_, R>,
    term: &GenTerm,
    correction: Option<Value>,
) -> Result<Option<Value>, Stop> {
    let GenTermKind::Arbitrary = &term.term else {
        unreachable!("draw is only called at an arbitrary");
    };
    choice_bits(&term.bt).map_err(Stop::Hard)?;
    if let Some(value) = correction {
        return Ok(Some(value));
    }

    let mut context = PrimitiveContext {
        rng: &mut *att.rng,
        size: att.initial_size,
        heap: &mut att.heap,
        heap_bounds: att.bounds,
        declarations: att.ctx.declarations(),
    };
    match att.primitive.arbitrary(&term.bt, &mut context) {
        Ok(PrimitiveDraw::Value(value)) => Ok(Some(value)),
        Ok(PrimitiveDraw::Rejected(reason)) => Err(att.fail_unblamed(FailureKind::Assert, reason)),
        Err(error) => Err(Stop::Hard(error)),
    }
}

fn global_repair_slot(
    incoming: &Incoming,
    writes: &[Region],
    want: u64,
    base_class: Congruence,
) -> Option<Region> {
    let mut occupied: Vec<Region> = Vec::new();
    incoming
        .globals_in_order()
        .filter(|(_, object)| object.writable)
        .find_map(|(_, object)| {
            let region = object.region;
            occupied.clear();
            occupied.extend(
                writes
                    .iter()
                    .copied()
                    .filter(|write| region.overlaps(write)),
            );
            region.lowest_congruent_gap(&mut occupied, want, base_class)
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepairPlanError {
    Capacity,
    Congruence,
}

fn plan_pointer_repair<R>(
    att: &mut Attempt<'_, '_, R>,
    requirement: PointerReq,
) -> Result<u64, RepairPlanError> {
    let demand = requirement.ownership;
    let want = demand.bytes().ok_or(RepairPlanError::Capacity)?;
    debug_assert!(want > 0, "top ownership does not require repair");
    let base_class = requirement
        .allocation_base_congruence()
        .ok_or(RepairPlanError::Congruence)?;

    let Some(region) = att
        .heap
        .try_alloc_congruent(want, base_class, att.bounds.size)
        .or_else(|| global_repair_slot(att.incoming, &att.writes, want, base_class))
    else {
        if base_class == Congruence::TOP {
            return Err(RepairPlanError::Capacity);
        }
        let heap_mark = att.heap.save();
        let raw_heap_slot = att.heap.try_alloc(want, att.bounds.size).is_some();
        att.heap.restore(heap_mark);
        let raw_slot = raw_heap_slot
            || global_repair_slot(att.incoming, &att.writes, want, Congruence::TOP).is_some();
        return Err(if raw_slot {
            RepairPlanError::Congruence
        } else {
            RepairPlanError::Capacity
        });
    };
    region
        .base
        .checked_add(demand.before)
        .ok_or(RepairPlanError::Congruence)
}

fn allocation_failure(addr: &IndexTerm, write: Region, env: &Env, capacity: u64) -> Failure {
    pointer_failure(
        FailureKind::Assign,
        RejectReason::AsgnOutOfBounds,
        addr,
        write,
        env,
        capacity,
    )
}

fn failed_pointer_good(
    constraint: &austen_lib::gen_dsl::LogicalConstraint,
    env: &Env,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
    capacity: u64,
) -> Option<Failure> {
    let austen_lib::gen_dsl::LogicalConstraint::T(IndexTerm {
        term: austen_lib::gen_dsl::Term::Good(Sctype::Pointer(pointee), about),
        ..
    }) = constraint
    else {
        return None;
    };
    let alignment = good_pointer_alignment(pointee, ctx.declarations().structs()).ok()?;
    let Value::Scalar(Scalar::Pointer {
        addr: checked_addr, ..
    }) = evaluator.eval(about, env)?
    else {
        return None;
    };

    let requirement = PointerReq::aligned(alignment);
    let mut failure = Failure::unblamed(FailureKind::Assign, RejectReason::AsgnOutOfBounds);
    address::blame_bases(&mut failure, about, env, |base| {
        requirement.rebase(checked_addr, base, capacity)
    });
    (!failure.is_unblamed()).then_some(failure)
}

fn pointer_failure(
    kind: FailureKind,
    reason: RejectReason,
    addr: &IndexTerm,
    write: Region,
    env: &Env,
    capacity: u64,
) -> Failure {
    let mut f = Failure::unblamed(kind, reason);
    address::blame_bases(&mut f, addr, env, |base| {
        PointerReq::owning(OwnDomain::covering(base, write.base, write.size, capacity))
    });
    f
}

pub(crate) fn run<'a, R: Rng + Rewind>(
    att: &mut Attempt<'a, '_, R>,
    term: &'a GenTerm,
) -> Result<Scope, Stop> {
    let mut trie = Trie::new();
    match enter_root_with_evaluator(term, att.ctx, att.incoming, att.evaluator) {
        Ok((body, mut env)) => walk::walk(att, body, &mut trie, &mut env)?,
        Err(RootCallError::Structural(error)) => return Err(Stop::Hard(error.into())),
        Err(RootCallError::CallArg(arg)) => {
            return Err(att.fail(FailureKind::Assert, RejectReason::CallArg, arg.free_vars()));
        }
        Err(RootCallError::RecursionBudget) => {
            return Err(att.fail_unblamed(FailureKind::Depth, RejectReason::RecursionBudget));
        }
    };
    trie.finish().map_err(|error| Stop::Hard(error.into()))
}
