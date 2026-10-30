use crate::gen_dsl::{BaseType, IndexTerm, Sctype, Sym, SymbolicMapBounds, symbolic_map_bounds};
use serde::{Deserialize, Serialize};

use crate::spec::{Clause, ClauseShapeError, PredicateDef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OracleSupportDefect {
    #[error("logical quantified request does not bind a map")]
    QuantifiedBindingNotMap,
    #[error("logical quantified request index and map key types disagree")]
    QuantifiedKeyMismatch,
    #[error("logical quantified request permission is not a finite range")]
    QuantifiedPermissionNotFinite,
    #[error("Owned request has an unsizable C type")]
    OwnedUnsized,
    #[error("Owned byte request has no logical value reader")]
    OwnedByteUnreadable,
    #[error("Owned request has no logical C value type")]
    OwnedNoLogicalType,
    #[error("Owned C type and resource binding type disagree")]
    OwnedTypeMismatch,
    #[error("logical predicate argument count disagrees with its declaration")]
    PredicateArity,
    #[error("logical predicate argument type disagrees with its declaration")]
    PredicateArgumentType,
    #[error("logical predicate result type disagrees with its resource binding")]
    PredicateResultType,
    #[error("logical request names an uninterpreted predicate")]
    PredicateUninterpreted,
    #[error("logical predicate has no clauses")]
    PredicateNoClauses,
    #[error("logical predicate has a guarded final clause")]
    PredicateGuardedFinalClause,
}

pub fn check_quantified_request<'a>(
    bt: &'a BaseType,
    key: &BaseType,
) -> Result<(&'a BaseType, &'a BaseType), OracleSupportDefect> {
    let BaseType::Map(map_key, map_value) = bt else {
        return Err(OracleSupportDefect::QuantifiedBindingNotMap);
    };
    if **map_key != *key {
        return Err(OracleSupportDefect::QuantifiedKeyMismatch);
    }
    Ok((map_key, map_value))
}

pub fn check_quantified_permission(
    permission: &IndexTerm,
    q: &Sym,
) -> Result<SymbolicMapBounds, OracleSupportDefect> {
    symbolic_map_bounds(permission, q)
        .map_err(|_| OracleSupportDefect::QuantifiedPermissionNotFinite)
}

pub fn check_owned_request(ct: &Sctype, expected: &BaseType) -> Result<(), OracleSupportDefect> {
    if matches!(ct, Sctype::Void | Sctype::Function { .. }) || matches!(ct, Sctype::Array(_, None))
    {
        return Err(OracleSupportDefect::OwnedUnsized);
    }
    if matches!(ct, Sctype::Byte) {
        return Err(OracleSupportDefect::OwnedByteUnreadable);
    }
    let Some(actual) = ct.to_base_type() else {
        return Err(OracleSupportDefect::OwnedNoLogicalType);
    };
    if logical_object_types_agree(&actual, expected) {
        Ok(())
    } else {
        Err(OracleSupportDefect::OwnedTypeMismatch)
    }
}

fn logical_object_types_agree(actual: &BaseType, expected: &BaseType) -> bool {
    match (actual, expected) {
        (BaseType::Bits(_, left), BaseType::Bits(_, right)) => left == right,
        (BaseType::Loc(_), BaseType::Loc(_)) => true,
        (BaseType::Map(left_key, left_value), BaseType::Map(right_key, right_value)) => {
            left_key == right_key && logical_object_types_agree(left_value, right_value)
        }
        _ => actual == expected,
    }
}

pub fn check_predicate_call<'a>(
    pred: &'a PredicateDef,
    iargs: &[IndexTerm],
    expected_bt: &BaseType,
) -> Result<&'a [Clause], OracleSupportDefect> {
    if iargs.len() != pred.iargs.len() {
        return Err(OracleSupportDefect::PredicateArity);
    }
    if iargs
        .iter()
        .zip(&pred.iargs)
        .any(|(actual, (_, expected))| actual.bt != *expected)
    {
        return Err(OracleSupportDefect::PredicateArgumentType);
    }
    if *expected_bt != pred.oarg {
        return Err(OracleSupportDefect::PredicateResultType);
    }
    pred.catch_all_clauses()
        .ok_or(OracleSupportDefect::PredicateUninterpreted)?
        .map_err(|shape| match shape {
            ClauseShapeError::Empty => OracleSupportDefect::PredicateNoClauses,
            ClauseShapeError::GuardedLast => OracleSupportDefect::PredicateGuardedFinalClause,
        })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Post {
    Check,
    Unsupported { reason: String },
}
