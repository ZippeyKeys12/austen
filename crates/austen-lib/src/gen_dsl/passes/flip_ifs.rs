use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::index_term::{BinOp, IndexTerm, Term};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::sym::{SymGen, SymSet};

use super::lift::{cnf, listify};
use super::reorder::{Stmt, rebuild, stmt_binds, stmt_reads, stmts_of};

fn is_bool_sym(it: &IndexTerm, selectors: &SymSet) -> bool {
    it.bt == BaseType::Bool && it.as_sym().is_some_and(|sym| selectors.contains(sym))
}

pub(super) fn is_selector_draw(value: &GenTerm) -> bool {
    value.bt == BaseType::Bool && matches!(value.term, GenTermKind::Arbitrary)
}

pub(super) fn selector_binder(stmts: &[Stmt], tail: &GenTerm) -> Option<usize> {
    let GenTermKind::Ite { cond, .. } = &tail.term else {
        return None;
    };
    if cond.bt != BaseType::Bool {
        return None;
    }
    let b = cond.as_sym()?;
    let at = stmts.iter().rposition(|stmt| stmt_binds(stmt) == Some(b))?;
    match &stmts[at] {
        Stmt::LetStar { value, .. } if is_selector_draw(value) => Some(at),
        _ => None,
    }
}

fn conjuncts(it: IndexTerm, out: &mut Vec<IndexTerm>) {
    for part in listify(it, BinOp::And) {
        let normal = cnf(part);
        if matches!(normal.term, Term::Binop(BinOp::And, _, _)) {
            conjuncts(normal, out);
        } else {
            out.push(normal);
        }
    }
}

fn transform(iargs: &SymSet, selectors: &SymSet, node: GenTerm, syms: &mut SymGen) -> GenTerm {
    let mut selectors = selectors.clone();
    let (stmts, tail) = stmts_of(node);
    let mut stmts = stmts
        .into_iter()
        .map(|stmt| match stmt {
            Stmt::LetStar { var, value } => {
                let selector = is_selector_draw(&value);
                let value = transform(iargs, &selectors, value, syms);
                if selector {
                    selectors.insert(var.clone());
                } else {
                    selectors.remove(&var);
                }
                Stmt::LetStar { var, value }
            }
            other => other,
        })
        .collect::<Vec<_>>();

    let (term, bt) = tail.into_parts();
    let last = match term {
        term @ (GenTermKind::Arbitrary | GenTermKind::Call { .. } | GenTermKind::Return(_)) => {
            GenTerm::from_parts(term, bt)
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            let then_ = transform(iargs, &selectors, *then_, syms);
            let else_ = transform(iargs, &selectors, *else_, syms);
            let mut parts = Vec::new();
            conjuncts(cond.clone(), &mut parts);
            let (steered, local): (Vec<_>, Vec<_>) = parts.into_iter().partition(|part| {
                part.free_vars().is_subset(iargs) || is_bool_sym(part, &selectors)
            });
            if local.is_empty() {
                return rebuild(stmts, GenTerm::rebuild_ite(cond, then_, else_));
            }
            let seeds: SymSet = local
                .iter()
                .flat_map(IndexTerm::free_vars)
                .filter(|sym| {
                    !iargs.contains(sym) && stmts.iter().any(|stmt| stmt_binds(stmt) == Some(sym))
                })
                .collect();
            let (kept, pulled) = split_dependencies(&stmts, seeds);
            stmts = kept;
            let coin = syms.fresh("choose");
            let flip = IndexTerm::sym(coin.clone(), BaseType::Bool);
            let test = match steered.into_iter().reduce(IndexTerm::and2) {
                Some(steered) => IndexTerm::and2(steered, flip),
                None => flip,
            };
            let then_ = rebuild(
                pulled.clone(),
                local.into_iter().rev().fold(then_, |acc, part| {
                    GenTerm::assert(LogicalConstraint::T(part), acc)
                }),
            );
            let else_ = rebuild(
                pulled,
                GenTerm::assert(LogicalConstraint::T(IndexTerm::not_(cond)), else_),
            );
            GenTerm::let_star(
                (coin, GenTerm::arbitrary(BaseType::Bool)),
                GenTerm::ite(test, then_, else_),
            )
        }
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => {
            selectors.remove(&var);
            let body = transform(iargs, &selectors, *body, syms);
            GenTerm::map((var, var_bt, perm), body)
        }
        GenTermKind::Scope { name, body } => {
            GenTerm::scope(name, transform(iargs, &selectors, *body, syms))
        }
        GenTermKind::WriteChoice { .. }
        | GenTermKind::Asgn { .. }
        | GenTermKind::LetStar { .. }
        | GenTermKind::Assert { .. } => unreachable!("a statement is never a spine's tail"),
    };
    rebuild(stmts, last)
}

pub(super) fn split_dependencies(stmts: &[Stmt], seeds: SymSet) -> (Vec<Stmt>, Vec<Stmt>) {
    let mut pulled_binds = seeds;
    let mut pulled_reads = SymSet::new();
    let mut kept = Vec::new();
    let mut pulled = Vec::new();
    for stmt in stmts {
        let binds = stmt_binds(stmt);
        let reads = stmt_reads(stmt);
        let moves = binds
            .is_some_and(|var| pulled_binds.contains(var) || pulled_reads.contains(var))
            || !reads.is_disjoint(&pulled_binds);
        if moves {
            if let Some(var) = binds {
                pulled_binds.insert(var.clone());
            }
            pulled_reads.extend(reads);
            pulled.push(stmt.clone());
        } else {
            kept.push(stmt.clone());
        }
    }
    (kept, pulled)
}

pub fn flip_ifs(iargs: &SymSet, term: GenTerm, syms: &mut SymGen) -> GenTerm {
    transform(iargs, &SymSet::new(), term, syms)
}
