use super::PassOptions;
use super::branch_pruning::{failed, ite_from_arms, possible, prune_under};
use super::path_condition::{Guard, PathConditions};

use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::index_term::{BinOp, IndexTerm, Term, UnOp};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::sym::{SymGen, SymSet};

pub fn lift_constraints(term: GenTerm, syms: &mut SymGen) -> GenTerm {
    lift_constraints_with_options(term, syms, PassOptions::NONE)
}

pub fn lift_constraints_with_options(
    term: GenTerm,
    syms: &mut SymGen,
    options: PassOptions,
) -> GenTerm {
    lift_constraints_bounded(term, syms, MAX_LIFT_GROWTH, options.path_pruning)
}

fn lift_constraints_bounded(
    term: GenTerm,
    syms: &mut SymGen,
    budget: usize,
    path_pruning: bool,
) -> GenTerm {
    let mut state = LiftState {
        syms: Some(syms),
        budget,
        path_pruning,
        path: PathConditions::default(),
    };
    let term = ext_walk(&SymSet::new(), term, false, &mut state, implication_assert);
    let term = ext_walk(&SymSet::new(), term, true, &mut state, disjunction_assert);
    let term = ext_walk(&SymSet::new(), term, true, &mut state, ite_assert);
    let term = lift_let(term);
    lift_conjunction(term)
}

pub const MAX_LIFT_GROWTH: usize = 1 << 24;

struct LiftState<'a> {
    syms: Option<&'a mut SymGen>,
    budget: usize,
    path_pruning: bool,
    path: PathConditions,
}

impl LiftState<'_> {
    fn charge(&mut self, cost: usize) -> bool {
        if cost > self.budget {
            return false;
        }
        self.budget -= cost;
        true
    }

    fn assuming<T>(&mut self, guards: &[&Guard], f: impl FnOnce(&mut Self) -> T) -> Option<T> {
        let mark = self.path.mark();
        let result = if guards.iter().all(|guard| self.path.assume_guard(guard)) {
            Some(f(self))
        } else {
            None
        };
        self.path.rewind(mark);
        result
    }

    fn possible(&mut self, rest: &GenTerm, guards: &[&Guard]) -> bool {
        self.assuming(guards, |state| possible(rest, &mut state.path))
            .unwrap_or(false)
    }

    fn continuation(&mut self, rest: &GenTerm, guards: &[&Guard]) -> GenTerm {
        self.assuming(guards, |state| prune_under(rest, &mut state.path))
            .expect("a surviving alternative has consistent assumptions")
    }

    fn charge_copies(&mut self, copies: usize, rest: &GenTerm) -> bool {
        copies == 0 || (self.budget != 0 && self.charge(copies.saturating_mul(term_bytes(rest))))
    }
}

pub(super) fn term_bytes(gt: &GenTerm) -> usize {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, gt).expect("counting serialization cannot fail");
    counter.0
}

pub fn lift_implication(term: GenTerm) -> GenTerm {
    lift_implication_bounded(term, MAX_LIFT_GROWTH, false)
}

fn lift_implication_bounded(term: GenTerm, budget: usize, path_pruning: bool) -> GenTerm {
    let mut state = LiftState {
        syms: None,
        budget,
        path_pruning,
        path: PathConditions::default(),
    };
    ext_walk(&SymSet::new(), term, false, &mut state, implication_assert)
}

fn implication_assert(
    ext: &SymSet,
    constraint: LogicalConstraint,
    rest: GenTerm,
    _bt: BaseType,
    state: &mut LiftState,
) -> GenTerm {
    let rest = ext_walk(ext, rest, false, state, implication_assert);
    match constraint {
        LogicalConstraint::T(it) => lift_consequent(it, rest, state),
        lc @ LogicalConstraint::Forall { .. } => GenTerm::assert(lc, rest),
    }
}

fn lift_consequent(it: IndexTerm, rest: GenTerm, state: &mut LiftState) -> GenTerm {
    lift_consequent_over(it, rest, None, state)
}

fn lift_consequent_over(
    it: IndexTerm,
    rest: GenTerm,
    known_cost: Option<usize>,
    state: &mut LiftState,
) -> GenTerm {
    let Term::Binop(BinOp::Implies, a, b) = &it.term else {
        return GenTerm::assert(LogicalConstraint::T(it), rest);
    };
    if !state.path_pruning {
        if state.budget == 0 {
            return GenTerm::assert(LogicalConstraint::T(it), rest);
        }
        let cost = known_cost.unwrap_or_else(|| term_bytes(&rest));
        if !state.charge(cost) {
            return GenTerm::assert(LogicalConstraint::T(it), rest);
        }
        let Term::Binop(BinOp::Implies, a, b) = it.into_parts().0 else {
            unreachable!("matched an implication");
        };
        let then_ = lift_consequent_over(*b, rest.clone(), Some(cost), state);
        return GenTerm::ite(*a, then_, rest);
    }
    if !state.charge_copies(1, &rest) {
        return GenTerm::assert(LogicalConstraint::T(it), rest);
    }
    let (a_guard, b_guard) = (Guard::of(a), Guard::of(b));
    let not_a = a_guard.negated();
    let then_live = state.possible(&rest, &[&a_guard, &b_guard]);
    let else_live = state.possible(&rest, &[&not_a]);
    if !then_live && !else_live {
        return failed(rest.bt);
    }
    let then_ = then_live.then(|| {
        state
            .assuming(&[&a_guard, &b_guard], |state| {
                let rest = prune_under(&rest, &mut state.path);
                lift_consequent((**b).clone(), rest, state)
            })
            .expect("preflight accepted the consequent")
    });
    let else_ = else_live.then(|| state.continuation(&rest, &[&not_a]));
    match (then_, else_) {
        (Some(then_), Some(else_)) => GenTerm::ite((**a).clone(), then_, else_),
        (Some(then_), None) => GenTerm::assert(LogicalConstraint::T((**a).clone()), then_),
        (None, Some(else_)) => {
            GenTerm::assert(LogicalConstraint::T(IndexTerm::not_((**a).clone())), else_)
        }
        (None, None) => unreachable!("at least one alternative survives"),
    }
}

type AssertArm = fn(&SymSet, LogicalConstraint, GenTerm, BaseType, &mut LiftState) -> GenTerm;

fn ext_walk(
    ext: &SymSet,
    gt: GenTerm,
    track_ext: bool,
    state: &mut LiftState,
    on_assert: AssertArm,
) -> GenTerm {
    let mark = state.path.mark();
    let (term, bt) = gt.into_parts();
    let out = match term {
        GenTermKind::Arbitrary | GenTermKind::Call { .. } | GenTermKind::Return(_) => {
            GenTerm::from_parts(term, bt)
        }
        GenTermKind::WriteChoice { label, value, rest } => GenTerm::write_choice(
            label,
            value,
            ext_walk(ext, *rest, track_ext, state, on_assert),
        ),
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => GenTerm::asgn(
            addr,
            ct,
            value,
            ext_walk(ext, *rest, track_ext, state, on_assert),
        ),
        GenTermKind::LetStar { var, value, rest } => {
            let value = ext_walk(ext, *value, track_ext, state, on_assert);
            if state.path_pruning {
                state.path.shadow(&var);
            }
            let rest = if track_ext && is_external(ext, &value) {
                let mut grown = ext.clone();
                grown.insert(var.clone());
                ext_walk(&grown, *rest, track_ext, state, on_assert)
            } else {
                ext_walk(ext, *rest, track_ext, state, on_assert)
            };
            GenTerm::let_star((var, value), rest)
        }
        GenTermKind::Assert { constraint, rest } => {
            if state.path_pruning
                && let LogicalConstraint::T(condition) = &constraint
                && !state.path.assume(condition)
            {
                failed(bt)
            } else {
                on_assert(ext, constraint, *rest, bt, state)
            }
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            if !state.path_pruning {
                let then_ = ext_walk(ext, *then_, track_ext, state, on_assert);
                let else_ = ext_walk(ext, *else_, track_ext, state, on_assert);
                return GenTerm::ite(cond, then_, else_);
            }
            let guard = Guard::of(&cond);
            let then_ = state.assuming(&[&guard], |state| {
                ext_walk(ext, *then_, track_ext, state, on_assert)
            });
            let else_ = state.assuming(&[&guard.negated()], |state| {
                ext_walk(ext, *else_, track_ext, state, on_assert)
            });
            ite_from_arms(cond, then_, else_).unwrap_or_else(|| failed(bt))
        }
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => {
            if !state.path_pruning {
                return GenTerm::map(
                    (var, var_bt, perm),
                    ext_walk(ext, *body, track_ext, state, on_assert),
                );
            }
            state.path.shadow(&var);
            let body_bt = body.bt.clone();
            let body = state
                .assuming(&[&Guard::of(&perm)], |state| {
                    ext_walk(ext, *body, track_ext, state, on_assert)
                })
                .unwrap_or_else(|| failed(body_bt));
            GenTerm::map((var, var_bt, perm), body)
        }
        GenTermKind::Scope { name, body } => {
            GenTerm::scope(name, ext_walk(ext, *body, track_ext, state, on_assert))
        }
    };
    state.path.rewind(mark);
    out
}

fn is_external(ext: &SymSet, gt: &GenTerm) -> bool {
    match &gt.term {
        GenTermKind::Arbitrary => false,
        GenTermKind::Return(it) => !ext.is_disjoint(&it.free_vars()),
        GenTermKind::Call { .. } => true,
        GenTermKind::Asgn { rest, .. } | GenTermKind::WriteChoice { rest, .. } => {
            is_external(ext, rest)
        }
        GenTermKind::LetStar { value, rest, .. } => {
            is_external(ext, value) || is_external(ext, rest)
        }
        GenTermKind::Assert { rest, .. } => is_external(ext, rest),
        GenTermKind::Ite { then_, else_, .. } => is_external(ext, then_) || is_external(ext, else_),
        GenTermKind::Map { body, .. } => is_external(ext, body),
        GenTermKind::Scope { body, .. } => is_external(ext, body),
    }
}

pub fn lift_disjunction(term: GenTerm, syms: &mut SymGen) -> GenTerm {
    lift_disjunction_bounded(term, syms, MAX_LIFT_GROWTH, false)
}

fn lift_disjunction_bounded(
    term: GenTerm,
    syms: &mut SymGen,
    budget: usize,
    path_pruning: bool,
) -> GenTerm {
    let mut state = LiftState {
        syms: Some(syms),
        budget,
        path_pruning,
        path: PathConditions::default(),
    };
    ext_walk(&SymSet::new(), term, true, &mut state, disjunction_assert)
}

fn disjunction_assert(
    ext: &SymSet,
    constraint: LogicalConstraint,
    rest: GenTerm,
    bt: BaseType,
    state: &mut LiftState,
) -> GenTerm {
    let it = match constraint {
        lc @ LogicalConstraint::Forall { .. } => {
            return GenTerm::assert(lc, ext_walk(ext, rest, true, state, disjunction_assert));
        }
        LogicalConstraint::T(it) => it,
    };
    let normalized = dnf(it.clone());
    if !matches!(normalized.term, Term::Binop(BinOp::Or, _, _)) {
        let rest = ext_walk(ext, rest, true, state, disjunction_assert);
        return GenTerm::assert(LogicalConstraint::T(normalized), rest);
    }
    let (mut alternatives, left): (Vec<_>, Vec<_>) = listify(normalized, BinOp::Or)
        .into_iter()
        .partition(|d| splits_apart(ext, d));
    if alternatives.is_empty() {
        if !state.path_pruning {
            drop(ext_walk(ext, rest.clone(), true, state, disjunction_assert));
        }
        return GenTerm::from_parts(
            GenTermKind::Assert {
                constraint: LogicalConstraint::T(it),
                rest: Box::new(rest),
            },
            bt,
        );
    }
    if let Some(residual) = left.into_iter().reduce(or2) {
        alternatives.push(residual);
    }
    let rest = ext_walk(ext, rest, true, state, disjunction_assert);
    if !state.charge_copies(alternatives.len() - 1, &rest) {
        return GenTerm::assert(LogicalConstraint::T(it), rest);
    }
    if !state.path_pruning {
        let alternatives = alternatives
            .into_iter()
            .map(|condition| GenTerm::assert(LogicalConstraint::T(condition), rest.clone()))
            .collect();
        return GenTerm::choose_bool(
            alternatives,
            state
                .syms
                .as_deref_mut()
                .expect("disjunction symbol supply"),
        );
    }
    let guards: Vec<Guard> = alternatives.iter().map(Guard::of).collect();
    let live: Vec<_> = alternatives
        .into_iter()
        .zip(guards)
        .filter(|(_, guard)| state.possible(&rest, &[guard]))
        .collect();
    if live.is_empty() {
        return failed(rest.bt);
    }
    let alternatives = live
        .into_iter()
        .map(|(d, guard)| {
            let rest = state.continuation(&rest, &[&guard]);
            GenTerm::assert(LogicalConstraint::T(d), rest)
        })
        .collect();
    let syms = state
        .syms
        .as_deref_mut()
        .expect("a disjunction split carries a real symbol supply");
    GenTerm::choose_bool(alternatives, syms)
}

fn splits_apart(ext: &SymSet, it: &IndexTerm) -> bool {
    if let Term::Binop(BinOp::Eq, l, r) = &it.term {
        if let Term::Sym(x) = &l.term
            && !ext.contains(x)
        {
            return true;
        }
        if let Term::Sym(x) = &r.term
            && !ext.contains(x)
        {
            return true;
        }
    }
    ext.is_disjoint(&it.free_vars())
}

fn dnf(it: IndexTerm) -> IndexTerm {
    let (term, bt) = it.into_parts();
    let term = match term {
        Term::Unop(UnOp::Not, inner) => push_not(*inner, dnf),
        Term::Binop(BinOp::And, e1, e2) => {
            distribute(dnf(*e1), dnf(*e2), BinOp::And, BinOp::Or, dnf)
        }
        term => term,
    };
    IndexTerm { term, bt }
}

pub(super) fn cnf(it: IndexTerm) -> IndexTerm {
    let (term, bt) = it.into_parts();
    let term = match term {
        Term::Unop(UnOp::Not, inner) => push_not(*inner, cnf),
        Term::Binop(BinOp::Or, e1, e2) => {
            distribute(cnf(*e1), cnf(*e2), BinOp::Or, BinOp::And, cnf)
        }
        term => term,
    };
    IndexTerm { term, bt }
}

fn push_not(inner: IndexTerm, norm: fn(IndexTerm) -> IndexTerm) -> Term {
    let (term, bt) = norm(inner).into_parts();
    match term {
        Term::Unop(UnOp::Not, e) => e.into_parts().0,
        Term::Binop(BinOp::Lt, e1, e2) => Term::Binop(BinOp::Le, e2, e1),
        Term::Binop(BinOp::Le, e1, e2) => Term::Binop(BinOp::Lt, e2, e1),
        Term::Binop(BinOp::And, e1, e2) => Term::Binop(
            BinOp::Or,
            Box::new(norm(IndexTerm {
                term: Term::Unop(UnOp::Not, e1),
                bt: bt.clone(),
            })),
            Box::new(norm(IndexTerm {
                term: Term::Unop(UnOp::Not, e2),
                bt,
            })),
        ),
        Term::Binop(BinOp::Or, e1, e2) => Term::Binop(
            BinOp::And,
            Box::new(norm(IndexTerm {
                term: Term::Unop(UnOp::Not, e1),
                bt: bt.clone(),
            })),
            Box::new(norm(IndexTerm {
                term: Term::Unop(UnOp::Not, e2),
                bt,
            })),
        ),
        term => Term::Unop(UnOp::Not, Box::new(IndexTerm { term, bt })),
    }
}

fn distribute(
    l: IndexTerm,
    r: IndexTerm,
    outer: BinOp,
    inner: BinOp,
    norm: fn(IndexTerm) -> IndexTerm,
) -> Term {
    let is_inner = |it: &IndexTerm| matches!(&it.term, Term::Binop(op, _, _) if *op == inner);
    let split = |it: IndexTerm| {
        let (term, bt) = it.into_parts();
        let Term::Binop(_, x, y) = term else {
            unreachable!("is_inner matched a Binop");
        };
        (x, y, bt)
    };
    let (a, x, y, bt) = if is_inner(&r) {
        let (x, y, bt) = split(r);
        (l, x, y, bt)
    } else if is_inner(&l) {
        let (x, y, bt) = split(l);
        (r, x, y, bt)
    } else {
        return Term::Binop(outer, Box::new(l), Box::new(r));
    };
    Term::Binop(
        inner,
        Box::new(norm(IndexTerm {
            term: Term::Binop(outer, Box::new(a.clone()), x),
            bt: bt.clone(),
        })),
        Box::new(norm(IndexTerm {
            term: Term::Binop(outer, Box::new(a), y),
            bt,
        })),
    )
}

pub(super) fn listify(it: IndexTerm, op: BinOp) -> Vec<IndexTerm> {
    fn walk(it: IndexTerm, op: BinOp, out: &mut Vec<IndexTerm>) {
        if matches!(&it.term, Term::Binop(head, _, _) if *head == op) {
            let Term::Binop(_, e1, e2) = it.into_parts().0 else {
                unreachable!("just matched a Binop");
            };
            walk(*e1, op, out);
            walk(*e2, op, out);
        } else {
            out.push(it);
        }
    }
    let mut out = Vec::new();
    walk(it, op, &mut out);
    out
}

fn or2(l: IndexTerm, r: IndexTerm) -> IndexTerm {
    IndexTerm {
        term: Term::Binop(BinOp::Or, Box::new(l), Box::new(r)),
        bt: BaseType::Bool,
    }
}

fn ite_term(cond: IndexTerm, then_: IndexTerm, else_: IndexTerm) -> IndexTerm {
    let bt = then_.bt.clone();
    IndexTerm {
        term: Term::Ite(Box::new(cond), Box::new(then_), Box::new(else_)),
        bt,
    }
}

pub fn lift_ite(term: GenTerm) -> GenTerm {
    lift_ite_bounded(term, MAX_LIFT_GROWTH, false)
}

fn lift_ite_bounded(term: GenTerm, budget: usize, path_pruning: bool) -> GenTerm {
    let mut state = LiftState {
        syms: None,
        budget,
        path_pruning,
        path: PathConditions::default(),
    };
    ext_walk(&SymSet::new(), term, true, &mut state, ite_assert)
}

fn ite_assert(
    ext: &SymSet,
    constraint: LogicalConstraint,
    rest: GenTerm,
    _bt: BaseType,
    state: &mut LiftState,
) -> GenTerm {
    let rest = ext_walk(ext, rest, true, state, ite_assert);
    let transformed = transform_lc(constraint.clone());
    let LogicalConstraint::T(it) = &transformed else {
        return GenTerm::assert(constraint, rest);
    };
    let Term::Ite(cond, then_, else_) = &it.term else {
        return GenTerm::assert(constraint, rest);
    };
    if !ext.is_disjoint(&cond.free_vars()) {
        return GenTerm::assert(constraint, rest);
    }
    if !state.charge_copies(1, &rest) {
        return GenTerm::assert(constraint, rest);
    }
    if !state.path_pruning {
        let LogicalConstraint::T(it) = transformed else {
            unreachable!("matched a term-level conditional");
        };
        let Term::Ite(cond, then_, else_) = it.into_parts().0 else {
            unreachable!("matched a term-level conditional");
        };
        return GenTerm::ite(
            *cond,
            GenTerm::assert(LogicalConstraint::T(*then_), rest.clone()),
            GenTerm::assert(LogicalConstraint::T(*else_), rest),
        );
    }
    let cond_guard = Guard::of(cond);
    let not_cond = cond_guard.negated();
    let (then_guard, else_guard) = (Guard::of(then_), Guard::of(else_));
    let then_live = state.possible(&rest, &[&cond_guard, &then_guard]);
    let else_live = state.possible(&rest, &[&not_cond, &else_guard]);
    if !then_live && !else_live {
        return failed(rest.bt);
    }
    let then_ = then_live.then(|| {
        GenTerm::assert(
            LogicalConstraint::T((**then_).clone()),
            state.continuation(&rest, &[&cond_guard, &then_guard]),
        )
    });
    let else_ = else_live.then(|| {
        GenTerm::assert(
            LogicalConstraint::T((**else_).clone()),
            state.continuation(&rest, &[&not_cond, &else_guard]),
        )
    });
    match (then_, else_) {
        (Some(then_), Some(else_)) => GenTerm::ite((**cond).clone(), then_, else_),
        (Some(then_), None) => GenTerm::assert(LogicalConstraint::T((**cond).clone()), then_),
        (None, Some(else_)) => GenTerm::assert(
            LogicalConstraint::T(IndexTerm::not_((**cond).clone())),
            else_,
        ),
        (None, None) => unreachable!("at least one alternative survives"),
    }
}

fn transform_lc(lc: LogicalConstraint) -> LogicalConstraint {
    match lc {
        LogicalConstraint::T(it) => LogicalConstraint::T(transform_it(it)),
        lc @ LogicalConstraint::Forall { .. } => lc,
    }
}

fn transform_it(it: IndexTerm) -> IndexTerm {
    map_term_post(it, hoist_ite)
}

fn hoist_ite(it: IndexTerm) -> IndexTerm {
    let (term, bt) = it.into_parts();
    let is_ite = |it: &IndexTerm| matches!(&it.term, Term::Ite(_, _, _));
    let split_ite = |it: Box<IndexTerm>| {
        let Term::Ite(c, t, e) = it.into_parts().0 else {
            unreachable!("is_ite matched an ite");
        };
        (c, t, e)
    };
    match term {
        Term::Unop(op, inner) if is_ite(&inner) => {
            let (c, t, e) = split_ite(inner);
            let wrap = |arm: Box<IndexTerm>| IndexTerm {
                term: Term::Unop(op, arm),
                bt: bt.clone(),
            };
            ite_term(*c, wrap(t), wrap(e))
        }
        Term::Binop(op, l, r) if is_ite(&l) => {
            let (c, t, e) = split_ite(l);
            let wrap = |arm: Box<IndexTerm>| IndexTerm {
                term: Term::Binop(op, arm, r.clone()),
                bt: bt.clone(),
            };
            ite_term(*c, wrap(t), wrap(e))
        }
        Term::Binop(op, l, r) if is_ite(&r) => {
            let (c, t, e) = split_ite(r);
            let wrap = |arm: Box<IndexTerm>| IndexTerm {
                term: Term::Binop(op, l.clone(), arm),
                bt: bt.clone(),
            };
            ite_term(*c, wrap(t), wrap(e))
        }
        Term::EachI {
            start,
            var,
            end,
            body,
        } if matches!(&body.term, Term::Ite(c, _, _) if !c.free_vars().contains(&var.0)) => {
            let (c, t, e) = split_ite(body);
            let wrap = |arm: Box<IndexTerm>| IndexTerm {
                term: Term::EachI {
                    start,
                    var: var.clone(),
                    end,
                    body: arm,
                },
                bt: BaseType::Bool,
            };
            ite_term(*c, wrap(t), wrap(e))
        }
        Term::Cast(cast_bt, inner) if is_ite(&inner) => {
            let (c, t, e) = split_ite(inner);
            ite_term(
                *c,
                IndexTerm::cast(cast_bt.clone(), *t),
                IndexTerm::cast(cast_bt, *e),
            )
        }
        term => IndexTerm { term, bt },
    }
}

fn map_term_post(it: IndexTerm, f: fn(IndexTerm) -> IndexTerm) -> IndexTerm {
    let (term, bt) = it.into_parts();
    let go = |it: Box<IndexTerm>| Box::new(map_term_post(*it, f));
    let go_all = |its: Vec<IndexTerm>| -> Vec<IndexTerm> {
        its.into_iter().map(|it| map_term_post(it, f)).collect()
    };
    let go_fields = |fields: Vec<(crate::gen_dsl::lang::id::Id, IndexTerm)>| {
        fields
            .into_iter()
            .map(|(id, it)| (id, map_term_post(it, f)))
            .collect::<Vec<_>>()
    };
    let term = match term {
        Term::Const(_)
        | Term::Sym(_)
        | Term::SizeOf(_)
        | Term::OffsetOf(_, _)
        | Term::Nil(_)
        | Term::CnNone(_) => term,
        Term::Unop(op, a) => Term::Unop(op, go(a)),
        Term::Binop(op, a, b) => Term::Binop(op, go(a), go(b)),
        Term::Ite(c, t, e) => Term::Ite(go(c), go(t), go(e)),
        Term::EachI {
            start,
            var,
            end,
            body,
        } => Term::EachI {
            start,
            var,
            end,
            body: go(body),
        },
        Term::Tuple(its) => Term::Tuple(go_all(its)),
        Term::NthTuple(i, a) => Term::NthTuple(i, go(a)),
        Term::Struct(tag, fields) => Term::Struct(tag, go_fields(fields)),
        Term::StructMember(a, member) => Term::StructMember(go(a), member),
        Term::StructUpdate {
            target,
            member,
            value,
        } => Term::StructUpdate {
            target: go(target),
            member,
            value: go(value),
        },
        Term::Record(fields) => Term::Record(go_fields(fields)),
        Term::RecordMember(a, member) => Term::RecordMember(go(a), member),
        Term::RecordUpdate {
            target,
            member,
            value,
        } => Term::RecordUpdate {
            target: go(target),
            member,
            value: go(value),
        },
        Term::Constructor(tag, fields) => Term::Constructor(tag, go_fields(fields)),
        Term::MemberShift(a, tag, member) => Term::MemberShift(go(a), tag, member),
        Term::ArrayShift { base, ct, index } => Term::ArrayShift {
            base: go(base),
            ct,
            index: go(index),
        },
        Term::CopyAllocId { addr, loc } => Term::CopyAllocId {
            addr: go(addr),
            loc: go(loc),
        },
        Term::HasAllocId(a) => Term::HasAllocId(go(a)),
        Term::Cons(h, t) => Term::Cons(go(h), go(t)),
        Term::Head(a) => Term::Head(go(a)),
        Term::Tail(a) => Term::Tail(go(a)),
        Term::Representable(ct, a) => Term::Representable(ct, go(a)),
        Term::Good(ct, a) => Term::Good(ct, go(a)),
        Term::Aligned { t, align } => Term::Aligned {
            t: go(t),
            align: go(align),
        },
        Term::WrapI(ity, a) => Term::WrapI(ity, go(a)),
        Term::MapConst(mbt, a) => Term::MapConst(mbt, go(a)),
        Term::MapSet(m, k, v) => Term::MapSet(go(m), go(k), go(v)),
        Term::MapGet(m, k) => Term::MapGet(go(m), go(k)),
        Term::MapDef(var, a) => Term::MapDef(var, go(a)),
        Term::Apply(fsym, args) => Term::Apply(fsym, go_all(args)),
        Term::Let((x, bound), body) => Term::Let((x, go(bound)), go(body)),
        Term::Match(scrutinee, arms) => Term::Match(
            go(scrutinee),
            arms.into_iter()
                .map(|(pat, it)| (pat, map_term_post(it, f)))
                .collect(),
        ),
        Term::Cast(cbt, a) => Term::Cast(cbt, go(a)),
        Term::CnSome(a) => Term::CnSome(go(a)),
        Term::IsSome(a) => Term::IsSome(go(a)),
        Term::GetOpt(a) => Term::GetOpt(go(a)),
    };
    f(IndexTerm { term, bt })
}

pub fn lift_let(term: GenTerm) -> GenTerm {
    let out: Result<GenTerm, std::convert::Infallible> = term.map_pre(&mut |node| {
        let (term, bt) = node.into_parts();
        match term {
            GenTermKind::Assert {
                constraint: LogicalConstraint::T(it),
                rest,
            } if matches!(&it.term, Term::Let(_, _)) => {
                let Term::Let((x, inner), body) = it.into_parts().0 else {
                    unreachable!("guard matched a let");
                };
                debug_assert!(
                    !rest.free_vars().contains(&x),
                    "a constraint-local binder cannot occur free in the continuation"
                );
                Ok(GenTerm::let_star(
                    (x, GenTerm::return_(*inner)),
                    GenTerm::assert(LogicalConstraint::T(*body), *rest),
                ))
            }
            term => Ok(GenTerm::from_parts(term, bt)),
        }
    });
    out.unwrap()
}

pub fn lift_conjunction(term: GenTerm) -> GenTerm {
    let out: Result<GenTerm, std::convert::Infallible> = term.map_pre(&mut |node| {
        let (term, bt) = node.into_parts();
        match term {
            GenTermKind::Assert {
                constraint: LogicalConstraint::T(it),
                rest,
            } => Ok(listify(cnf(it), BinOp::And)
                .into_iter()
                .fold(*rest, |acc, c| {
                    GenTerm::assert(LogicalConstraint::T(c), acc)
                })),
            term => Ok(GenTerm::from_parts(term, bt)),
        }
    });
    out.unwrap()
}
