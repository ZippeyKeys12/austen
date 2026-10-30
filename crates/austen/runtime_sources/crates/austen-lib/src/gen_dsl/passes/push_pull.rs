use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::index_term::{BinOp, IndexTerm, Term};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::subst::Substitution;
use crate::gen_dsl::lang::sym::{Sym, SymGen, SymSet};

use super::choice_effects::ChoiceEffects;
use super::flip_ifs::{selector_binder, split_dependencies};
use super::lift::term_bytes;
use super::reorder::{Stmt, rebuild, stmt_binds, stmt_reads, stmts_of};

const MAX_ROUNDS: usize = 10_000;

pub const MAX_PUSH_GROWTH: usize = 1 << 22;

fn pull(term: GenTerm, syms: &mut SymGen, effects: &ChoiceEffects) -> GenTerm {
    let out: Result<GenTerm, std::convert::Infallible> = term.map_post(&mut |node| {
        let GenTermKind::LetStar { var, value, rest } = node.term else {
            return Ok(node);
        };
        let (value_term, value_bt) = value.into_parts();

        Ok(match value_term {
            GenTermKind::Asgn {
                addr,
                ct,
                value,
                rest: inner,
            } => GenTerm::asgn(addr, ct, value, GenTerm::let_star((var, *inner), *rest)),

            GenTermKind::LetStar {
                var: inner_var,
                value: inner_value,
                rest: inner_rest,
            } => {
                let fresh = syms.fresh("flat");
                let rename = Substitution::from([(
                    inner_var,
                    IndexTerm::sym(fresh.clone(), inner_value.bt.clone()),
                )]);
                let renamed = (*inner_rest).subst(&rename, syms);
                GenTerm::let_star(
                    (fresh, *inner_value),
                    GenTerm::let_star((var, renamed), *rest),
                )
            }

            GenTermKind::Assert {
                constraint,
                rest: inner,
            } => GenTerm::assert(constraint, GenTerm::let_star((var, *inner), *rest)),

            GenTermKind::Ite { cond, then_, else_ } => GenTerm::ite(
                cond,
                GenTerm::let_star((var.clone(), *then_), (*rest).clone()),
                GenTerm::let_star((var, *else_), *rest),
            ),

            GenTermKind::Scope { body, .. } if !effects.may_touch_tree(&body) => {
                GenTerm::let_star((var, *body), *rest)
            }

            value_term => {
                GenTerm::let_star((var, GenTerm::from_parts(value_term, value_bt)), *rest)
            }
        })
    });
    out.unwrap()
}

fn push(node: GenTerm, budget: &mut usize) -> GenTerm {
    let (stmts, tail) = stmts_of(node);
    let (stmts, tail) = match selector_binder(&stmts, &tail) {
        Some(at) => push_at(stmts, tail, at, budget),
        None => (stmts, tail),
    };
    let stmts = stmts
        .into_iter()
        .map(|stmt| match stmt {
            Stmt::LetStar { var, value } => Stmt::LetStar {
                var,
                value: push(value, budget),
            },
            other => other,
        })
        .collect();
    let (kind, bt) = tail.into_parts();
    let tail = match kind {
        GenTermKind::Ite { cond, then_, else_ } => {
            GenTerm::ite(cond, push(*then_, budget), push(*else_, budget))
        }
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => GenTerm::map((var, var_bt, perm), push(*body, budget)),
        GenTermKind::Scope { name, body } => GenTerm::scope(name, push(*body, budget)),
        kind => GenTerm::from_parts(kind, bt),
    };
    rebuild(stmts, tail)
}

fn pinned_in(arm: &GenTerm) -> SymSet {
    fn pins(it: &IndexTerm) -> Option<&Sym> {
        match &it.term {
            Term::Cast(_, inner) => inner.as_sym(),
            _ => it.as_sym(),
        }
    }
    let mut pinned = SymSet::new();
    let _: Result<GenTerm, std::convert::Infallible> = arm.clone().map_post(&mut |node| {
        if let GenTermKind::Assert {
            constraint: LogicalConstraint::T(it),
            ..
        } = &node.term
            && let Term::Binop(BinOp::Eq, l, r) = &it.term
        {
            pinned.extend(pins(l).into_iter().chain(pins(r)).cloned());
        }
        Ok(node)
    });
    pinned
}

fn movable(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::LetStar { value, .. } => !matches!(
            value.term,
            GenTermKind::Call { .. } | GenTermKind::Map { .. } | GenTermKind::Scope { .. }
        ),
        Stmt::Assert(constraint) => !lift_splits(constraint),
        Stmt::Asgn { .. } | Stmt::WriteChoice { .. } => true,
    }
}

fn lift_splits(constraint: &LogicalConstraint) -> bool {
    let LogicalConstraint::T(it) = constraint else {
        return false;
    };
    let mut stack = vec![it];
    while let Some(it) = stack.pop() {
        match &it.term {
            Term::Binop(BinOp::Or | BinOp::Implies, _, _) | Term::Ite(..) => return true,
            Term::Binop(_, l, r) => {
                stack.push(l);
                stack.push(r);
            }
            Term::Unop(_, x) => stack.push(x),
            _ => {}
        }
    }
    false
}

fn push_at(stmts: Vec<Stmt>, tail: GenTerm, at: usize, budget: &mut usize) -> (Vec<Stmt>, GenTerm) {
    let GenTermKind::Ite { cond, then_, else_ } = &tail.term else {
        return (stmts, tail);
    };
    let mut prefix = stmts;
    let rest = prefix.split_off(at);
    let prefix_binds: SymSet = prefix.iter().filter_map(stmt_binds).cloned().collect();

    let mut needed = pinned_in(then_);
    needed.extend(pinned_in(else_));
    needed.retain(|sym| prefix_binds.contains(sym));
    if needed.is_empty() {
        prefix.extend(rest);
        return (prefix, tail);
    }

    let mut seeds = needed;
    let (kept, moved) = loop {
        let (kept, moved) = split_dependencies(&prefix, seeds.clone());
        let mut stay: SymSet = moved
            .iter()
            .filter(|stmt| !movable(stmt))
            .flat_map(|stmt| {
                stmt_binds(stmt)
                    .cloned()
                    .into_iter()
                    .chain(stmt_reads(stmt))
            })
            .collect();
        if stay.is_empty() {
            break (kept, moved);
        }
        loop {
            let mut grew = false;
            for stmt in &prefix {
                if stmt_binds(stmt).is_some_and(|var| stay.contains(var)) {
                    for read in stmt_reads(stmt) {
                        if prefix_binds.contains(&read) && stay.insert(read) {
                            grew = true;
                        }
                    }
                }
            }
            if !grew {
                break;
            }
        }
        let before = seeds.len();
        seeds.retain(|sym| !stay.contains(sym));
        if seeds.is_empty() || seeds.len() == before {
            prefix.extend(rest);
            return (prefix, tail);
        }
    };

    let mut fixed = cond.free_vars();
    for stmt in &rest {
        fixed.extend(stmt_reads(stmt));
        if let Some(var) = stmt_binds(stmt) {
            fixed.insert(var.clone());
        }
    }
    if moved
        .iter()
        .filter_map(stmt_binds)
        .any(|var| fixed.contains(var))
    {
        tracing::debug!(
            selector = ?cond,
            "push_pull: a statement between the selector's draw and its decision reads or rebinds the prefix; the prefix stays above"
        );
        let mut stmts = kept;
        stmts.extend(moved);
        stmts.extend(rest);
        return (stmts, tail);
    }

    let cost = term_bytes(&rebuild(
        moved.clone(),
        GenTerm::return_(IndexTerm::boolean(true)),
    ));
    if cost > *budget {
        tracing::debug!(
            selector = ?cond,
            cost,
            budget = *budget,
            "push_pull: copying the arms' prefix would exceed the growth budget; the prefix stays above"
        );
        let mut stmts = kept;
        stmts.extend(moved);
        stmts.extend(rest);
        return (stmts, tail);
    }
    *budget -= cost;
    tracing::debug!(selector = ?cond, cost, remaining = *budget, "push_pull: pushed");

    let (kind, _) = tail.into_parts();
    let GenTermKind::Ite { cond, then_, else_ } = kind else {
        unreachable!("the tail was matched as an ite above")
    };
    let then_ = rebuild(moved.clone(), *then_);
    let else_ = rebuild(moved, *else_);
    let mut stmts = kept;
    stmts.extend(rest);
    (stmts, GenTerm::ite(cond, then_, else_))
}

pub fn push_pull(term: GenTerm, syms: &mut SymGen, effects: &ChoiceEffects) -> GenTerm {
    let mut budget = MAX_PUSH_GROWTH;
    push_pull_bounded(term, syms, effects, &mut budget)
}

pub fn push_pull_bounded(
    term: GenTerm,
    syms: &mut SymGen,
    effects: &ChoiceEffects,
    budget: &mut usize,
) -> GenTerm {
    let mut term = term;
    let mut rounds = 0usize;
    loop {
        let pulled = pull(term.clone(), syms, effects);
        let next = if *budget == 0 {
            pulled
        } else {
            push(pulled, budget)
        };
        if next == term {
            return next;
        }
        term = next;
        rounds += 1;
        debug_assert!(
            rounds < MAX_ROUNDS,
            "push_pull failed to converge: rewrites are cycling"
        );
    }
}
