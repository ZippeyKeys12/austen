use num_bigint::BigInt;

use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::index_term::{BinOp, IndexTerm, Term};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::sym::Sym;
use crate::gen_dsl::passes::{TransformError, peel_scopes, term_bounds, wrap_scopes};

pub const DEFAULT_MAX_ARRAY_LENGTH: usize = 50;

pub fn apply_array_max_length(
    term: GenTerm,
    max_array_length: usize,
) -> Result<GenTerm, TransformError> {
    if max_array_length == 0 {
        return Ok(term);
    }
    walk(term, max_array_length)
}

fn walk(gt: GenTerm, max: usize) -> Result<GenTerm, TransformError> {
    let (term, bt) = gt.into_parts();
    Ok(match term {
        term @ (GenTermKind::Arbitrary | GenTermKind::Call { .. } | GenTermKind::Return(_)) => {
            GenTerm::from_parts(term, bt)
        }
        GenTermKind::WriteChoice { label, value, rest } => {
            GenTerm::write_choice(label, value, walk(*rest, max)?)
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => GenTerm::asgn(addr, ct, value, walk(*rest, max)?),
        GenTermKind::LetStar { var, value, rest } => {
            let (scopes, value) = peel_scopes(*value);
            match value {
                GenTerm {
                    term:
                        GenTermKind::Map {
                            var: index,
                            var_bt: index_bt,
                            perm,
                            body,
                        },
                    ..
                } => {
                    let mapped = GenTerm::map(
                        (index.clone(), index_bt.clone(), perm.clone()),
                        walk(*body, max)?,
                    );
                    let binding =
                        GenTerm::let_star((var, wrap_scopes(scopes, mapped)), walk(*rest, max)?);
                    bound_length(&index, &index_bt, &perm, binding, max)?
                }
                value => GenTerm::let_star(
                    (var, wrap_scopes(scopes, walk(value, max)?)),
                    walk(*rest, max)?,
                ),
            }
        }
        GenTermKind::Assert { constraint, rest } => GenTerm::assert(constraint, walk(*rest, max)?),
        GenTermKind::Ite { cond, then_, else_ } => {
            let then_ = walk(*then_, max)?;
            let else_ = walk(*else_, max)?;
            GenTerm::rebuild_ite(cond, then_, else_)
        }
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => {
            let mapped = GenTerm::map(
                (var.clone(), var_bt.clone(), perm.clone()),
                walk(*body, max)?,
            );
            bound_length(&var, &var_bt, &perm, mapped, max)?
        }
        GenTermKind::Scope { name, body } => GenTerm::scope(name, walk(*body, max)?),
    })
}

fn bound_length(
    index: &Sym,
    index_bt: &BaseType,
    perm: &IndexTerm,
    gt: GenTerm,
    max: usize,
) -> Result<GenTerm, TransformError> {
    let Some(upper) = term_bounds::upper_bound_opt(index, index_bt, perm) else {
        return Ok(gt);
    };
    let lower = term_bounds::lower_bound(index, index_bt, perm)?;
    let (Some(one), Some(max_len)) = (
        IndexTerm::num_lit(BigInt::from(1), index_bt),
        IndexTerm::num_lit(BigInt::from(max), index_bt),
    ) else {
        return Ok(gt);
    };
    let arith = |op: BinOp, l: IndexTerm, r: IndexTerm| IndexTerm {
        term: Term::Binop(op, Box::new(l), Box::new(r)),
        bt: index_bt.clone(),
    };
    let array_len = arith(BinOp::Add, arith(BinOp::Sub, upper, lower), one);
    Ok(GenTerm::assert(
        LogicalConstraint::T(IndexTerm::le(array_len, max_len)),
        gt,
    ))
}
