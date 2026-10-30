use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::index_term::{BinOp, Const, IndexTerm, Term, UnOp};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::sym::{Sym, SymSet};

use super::flip_ifs::selector_binder;
use super::lift::term_bytes;
use super::reorder::{Stmt, rebuild, stmt_binds, stmts_of};

pub const MAX_GUARDED_GROWTH: usize = 1 << 20;

pub fn specialize_guarded_choices(iargs: &SymSet, term: GenTerm) -> GenTerm {
    specialize_guarded_choices_bounded(iargs, term, MAX_GUARDED_GROWTH)
}

pub fn specialize_guarded_choices_bounded(iargs: &SymSet, term: GenTerm, budget: usize) -> GenTerm {
    let mut budget = budget;
    transform(iargs, term, &mut budget)
}

fn transform(entry: &SymSet, node: GenTerm, budget: &mut usize) -> GenTerm {
    let mut bound = entry.clone();
    let (stmts, tail) = stmts_of(node);
    let stmts = stmts
        .into_iter()
        .map(|stmt| match stmt {
            Stmt::LetStar { var, value } => {
                let value = transform(&bound, value, budget);
                bound.insert(var.clone());
                Stmt::LetStar { var, value }
            }
            other => other,
        })
        .collect::<Vec<_>>();

    let (term, bt) = tail.into_parts();
    let tail = match term {
        term @ (GenTermKind::Arbitrary | GenTermKind::Call { .. } | GenTermKind::Return(_)) => {
            GenTerm::from_parts(term, bt)
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            let then_ = transform(&bound, *then_, budget);
            let else_ = transform(&bound, *else_, budget);
            GenTerm::rebuild_ite(cond, then_, else_)
        }
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => {
            let mut inner = bound.clone();
            inner.insert(var.clone());
            let body = transform(&inner, *body, budget);
            GenTerm::map((var, var_bt, perm), body)
        }
        GenTermKind::Scope { name, body } => GenTerm::scope(name, transform(&bound, *body, budget)),
        GenTermKind::WriteChoice { .. }
        | GenTermKind::Asgn { .. }
        | GenTermKind::LetStar { .. }
        | GenTermKind::Assert { .. } => unreachable!("a statement is never a spine's tail"),
    };
    decide(entry, stmts, tail, budget)
}

fn decide(entry: &SymSet, stmts: Vec<Stmt>, tail: GenTerm, budget: &mut usize) -> GenTerm {
    let Some(at) = selector_binder(&stmts, &tail) else {
        return rebuild(stmts, tail);
    };
    let GenTermKind::Ite { then_, else_, .. } = &tail.term else {
        unreachable!("a selector binder is found only for an `ite` tail");
    };
    let admissible = admissible(entry, &stmts, at);
    let p = entry_guard(then_, &admissible);
    let q = entry_guard(else_, &admissible);
    if p.is_none() && q.is_none() {
        return rebuild(stmts, tail);
    }

    let mut before = stmts;
    let after = before.split_off(at + 1);
    let Some(Stmt::LetStar { var, value }) = before.pop() else {
        unreachable!("the selector binder is a `let*` at `at`");
    };
    let continuation = rebuild(after, tail);
    let extra = usize::from(p.is_some()) + usize::from(q.is_some());
    let cost = extra.saturating_mul(term_bytes(&continuation));
    if cost > *budget {
        tracing::debug!(
            selector = ?var,
            cost,
            budget = *budget,
            "specialize_guarded_choices: the continuation's copies exceed the growth budget; \
             the selector keeps its draw"
        );
        return rebuild(before, GenTerm::let_star((var, value), continuation));
    }
    *budget -= cost;
    rebuild(before, split(p, q, &var, value, continuation))
}

fn admissible(entry: &SymSet, stmts: &[Stmt], at: usize) -> SymSet {
    let mut admissible = entry.clone();
    for stmt in &stmts[..at] {
        if let Some(var) = stmt_binds(stmt) {
            admissible.insert(var.clone());
        }
    }
    for stmt in &stmts[at..] {
        if let Some(var) = stmt_binds(stmt) {
            admissible.remove(var);
        }
    }
    admissible
}

fn entry_guard(arm: &GenTerm, admissible: &SymSet) -> Option<IndexTerm> {
    let mut guards = Vec::new();
    let mut shadowed = SymSet::new();
    let mut node = arm;
    loop {
        node = match &node.term {
            GenTermKind::Assert {
                constraint: LogicalConstraint::T(it),
                rest,
            } => {
                let reads_before_draw = it
                    .free_vars()
                    .iter()
                    .all(|sym| admissible.contains(sym) && !shadowed.contains(sym));
                if reads_before_draw && total_scalar(it) {
                    guards.push(it.clone());
                }
                rest
            }
            GenTermKind::Assert {
                constraint: LogicalConstraint::Forall { .. },
                rest,
            } => rest,
            GenTermKind::LetStar { var, rest, .. } => {
                shadowed.insert(var.clone());
                rest
            }
            GenTermKind::WriteChoice { rest, .. } | GenTermKind::Asgn { rest, .. } => rest,
            GenTermKind::Scope { body, .. } => body,
            GenTermKind::Arbitrary
            | GenTermKind::Call { .. }
            | GenTermKind::Return(_)
            | GenTermKind::Ite { .. }
            | GenTermKind::Map { .. } => break,
        };
    }
    guards.into_iter().reduce(IndexTerm::and2)
}

fn total_scalar(it: &IndexTerm) -> bool {
    it.bt == BaseType::Bool && total(it)
}

fn scalar(bt: &BaseType) -> bool {
    match bt {
        BaseType::Bool | BaseType::Integer | BaseType::Loc(_) => true,
        BaseType::Bits(_, width) => *width != 0,
        _ => false,
    }
}

fn numeric(bt: &BaseType) -> bool {
    match bt {
        BaseType::Integer => true,
        BaseType::Bits(_, width) => *width != 0,
        _ => false,
    }
}

fn total(it: &IndexTerm) -> bool {
    if !scalar(&it.bt) {
        return false;
    }
    match &it.term {
        Term::Const(c) => matches!(
            c,
            Const::Bool(_) | Const::Z(_) | Const::Bits(..) | Const::Null | Const::Pointer { .. }
        ),
        Term::Sym(_) => true,
        Term::Unop(op, e) => {
            total(e)
                && match op {
                    UnOp::Not => it.bt == BaseType::Bool && e.bt == BaseType::Bool,
                    UnOp::Negate => e.bt == it.bt && numeric(&e.bt),
                    UnOp::BwCompl => e.bt == it.bt && matches!(e.bt, BaseType::Bits(..)),
                    UnOp::BwClz | UnOp::BwCtz | UnOp::BwFfs | UnOp::BwFls => false,
                }
        }
        Term::Binop(op, l, r) => {
            total(l)
                && total(r)
                && match op {
                    BinOp::And | BinOp::Or | BinOp::Implies => {
                        it.bt == BaseType::Bool && l.bt == BaseType::Bool && r.bt == BaseType::Bool
                    }
                    BinOp::Eq => it.bt == BaseType::Bool && l.bt == r.bt,
                    BinOp::Lt | BinOp::Le => {
                        it.bt == BaseType::Bool && l.bt == r.bt && numeric(&l.bt)
                    }
                    BinOp::LtPointer | BinOp::LePointer => {
                        it.bt == BaseType::Bool
                            && matches!((&l.bt, &r.bt), (BaseType::Loc(_), BaseType::Loc(_)))
                    }
                    BinOp::Add
                    | BinOp::Sub
                    | BinOp::Mul
                    | BinOp::Min
                    | BinOp::Max
                    | BinOp::BwAnd
                    | BinOp::BwOr
                    | BinOp::BwXor => l.bt == r.bt && it.bt == l.bt && numeric(&l.bt),
                    _ => false,
                }
        }
        Term::Cast(target, e) => {
            total(e)
                && *target == it.bt
                && matches!(
                    (&e.bt, target),
                    (BaseType::Bits(..), BaseType::Bits(..))
                        | (BaseType::Bits(..), BaseType::Integer)
                        | (BaseType::Integer, BaseType::Bits(..))
                        | (BaseType::Loc(_), BaseType::Bits(..))
                        | (BaseType::Bits(..), BaseType::Loc(_))
                )
        }
        _ => false,
    }
}

fn split(
    p: Option<IndexTerm>,
    q: Option<IndexTerm>,
    var: &Sym,
    draw: GenTerm,
    k: GenTerm,
) -> GenTerm {
    let bind = |value: GenTerm, k: GenTerm| GenTerm::let_star((var.clone(), value), k);
    let forced = |value: bool| GenTerm::return_(IndexTerm::boolean(value));
    match (p, q) {
        (Some(p), Some(q)) => GenTerm::ite(
            p,
            GenTerm::ite(q, bind(draw, k.clone()), bind(forced(true), k.clone())),
            bind(forced(false), k),
        ),
        (Some(p), None) => GenTerm::ite(p, bind(draw, k.clone()), bind(forced(false), k)),
        (None, Some(q)) => GenTerm::ite(q, bind(draw, k.clone()), bind(forced(true), k)),
        (None, None) => unreachable!("a selector with no known guard is not decided"),
    }
}
