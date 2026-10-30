use num_bigint::BigInt;

use crate::gen_dsl::lang::base_type::{BaseType, bits_range};
use crate::gen_dsl::lang::index_term::{BinOp, IndexTerm, Term};
use crate::gen_dsl::lang::sym::Sym;
use crate::gen_dsl::passes::TransformError;

fn unsupported(bt: &BaseType) -> TransformError {
    TransformError::UnsupportedIndexType { bt: bt.clone() }
}

fn as_var(it: &IndexTerm) -> Option<&Sym> {
    match &it.term {
        Term::Sym(x) => Some(x),
        _ => None,
    }
}

fn is_var(it: &IndexTerm, var: &Sym) -> bool {
    as_var(it).is_some_and(|x| x == var)
}

fn binop(op: BinOp, l: IndexTerm, r: IndexTerm, bt: &BaseType) -> IndexTerm {
    IndexTerm {
        term: Term::Binop(op, Box::new(l), Box::new(r)),
        bt: bt.clone(),
    }
}

fn offset_one(op: BinOp, t: &IndexTerm, bt: &BaseType) -> Option<IndexTerm> {
    let one = IndexTerm::num_lit(BigInt::from(1), bt)?;
    Some(binop(op, t.clone(), one, bt))
}

fn eq_bound(var: &Sym, l: &IndexTerm, r: &IndexTerm) -> Option<IndexTerm> {
    if let Some(x) = as_var(l) {
        return (x == var).then(|| r.clone());
    }
    if let Some(x) = as_var(r) {
        return (x == var).then(|| l.clone());
    }
    None
}

fn and_combine(
    tighter: BinOp,
    a: Option<IndexTerm>,
    b: Option<IndexTerm>,
    bt: &BaseType,
) -> Option<IndexTerm> {
    match (a, b) {
        (None, None) => None,
        (Some(t), None) | (None, Some(t)) => Some(t),
        (Some(l), Some(r)) => Some(binop(tighter, l, r, bt)),
    }
}

fn or_combine(
    looser: BinOp,
    a: Option<IndexTerm>,
    b: Option<IndexTerm>,
    bt: &BaseType,
) -> Option<IndexTerm> {
    match (a, b) {
        (Some(l), Some(r)) => Some(binop(looser, l, r, bt)),
        _ => None,
    }
}

pub fn lower_bound_opt(var: &Sym, bt: &BaseType, perm: &IndexTerm) -> Option<IndexTerm> {
    match &perm.term {
        Term::Binop(BinOp::Eq, l, r) => eq_bound(var, l, r),
        Term::Binop(BinOp::Le, l, r) if is_var(r, var) => Some((**l).clone()),
        Term::Binop(BinOp::Lt, l, r) if is_var(r, var) => offset_one(BinOp::Add, l, bt),
        Term::Binop(BinOp::And, l, r) => and_combine(
            BinOp::Max,
            lower_bound_opt(var, bt, l),
            lower_bound_opt(var, bt, r),
            bt,
        ),
        Term::Binop(BinOp::Or, l, r) => or_combine(
            BinOp::Min,
            lower_bound_opt(var, bt, l),
            lower_bound_opt(var, bt, r),
            bt,
        ),
        _ => None,
    }
}

pub fn upper_bound_opt(var: &Sym, bt: &BaseType, perm: &IndexTerm) -> Option<IndexTerm> {
    match &perm.term {
        Term::Binop(BinOp::Eq, l, r) => eq_bound(var, l, r),
        Term::Binop(BinOp::Le, l, r) if is_var(l, var) => Some((**r).clone()),
        Term::Binop(BinOp::Lt, l, r) if is_var(l, var) => offset_one(BinOp::Sub, r, bt),
        Term::Binop(BinOp::And, l, r) => and_combine(
            BinOp::Min,
            upper_bound_opt(var, bt, l),
            upper_bound_opt(var, bt, r),
            bt,
        ),
        Term::Binop(BinOp::Or, l, r) => or_combine(
            BinOp::Max,
            upper_bound_opt(var, bt, l),
            upper_bound_opt(var, bt, r),
            bt,
        ),
        _ => None,
    }
}

pub fn lower_bound(
    var: &Sym,
    bt: &BaseType,
    perm: &IndexTerm,
) -> Result<IndexTerm, TransformError> {
    let BaseType::Bits(sign, width) = bt else {
        return Err(unsupported(bt));
    };
    match lower_bound_opt(var, bt, perm) {
        Some(t) => Ok(t),
        None => IndexTerm::num_lit(bits_range(*sign, *width).0, bt).ok_or_else(|| unsupported(bt)),
    }
}

pub fn upper_bound(
    var: &Sym,
    bt: &BaseType,
    perm: &IndexTerm,
) -> Result<IndexTerm, TransformError> {
    let BaseType::Bits(sign, width) = bt else {
        return Err(unsupported(bt));
    };
    match upper_bound_opt(var, bt, perm) {
        Some(t) => Ok(t),
        None => IndexTerm::num_lit(bits_range(*sign, *width).1, bt).ok_or_else(|| unsupported(bt)),
    }
}

pub fn bounds(
    var: &Sym,
    bt: &BaseType,
    perm: &IndexTerm,
) -> Result<(IndexTerm, IndexTerm), TransformError> {
    Ok((lower_bound(var, bt, perm)?, upper_bound(var, bt, perm)?))
}
