use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::index_term::{BinOp, IndexTerm, Term, UnOp};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::sym::{Sym, SymSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingletonSearch {
    TunnelLets,
    StopAtLet,
}

fn pins(side: &IndexTerm, x: &Sym) -> Option<BaseType> {
    match &side.term {
        Term::Sym(y) if y == x => Some(side.bt.clone()),
        Term::Cast(_, inner) => match &inner.term {
            Term::Sym(y) if y == x => Some(inner.bt.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn pinning_equality(lc: &LogicalConstraint, x: &Sym, vars: &SymSet) -> Option<IndexTerm> {
    let LogicalConstraint::T(it) = lc else {
        return None;
    };
    let Term::Binop(BinOp::Eq, lhs, rhs) = &it.term else {
        return None;
    };
    if let Some(bt) = pins(lhs, x)
        && rhs.free_vars().is_subset(vars)
    {
        return Some(IndexTerm::cast(bt, (**rhs).clone()));
    }
    if let Some(bt) = pins(rhs, x)
        && lhs.free_vars().is_subset(vars)
    {
        return Some(IndexTerm::cast(bt, (**lhs).clone()));
    }
    None
}

fn pinning_boolean(lc: &LogicalConstraint, x: &Sym) -> Option<IndexTerm> {
    let LogicalConstraint::T(it) = lc else {
        return None;
    };
    let (side, value) = match &it.term {
        Term::Unop(UnOp::Not, inner) => (&**inner, false),
        _ => (it, true),
    };
    let pinned = matches!(&side.term, Term::Sym(y) if y == x) && side.bt == BaseType::Bool;
    pinned.then(|| IndexTerm::boolean(value))
}

fn pinning_constraint(lc: &LogicalConstraint, x: &Sym, vars: &SymSet) -> Option<IndexTerm> {
    pinning_equality(lc, x, vars).or_else(|| pinning_boolean(lc, x))
}

fn find_constraint(
    vars: &SymSet,
    x: &Sym,
    gt: GenTerm,
    search: SingletonSearch,
) -> Result<(GenTerm, IndexTerm), Box<GenTerm>> {
    match gt.term {
        GenTermKind::Arbitrary
        | GenTermKind::Call { .. }
        | GenTermKind::Return(_)
        | GenTermKind::Ite { .. }
        | GenTermKind::Map { .. }
        | GenTermKind::Scope { .. } => Err(Box::new(gt)),
        GenTermKind::WriteChoice { label, value, rest } => {
            match find_constraint(vars, x, *rest, search) {
                Ok((rest, it)) => Ok((GenTerm::write_choice(label, value, rest), it)),
                Err(rest) => Err(Box::new(GenTerm::write_choice(label, value, *rest))),
            }
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => match find_constraint(vars, x, *rest, search) {
            Ok((rest, it)) => Ok((GenTerm::asgn(addr, ct, value, rest), it)),
            Err(rest) => Err(Box::new(GenTerm::asgn(addr, ct, value, *rest))),
        },
        GenTermKind::LetStar { var, value, rest } => match search {
            SingletonSearch::StopAtLet => Err(Box::new(GenTerm::from_parts(
                GenTermKind::LetStar { var, value, rest },
                gt.bt,
            ))),
            SingletonSearch::TunnelLets => {
                if var == *x {
                    return Err(Box::new(GenTerm::from_parts(
                        GenTermKind::LetStar { var, value, rest },
                        gt.bt,
                    )));
                }
                match find_constraint(vars, x, *rest, search) {
                    Ok((rest, it)) => Ok((GenTerm::let_star((var, *value), rest), it)),
                    Err(rest) => Err(Box::new(GenTerm::let_star((var, *value), *rest))),
                }
            }
        },
        GenTermKind::Assert { constraint, rest } => {
            if let Some(it) = pinning_constraint(&constraint, x, vars) {
                return Ok((*rest, it));
            }
            match find_constraint(vars, x, *rest, search) {
                Ok((rest, it)) => Ok((GenTerm::assert(constraint, rest), it)),
                Err(rest) => Err(Box::new(GenTerm::assert(constraint, *rest))),
            }
        }
    }
}

fn transform_gt(vars: &SymSet, gt: GenTerm, search: SingletonSearch) -> GenTerm {
    let (term, bt) = gt.into_parts();
    match term {
        GenTermKind::Arbitrary | GenTermKind::Call { .. } | GenTermKind::Return(_) => {
            GenTerm::from_parts(term, bt)
        }
        GenTermKind::WriteChoice { label, value, rest } => {
            GenTerm::write_choice(label, value, transform_gt(vars, *rest, search))
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => GenTerm::asgn(addr, ct, value, transform_gt(vars, *rest, search)),
        GenTermKind::LetStar { var, value, rest } => {
            let (value, rest) = if matches!(value.term, GenTermKind::Arbitrary) {
                match find_constraint(vars, &var, *rest, search) {
                    Ok((rest, it)) => (GenTerm::return_(it), rest),
                    Err(rest) => (*value, *rest),
                }
            } else {
                (transform_gt(vars, *value, search), *rest)
            };
            let mut inner = vars.clone();
            inner.insert(var.clone());
            GenTerm::let_star((var, value), transform_gt(&inner, rest, search))
        }
        GenTermKind::Assert { constraint, rest } => {
            GenTerm::assert(constraint, transform_gt(vars, *rest, search))
        }
        GenTermKind::Ite { cond, then_, else_ } => GenTerm::from_parts(
            GenTermKind::Ite {
                cond,
                then_: Box::new(transform_gt(vars, *then_, search)),
                else_: Box::new(transform_gt(vars, *else_, search)),
            },
            bt,
        ),
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => {
            let mut inner = vars.clone();
            inner.insert(var.clone());
            let body = transform_gt(&inner, *body, search);
            GenTerm::map((var, var_bt, perm), body)
        }
        GenTermKind::Scope { name, body } => {
            GenTerm::scope(name, transform_gt(vars, *body, search))
        }
    }
}

pub fn specialize_singleton(iargs: &SymSet, term: GenTerm, search: SingletonSearch) -> GenTerm {
    transform_gt(iargs, term, search)
}
