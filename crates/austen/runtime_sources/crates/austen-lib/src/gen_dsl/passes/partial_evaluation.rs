use std::convert::Infallible;

use crate::gen_dsl::lang::eval::eval_in;
use crate::gen_dsl::lang::gen_module::GenModule;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind, ScopeName};
use crate::gen_dsl::lang::index_term::IndexTerm;
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::sym::SymMap;

pub fn partial_evaluation(term: GenTerm, ctx: &GenModule) -> GenTerm {
    rewrite_embedded(term, &mut |it| fold_term(it, ctx), &mut |lc| {
        fold_constraint(lc, ctx)
    })
}

pub(in crate::gen_dsl) fn rewrite_embedded(
    term: GenTerm,
    it_f: &mut impl FnMut(IndexTerm) -> IndexTerm,
    lc_f: &mut impl FnMut(LogicalConstraint) -> LogicalConstraint,
) -> GenTerm {
    let out: Result<GenTerm, Infallible> = term.map_pre(&mut |node| {
        let (term, bt) = node.into_parts();
        let term = match term {
            GenTermKind::Arbitrary => GenTermKind::Arbitrary,
            GenTermKind::WriteChoice { label, value, rest } => GenTermKind::WriteChoice {
                label,
                value: it_f(value),
                rest,
            },
            GenTermKind::Call { fsym, args } => GenTermKind::Call {
                fsym,
                args: args.into_iter().map(&mut *it_f).collect(),
            },
            GenTermKind::Asgn {
                addr,
                ct,
                value,
                rest,
            } => GenTermKind::Asgn {
                addr: it_f(addr),
                ct,
                value: it_f(value),
                rest,
            },
            GenTermKind::LetStar { var, value, rest } => GenTermKind::LetStar { var, value, rest },
            GenTermKind::Return(it) => GenTermKind::Return(it_f(it)),
            GenTermKind::Assert { constraint, rest } => GenTermKind::Assert {
                constraint: lc_f(constraint),
                rest,
            },
            GenTermKind::Ite { cond, then_, else_ } => GenTermKind::Ite {
                cond: it_f(cond),
                then_,
                else_,
            },
            GenTermKind::Map {
                var,
                var_bt,
                perm,
                body,
            } => GenTermKind::Map {
                var,
                var_bt,
                perm: it_f(perm),
                body,
            },
            GenTermKind::Scope { name, body } => GenTermKind::Scope {
                name: match name {
                    ScopeName::Index(it) => ScopeName::Index(it_f(it)),
                    name @ (ScopeName::Str(_) | ScopeName::Sid(_)) => name,
                },
                body,
            },
        };
        Ok(GenTerm::from_parts(term, bt))
    });
    out.unwrap()
}

pub(in crate::gen_dsl) fn fold_term(it: IndexTerm, ctx: &GenModule) -> IndexTerm {
    it.map_post(&mut |node| fold_ground(&node, ctx).unwrap_or(node))
}

fn fold_constraint(lc: LogicalConstraint, ctx: &GenModule) -> LogicalConstraint {
    match lc {
        LogicalConstraint::T(it) => LogicalConstraint::T(fold_term(it, ctx)),
        LogicalConstraint::Forall {
            var,
            permission,
            body,
        } => LogicalConstraint::Forall {
            var,
            permission: fold_term(permission, ctx),
            body: fold_term(body, ctx),
        },
    }
}

pub(in crate::gen_dsl) fn fold_ground(it: &IndexTerm, ctx: &GenModule) -> Option<IndexTerm> {
    let value = eval_in(it, &SymMap::new(), ctx)?;
    let folded = value.to_literal()?;
    (folded.bt == it.bt).then_some(folded)
}
