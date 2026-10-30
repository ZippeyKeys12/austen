use austen_bennet::BennetPrimitiveGenerator;
use austen_lib::gen_dsl::choice::translate::{
    MAX_CALL_DEPTH, choice_bits, choice_value, index_scalar, scalar_value, spent_rec_size,
};
use austen_lib::gen_dsl::rand_like::lemire;
use std::collections::HashMap;

use austen_lib::gen_dsl::{
    BaseType, BinOp, Const, Env, Evaluator, GenFailure, GenModule, GenTerm, GenTermKind, Incoming,
    IndexTerm, LogicalConstraint, MapDomainError, MarkedHeap, PrimitiveContext, PrimitiveDraw,
    PrimitiveGenerator, RejectReason, Rng, Sctype, Substitution, Sym, SymGen, SymSet, Term,
    TestCaseSize, Value, eval_in,
};

use crate::map_plan::{MapPlan, bool_binop, negate};

mod skew;
pub(crate) use skew::Precheck;

fn size_literal(size: i128) -> IndexTerm {
    IndexTerm::num_lit(
        num_bigint::BigInt::from(size),
        &austen_lib::gen_dsl::uintptr_bt(),
    )
    .expect("a size value fits the size arguments' base type")
}

#[derive(Debug, Clone)]
pub struct DrawVar {
    pub sym: Sym,
    pub bt: BaseType,
    pub skew: i128,
}

#[derive(Debug, Clone)]
pub struct ResourceClaim {
    pub active: IndexTerm,
    pub first_addr: IndexTerm,
    pub last_addr: IndexTerm,
    pub tail_bytes: u64,
}

#[derive(Debug, Clone, Default)]
pub struct SymbolicPath {
    pub defaults: austen_lib::gen_dsl::DefaultChoices,
    pub size: u64,
    pub draws: Vec<DrawVar>,
    pub constraints: Vec<LogicalConstraint>,
    pub pins: Vec<IndexTerm>,
    pub resources: Vec<ResourceClaim>,
    pub sites: PathSites,
}

#[derive(Debug, Clone, Default)]
pub struct PathSites {
    pub draws: Vec<Option<austen_lib::gen_dsl::SiteId>>,
    pub constraints: Vec<Option<austen_lib::gen_dsl::SiteId>>,
    pub pins: Vec<Option<austen_lib::gen_dsl::SiteId>>,
    pub resources: Vec<Option<austen_lib::gen_dsl::SiteId>>,
}

#[derive(Debug, Clone, Copy)]
struct Mark {
    draws: usize,
    constraints: usize,
    pins: usize,
    resources: usize,
}

impl SymbolicPath {
    pub fn default_types(&self, module: &GenModule) -> std::collections::BTreeSet<BaseType> {
        let mut types = std::collections::BTreeSet::new();
        let mut visit = |term: &IndexTerm| {
            types.extend(austen_lib::gen_dsl::smt::input_default_types(
                term,
                &module.functions,
            ));
        };
        for constraint in &self.constraints {
            match constraint {
                LogicalConstraint::T(term) => visit(term),
                LogicalConstraint::Forall {
                    permission, body, ..
                } => {
                    visit(permission);
                    visit(body);
                }
            }
        }
        for term in &self.pins {
            visit(term);
        }
        for resource in &self.resources {
            visit(&resource.active);
            visit(&resource.first_addr);
            visit(&resource.last_addr);
        }
        types
    }

    fn mark(&self) -> Mark {
        Mark {
            draws: self.draws.len(),
            constraints: self.constraints.len(),
            pins: self.pins.len(),
            resources: self.resources.len(),
        }
    }

    fn rewind(&mut self, mark: Mark) {
        self.draws.truncate(mark.draws);
        self.constraints.truncate(mark.constraints);
        self.pins.truncate(mark.pins);
        self.resources.truncate(mark.resources);
        self.sites.draws.truncate(mark.draws);
        self.sites.constraints.truncate(mark.constraints);
        self.sites.pins.truncate(mark.pins);
        self.sites.resources.truncate(mark.resources);
    }
}

pub enum Stop {
    Reject(RejectReason),
    Hard(GenFailure),
}

#[derive(Debug, Clone)]
struct Dual {
    sampled: Option<Value>,
    symbolic: IndexTerm,
}

#[derive(Default)]
struct ValueDemand {
    result: bool,
    bindings: HashMap<(*const GenTerm, bool), bool>,
}

impl ValueDemand {
    fn binding(&mut self, binder: &GenTerm, var: &Sym, rest: &GenTerm) -> bool {
        let key = (std::ptr::from_ref(binder), self.result);
        if let Some(needed) = self.bindings.get(&key) {
            return *needed;
        }
        let needed = self.inputs(rest, self.result).contains(var);
        self.bindings.insert(key, needed);
        needed
    }

    fn inputs(&mut self, term: &GenTerm, result: bool) -> SymSet {
        match &term.term {
            GenTermKind::Arbitrary => SymSet::new(),
            GenTermKind::Return(value) => {
                if result {
                    value.free_vars()
                } else {
                    SymSet::new()
                }
            }
            GenTermKind::WriteChoice { rest, .. } => self.inputs(rest, result),
            GenTermKind::Scope { body, .. } => self.inputs(body, result),
            GenTermKind::LetStar { var, value, rest } => {
                let mut inputs = self.inputs(rest, result);
                let needed = inputs.remove(var);
                self.bindings
                    .insert((std::ptr::from_ref(term), result), needed);
                inputs.extend(self.inputs(value, needed));
                inputs
            }
            GenTermKind::Assert { constraint, rest } => {
                let mut inputs = self.inputs(rest, result);
                inputs.extend(constraint.free_vars());
                inputs
            }
            GenTermKind::Asgn { addr, rest, .. } => {
                let mut inputs = self.inputs(rest, result);
                inputs.extend(addr.free_vars());
                inputs
            }
            GenTermKind::Ite { cond, then_, else_ } => {
                let mut inputs = self.inputs(then_, result);
                inputs.extend(self.inputs(else_, result));
                inputs.extend(cond.free_vars());
                inputs
            }
            GenTermKind::Call { args, .. } => args.iter().flat_map(IndexTerm::free_vars).collect(),
            GenTermKind::Map {
                var, perm, body, ..
            } => {
                let mut inputs = self.inputs(body, result);
                inputs.extend(perm.free_vars());
                inputs.remove(var);
                inputs
            }
        }
    }
}

struct MapElement {
    sampled_key: i128,
    symbolic_key: IndexTerm,
    sampled_active: bool,
    membership: IndexTerm,
    value: Option<Dual>,
}

struct ArrayBody<'a> {
    draw_term: &'a GenTerm,
    asgn_term: &'a GenTerm,
    draw_bt: &'a BaseType,
    addr: &'a IndexTerm,
    ct: &'a Sctype,
}

fn array_body<'a>(
    map_var: &Sym,
    map_var_bt: &BaseType,
    mut body: &'a GenTerm,
) -> Option<ArrayBody<'a>> {
    while let GenTermKind::Scope { body: inner, .. } = &body.term {
        body = inner;
    }
    let GenTermKind::LetStar {
        var: draw,
        value,
        rest,
    } = &body.term
    else {
        return None;
    };
    if !matches!(value.term, GenTermKind::Arbitrary) || value.bt != body.bt {
        return None;
    }

    let mut assignment = None;
    if !array_tail(rest, draw, &mut assignment) {
        return None;
    }
    let (asgn_term, addr, ct) = assignment?;
    let Term::ArrayShift {
        base,
        ct: shifted_ct,
        index,
    } = &addr.term
    else {
        return None;
    };
    if shifted_ct != ct || !is_map_index(index, map_var, map_var_bt) {
        return None;
    }
    let free = base.free_vars();
    if free.contains(map_var) || free.contains(draw) {
        return None;
    }
    Some(ArrayBody {
        draw_term: value,
        asgn_term,
        draw_bt: &value.bt,
        addr,
        ct,
    })
}

fn array_tail<'a>(
    term: &'a GenTerm,
    draw: &Sym,
    assignment: &mut Option<(&'a GenTerm, &'a IndexTerm, &'a Sctype)>,
) -> bool {
    match &term.term {
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } if value.as_sym() == Some(draw) && assignment.is_none() => {
            *assignment = Some((term, addr, ct));
            array_tail(rest, draw, assignment)
        }
        GenTermKind::LetStar { value, rest, .. } if recording_only(value, draw) => {
            array_tail(rest, draw, assignment)
        }
        GenTermKind::WriteChoice { value, rest, .. } if value.as_sym() == Some(draw) => {
            array_tail(rest, draw, assignment)
        }
        GenTermKind::Scope { body, .. } => array_tail(body, draw, assignment),
        GenTermKind::Return(value) => value.as_sym() == Some(draw) && assignment.is_some(),
        _ => false,
    }
}

fn recording_only(term: &GenTerm, draw: &Sym) -> bool {
    match &term.term {
        GenTermKind::Return(value) => matches!(value.term, Term::Const(Const::Unit)),
        GenTermKind::WriteChoice { value, rest, .. } => {
            value.as_sym() == Some(draw) && recording_only(rest, draw)
        }
        GenTermKind::Scope { body, .. } => recording_only(body, draw),
        GenTermKind::LetStar { value, rest, .. } => {
            recording_only(value, draw) && recording_only(rest, draw)
        }
        _ => false,
    }
}

fn is_map_index(term: &IndexTerm, var: &Sym, var_bt: &BaseType) -> bool {
    match &term.term {
        Term::Sym(sym) => sym == var && *var_bt == austen_lib::gen_dsl::uintptr_bt(),
        Term::Cast(target, inner)
            if *target == austen_lib::gen_dsl::uintptr_bt()
                && inner.bt == *var_bt
                && matches!(var_bt, BaseType::Bits(_, width) if *width <= 64) =>
        {
            matches!(&inner.term, Term::Sym(sym) if sym == var)
        }
        _ => false,
    }
}

fn map_dual(key_bt: &BaseType, elem_bt: &BaseType, elements: Vec<MapElement>) -> Option<Dual> {
    let map_bt = BaseType::make_map(key_bt.clone(), elem_bt.clone());
    let default = IndexTerm {
        term: Term::Const(Const::Unknown(elem_bt.clone())),
        bt: elem_bt.clone(),
    };
    let mut symbolic = IndexTerm {
        term: Term::MapConst(key_bt.clone(), Box::new(default.clone())),
        bt: map_bt.clone(),
    };
    let mut entries = Some(Vec::new());
    for element in elements {
        let dual = element.value?;
        if element.sampled_active {
            match (entries.as_mut(), dual.sampled) {
                (Some(es), Some(value)) => es.push((element.sampled_key, value)),
                _ => entries = None,
            }
        }
        let value = ite_term(element.membership, dual.symbolic, default.clone());
        symbolic = IndexTerm {
            term: Term::MapSet(
                Box::new(symbolic),
                Box::new(element.symbolic_key),
                Box::new(value),
            ),
            bt: map_bt.clone(),
        };
    }
    Some(Dual {
        sampled: entries.map(Value::iteration_map),
        symbolic,
    })
}

pub struct Setup<'a> {
    pub term: &'a GenTerm,
    pub ctx: &'a GenModule,
    pub incoming: &'a Incoming,
    pub max_array_length: usize,
    pub deadline: Option<std::time::Instant>,
}

pub struct Gather<'a, 'e, R> {
    rng: &'a mut R,
    setup: &'a Setup<'a>,
    initial_size: TestCaseSize,
    skew_heap: MarkedHeap,
    syms: SymGen,
    active: IndexTerm,
    steps: u64,
    out: SymbolicPath,
    skew_check: skew::SkewCheck,
    demand: ValueDemand,
    site_index: &'a austen_lib::gen_dsl::SiteIndex,
    current_site: Option<&'a austen_lib::gen_dsl::SiteId>,
    evaluator: &'a Evaluator<'e>,
}

pub const MAX_GATHER_STEPS: u64 = 1 << 20;

impl<'a, 'e, R: Rng> Gather<'a, 'e, R> {
    pub(crate) fn run_with_evaluator(
        setup: &Setup<'_>,
        size: TestCaseSize,
        sym_seed: u64,
        rng: &mut R,
        site_index: &austen_lib::gen_dsl::SiteIndex,
        evaluator: &Evaluator<'_>,
    ) -> Result<(SymbolicPath, Precheck), Stop> {
        let mut gather = Gather {
            rng,
            setup,
            site_index,
            current_site: None,
            initial_size: size,
            skew_heap: MarkedHeap::with_base(setup.incoming.heap().base),
            syms: SymGen::starting_at(sym_seed),
            active: IndexTerm::boolean(true),
            steps: 0,
            out: SymbolicPath {
                size: size.get(),
                ..SymbolicPath::default()
            },
            skew_check: skew::SkewCheck::default(),
            demand: ValueDemand::default(),
            evaluator,
        };
        let mut sampled = Env::new();
        let mut symbolic = Substitution::new();
        setup.incoming.bind_globals(&mut sampled);
        for (sym, global) in &setup.ctx.globals {
            let Some(object) = setup.incoming.global(sym) else {
                continue;
            };
            symbolic.insert(
                sym.clone(),
                IndexTerm {
                    term: Term::Const(Const::Pointer {
                        alloc_id: num_bigint::BigInt::ZERO,
                        addr: num_bigint::BigInt::from(object.address()),
                    }),
                    bt: BaseType::loc_to(global.ty.clone()),
                },
            );
        }
        let result = gather.walk_root(setup.term, &mut sampled, &mut symbolic);
        if let Some(error) = evaluator.default_error() {
            return Err(Stop::Hard(error.into()));
        }
        result?;
        for ty in gather.out.default_types(setup.ctx) {
            evaluator
                .resolve_default(&ty)
                .map_err(|error| Stop::Hard(error.into()))?;
        }
        gather.out.defaults = evaluator.default_choices();
        Ok((gather.out, gather.skew_check.verdict()))
    }

    fn walk_root(
        &mut self,
        term: &'a GenTerm,
        sampled: &mut Env,
        symbolic: &mut Substitution,
    ) -> Result<Option<Dual>, Stop> {
        let GenTermKind::Call { fsym, args } = &term.term else {
            return self.walk(0, term, sampled, symbolic);
        };
        let def = self
            .setup
            .ctx
            .resolve_call(fsym, args.len())
            .map_err(|error| Stop::Hard(error.into()))?;
        let mut callee_sampled = Env::new();
        let mut callee_symbolic = Substitution::new();
        for ((param, _), arg) in def.iargs.iter().zip(args) {
            let Some(value) = self.evaluator.eval(arg, sampled) else {
                return Err(Stop::Reject(RejectReason::CallArg));
            };
            callee_sampled.insert(param, value.clone());
            let symbolic_arg = self.call_argument(
                arg,
                &value,
                symbolic,
                def.size_args().any(|size| size == param),
            );
            callee_symbolic.insert(param.clone(), symbolic_arg);
        }
        if spent_rec_size(def, &callee_sampled) {
            return Err(Stop::Reject(RejectReason::RecursionBudget));
        }
        self.walk(0, &def.body, &mut callee_sampled, &mut callee_symbolic)
    }

    fn walk(
        &mut self,
        depth: usize,
        term: &'a GenTerm,
        sampled: &mut Env,
        symbolic: &mut Substitution,
    ) -> Result<Option<Dual>, Stop> {
        self.at_site(term, |this| this.walk_inner(depth, term, sampled, symbolic))
    }

    fn at_site<T>(&mut self, term: &'a GenTerm, f: impl FnOnce(&mut Self) -> T) -> T {
        let previous = std::mem::replace(&mut self.current_site, self.site_index.get(term));
        let result = f(self);
        self.current_site = previous;
        result
    }

    fn walk_inner(
        &mut self,
        depth: usize,
        term: &'a GenTerm,
        sampled: &mut Env,
        symbolic: &mut Substitution,
    ) -> Result<Option<Dual>, Stop> {
        self.charge_steps(1)?;
        match &term.term {
            GenTermKind::Arbitrary => self.arbitrary(&term.bt),
            GenTermKind::WriteChoice { rest, .. } => self.walk(depth, rest, sampled, symbolic),

            GenTermKind::LetStar { var, value, rest } => {
                let needed = self.demand.binding(term, var, rest);
                let previous = std::mem::replace(&mut self.demand.result, needed);
                let bound = self.walk(depth, value, sampled, symbolic);
                self.demand.result = previous;
                let Some(dual) = bound? else {
                    return self.walk(depth, rest, sampled, symbolic);
                };
                let named = self.name(dual.symbolic, dual.sampled.as_ref());
                let previous = symbolic.insert(var.clone(), named);
                let mark = sampled.mark();
                if let Some(value) = dual.sampled {
                    sampled.insert(var, value);
                }
                let out = self.walk(depth, rest, sampled, symbolic);
                sampled.rewind(mark);
                match previous {
                    Some(value) => symbolic.insert(var.clone(), value),
                    None => symbolic.remove(var),
                };
                out
            }

            GenTermKind::Return(value) => Ok(self.demand.result.then(|| Dual {
                sampled: self.evaluator.eval(value, sampled),
                symbolic: value.subst(symbolic),
            })),

            GenTermKind::Assert { constraint, rest } => {
                self.push_constraint(constraint.subst(symbolic));
                self.walk(depth, rest, sampled, symbolic)
            }

            GenTermKind::Scope { body, .. } => self.walk(depth, body, sampled, symbolic),

            GenTermKind::Ite { cond, then_, else_ } => {
                self.ite(depth, cond, then_, else_, sampled, symbolic)
            }

            GenTermKind::Call { fsym, args } => self.call(depth, fsym, args, sampled, symbolic),

            GenTermKind::Map {
                var,
                var_bt,
                perm,
                body,
            } => self.map(depth, (var, var_bt, perm, body), sampled, symbolic),

            GenTermKind::Asgn { addr, ct, rest, .. } => {
                let Some(bytes) = ct.byte_size_in(&self.setup.ctx.structs) else {
                    return Err(Stop::Hard(GenFailure::UnsizedCtype { ct: ct.clone() }));
                };
                let addr = addr.subst(symbolic);
                self.out.sites.resources.push(self.current_site.cloned());
                let claim = ResourceClaim {
                    active: self.active.clone(),
                    first_addr: addr.clone(),
                    last_addr: addr,
                    tail_bytes: bytes,
                };
                self.push_resource(claim);
                self.walk(depth, rest, sampled, symbolic)
            }
        }
    }

    fn push_constraint(&mut self, constraint: LogicalConstraint) {
        let constraint = match constraint {
            LogicalConstraint::T(body) => LogicalConstraint::T(implies(self.active.clone(), body)),
            LogicalConstraint::Forall {
                var,
                permission,
                body,
            } => LogicalConstraint::Forall {
                var,
                permission: and(self.active.clone(), permission),
                body,
            },
        };
        self.out.sites.constraints.push(self.current_site.cloned());
        self.skew_check
            .constraint_with_evaluator(&constraint, self.setup.ctx, self.evaluator);
        self.out.constraints.push(constraint);
    }

    fn push_resource(&mut self, claim: ResourceClaim) {
        self.skew_check.resource_with_evaluator(
            &claim,
            self.setup.ctx,
            self.setup.incoming,
            self.evaluator,
        );
        self.out.resources.push(claim);
    }

    fn name(&mut self, term: IndexTerm, sampled: Option<&Value>) -> IndexTerm {
        if matches!(term.term, Term::Sym(_) | Term::Const(_))
            || !austen_lib::gen_dsl::smt::input_default_types(&term, &self.setup.ctx.functions)
                .is_empty()
        {
            return term;
        }
        let sym = self.syms.fresh("darcy-let");
        self.skew_check.bind(&sym, sampled);
        let named = IndexTerm::sym(sym, term.bt.clone());
        self.out.sites.constraints.push(self.current_site.cloned());
        self.out
            .constraints
            .push(LogicalConstraint::T(IndexTerm::eq(named.clone(), term)));
        named
    }

    fn call_argument(
        &mut self,
        arg: &IndexTerm,
        value: &Value,
        symbolic: &Substitution,
        is_size: bool,
    ) -> IndexTerm {
        let arg = arg.subst(symbolic);
        if is_size
            && austen_lib::gen_dsl::smt::input_default_types(&arg, &self.setup.ctx.functions)
                .is_empty()
        {
            size_literal(
                value
                    .as_int()
                    .expect("a size argument evaluates to an integer"),
            )
        } else {
            self.name(arg, Some(value))
        }
    }

    fn arbitrary(&mut self, bt: &BaseType) -> Result<Option<Dual>, Stop> {
        choice_bits(bt).map_err(Stop::Hard)?;
        let outcome = {
            let mut context = PrimitiveContext {
                rng: &mut *self.rng,
                size: self.initial_size,
                heap: &mut self.skew_heap,
                heap_bounds: self.setup.incoming.heap(),
                declarations: self.setup.ctx.declarations(),
            };
            BennetPrimitiveGenerator.arbitrary(bt, &mut context)
        };
        let value = match outcome.map_err(Stop::Hard)? {
            PrimitiveDraw::Value(value) => value,
            PrimitiveDraw::Rejected(reason) => return Err(Stop::Reject(reason)),
        };
        let skew = choice_value(&value, bt)
            .expect("Bennet primitive draws inhabit their requested scalar type");
        let sym = self.syms.fresh("darcy");
        self.out.sites.draws.push(self.current_site.cloned());
        let draw = DrawVar {
            sym: sym.clone(),
            bt: bt.clone(),
            skew,
        };
        self.out.draws.push(draw);
        Ok(self.demand.result.then(|| {
            let sampled = scalar_value(skew, bt);
            self.skew_check.bind(&sym, sampled.as_ref());
            Dual {
                sampled,
                symbolic: IndexTerm::sym(sym, bt.clone()),
            }
        }))
    }

    fn ite(
        &mut self,
        depth: usize,
        cond: &'a IndexTerm,
        then_: &'a GenTerm,
        else_: &'a GenTerm,
        sampled: &mut Env,
        symbolic: &mut Substitution,
    ) -> Result<Option<Dual>, Stop> {
        let guard = cond.subst(symbolic);
        let sampled_default_route =
            (!austen_lib::gen_dsl::smt::input_default_types(&guard, &self.setup.ctx.functions)
                .is_empty())
            .then(|| {
                self.evaluator
                    .eval(cond, sampled)
                    .and_then(|value| value.as_bool())
            })
            .flatten();
        let first = sampled_default_route.unwrap_or_else(|| lemire::uniform_u64(self.rng, 2) == 1);
        let pick = |take_then: bool| {
            if take_then {
                (guard.clone(), then_)
            } else {
                (negate(guard.clone()), else_)
            }
        };
        let mark = self.out.mark();
        let skew_mark = self.skew_check.mark();
        let (pin, body) = pick(first);
        match self.arm(depth, pin, body, sampled, symbolic) {
            Err(Stop::Reject(RejectReason::RecursionBudget | RejectReason::CallDepth)) => {
                self.out.rewind(mark);
                self.skew_check.rewind(skew_mark);
                let (pin, body) = pick(!first);
                self.arm(depth, pin, body, sampled, symbolic)
            }
            out => out,
        }
    }

    fn arm(
        &mut self,
        depth: usize,
        pin: IndexTerm,
        body: &'a GenTerm,
        sampled: &mut Env,
        symbolic: &mut Substitution,
    ) -> Result<Option<Dual>, Stop> {
        self.out.sites.pins.push(self.current_site.cloned());
        self.skew_check
            .pin_with_evaluator(&pin, self.setup.ctx, self.evaluator);
        self.out.pins.push(pin);
        self.walk(depth, body, sampled, symbolic)
    }

    fn call(
        &mut self,
        depth: usize,
        fsym: &'a Sym,
        args: &'a [IndexTerm],
        sampled: &mut Env,
        symbolic: &mut Substitution,
    ) -> Result<Option<Dual>, Stop> {
        let def = self
            .setup
            .ctx
            .resolve_call(fsym, args.len())
            .map_err(|error| Stop::Hard(error.into()))?;
        if depth >= MAX_CALL_DEPTH {
            return Err(Stop::Reject(RejectReason::CallDepth));
        }
        let mut callee_sampled = Env::new();
        let mut callee_symbolic = Substitution::new();
        for ((param, _), arg) in def.iargs.iter().zip(args) {
            let Some(value) = self.evaluator.eval(arg, sampled) else {
                return Err(Stop::Reject(RejectReason::CallArg));
            };
            callee_sampled.insert(param, value.clone());
            let symbolic_arg = self.call_argument(
                arg,
                &value,
                symbolic,
                def.size_args().any(|size| size == param),
            );
            callee_symbolic.insert(param.clone(), symbolic_arg);
        }
        if spent_rec_size(def, &callee_sampled) {
            return Err(Stop::Reject(RejectReason::RecursionBudget));
        }
        self.walk(
            depth + 1,
            &def.body,
            &mut callee_sampled,
            &mut callee_symbolic,
        )
    }

    fn map(
        &mut self,
        depth: usize,
        (var, var_bt, perm, body): (&'a Sym, &'a BaseType, &'a IndexTerm, &'a GenTerm),
        sampled: &mut Env,
        symbolic: &mut Substitution,
    ) -> Result<Option<Dual>, Stop> {
        let plan = MapPlan::new(
            perm,
            var,
            var_bt,
            self.setup.ctx,
            self.setup.max_array_length,
        )
        .map_err(map_plan_stop)?;
        self.push_constraint(LogicalConstraint::T(
            plan.length_constraint().subst(symbolic),
        ));

        if plan.is_contiguous()
            && let Some(array) = array_body(var, var_bt, body)
        {
            let Some(bytes) = array.ct.byte_size_in(&self.setup.ctx.structs) else {
                return Err(Stop::Hard(GenFailure::UnsizedCtype {
                    ct: array.ct.clone(),
                }));
            };
            let span_fits = u64::try_from(plan.candidate_count().saturating_sub(1))
                .ok()
                .and_then(|offset| offset.checked_mul(bytes))
                .is_some();
            if bytes > 0 && span_fits && lower_is_nonnegative(&plan, var_bt, self.setup.ctx) {
                self.out
                    .sites
                    .resources
                    .push(self.site_index.get(array.asgn_term).cloned());
                let claim = ResourceClaim {
                    active: and(self.active.clone(), plan.nonempty().subst(symbolic)),
                    first_addr: subst_at(array.addr, var, plan.lower().subst(symbolic), symbolic),
                    last_addr: subst_at(array.addr, var, plan.upper().subst(symbolic), symbolic),
                    tail_bytes: bytes,
                };
                self.push_resource(claim);

                self.charge_steps(plan.candidate_count())?;
                let mut elements = Vec::new();
                for offset in 0..plan.candidate_count() {
                    let value =
                        self.at_site(array.draw_term, |this| this.arbitrary(array.draw_bt))?;
                    if value.is_some() {
                        let candidate =
                            self.map_candidate(&plan, perm, var, offset, sampled, symbolic)?;
                        elements.push(MapElement { value, ..candidate });
                    }
                }
                return Ok(self
                    .demand
                    .result
                    .then(|| map_dual(var_bt, &body.bt, elements))
                    .flatten());
            }
        }

        let parent_active = self.active.clone();
        let mut elements = Vec::with_capacity(plan.candidate_count());
        for offset in 0..plan.candidate_count() {
            let candidate = self.map_candidate(&plan, perm, var, offset, sampled, symbolic)?;
            let previous = symbolic.insert(var.clone(), candidate.symbolic_key.clone());
            self.active = and(parent_active.clone(), candidate.membership.clone());
            let value = sampled.scoped(
                var,
                index_scalar(var_bt, candidate.sampled_key),
                |sampled| self.walk(depth, body, sampled, symbolic),
            );
            self.active = parent_active.clone();
            match previous {
                Some(value) => symbolic.insert(var.clone(), value),
                None => symbolic.remove(var),
            };
            elements.push(MapElement {
                value: value?,
                ..candidate
            });
        }
        Ok(self
            .demand
            .result
            .then(|| map_dual(var_bt, &body.bt, elements))
            .flatten())
    }

    fn charge_steps(&mut self, additional: usize) -> Result<(), Stop> {
        if self
            .setup
            .deadline
            .is_some_and(|at| std::time::Instant::now() >= at)
        {
            return Err(Stop::Reject(RejectReason::GeneratorTimeout));
        }
        let additional =
            u64::try_from(additional).map_err(|_| Stop::Reject(RejectReason::SearchBudget))?;
        let steps = self
            .steps
            .checked_add(additional)
            .filter(|steps| *steps <= MAX_GATHER_STEPS)
            .ok_or(Stop::Reject(RejectReason::SearchBudget))?;
        self.steps = steps;
        Ok(())
    }

    fn map_candidate(
        &self,
        plan: &MapPlan,
        permission: &IndexTerm,
        var: &Sym,
        offset: usize,
        sampled: &Env,
        symbolic: &Substitution,
    ) -> Result<MapElement, Stop> {
        let source_key = plan.candidate(offset).map_err(map_plan_stop)?;
        let sampled_key = self
            .evaluator
            .eval(&source_key, sampled)
            .and_then(|value| value.as_int())
            .ok_or(Stop::Reject(RejectReason::MapBound))?;
        let source_permission = MapPlan::permission_at(permission, var, source_key.clone());
        let sampled_active = self
            .evaluator
            .eval(&source_permission, sampled)
            .and_then(|value| value.as_bool())
            .ok_or(Stop::Reject(RejectReason::MapBound))?;
        let membership = source_permission.subst(symbolic);
        Ok(MapElement {
            sampled_key,
            symbolic_key: source_key.subst(symbolic),
            sampled_active,
            membership,
            value: None,
        })
    }
}

fn map_plan_stop(error: MapDomainError) -> Stop {
    match error {
        MapDomainError::Unsupported => Stop::Hard(GenFailure::UnsupportedMapDomain),
        MapDomainError::Unevaluatable | MapDomainError::TooLarge { .. } => {
            Stop::Reject(RejectReason::MapBound)
        }
    }
}

fn lower_is_nonnegative(plan: &MapPlan, bt: &BaseType, ctx: &GenModule) -> bool {
    if matches!(bt, BaseType::Bits(austen_lib::gen_dsl::Sign::Unsigned, _)) {
        return true;
    }
    eval_in(plan.lower(), &austen_lib::gen_dsl::SymMap::new(), ctx)
        .and_then(|value| value.as_int())
        .is_some_and(|lower| lower >= 0)
}

fn subst_at(term: &IndexTerm, var: &Sym, key: IndexTerm, outer: &Substitution) -> IndexTerm {
    let mut subst = outer.clone();
    subst.insert(var.clone(), key);
    term.subst(&subst)
}

fn and(left: IndexTerm, right: IndexTerm) -> IndexTerm {
    if left.is_true() {
        return right;
    }
    if right.is_true() {
        return left;
    }
    IndexTerm::and2(left, right)
}

fn implies(left: IndexTerm, right: IndexTerm) -> IndexTerm {
    if left.is_true() {
        return right;
    }
    bool_binop(BinOp::Implies, left, right)
}

fn ite_term(cond: IndexTerm, then_: IndexTerm, else_: IndexTerm) -> IndexTerm {
    debug_assert_eq!(then_.bt, else_.bt, "ITE arms must have equal base types");
    let bt = then_.bt.clone();
    IndexTerm {
        term: Term::Ite(Box::new(cond), Box::new(then_), Box::new(else_)),
        bt,
    }
}
