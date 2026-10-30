use num_bigint::BigInt;

use austen_lib::gen_dsl::{
    BaseType, BinOp, GenModule, IndexTerm, MapDomainError, Sign, Substitution, Sym, SymMap,
    SymbolicMapBounds, Term, UnOp, Value, eval_in, normalise_to_range_big, symbolic_map_bounds,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MapPlan {
    lower: IndexTerm,
    upper: IndexTerm,
    nonempty: IndexTerm,
    length_constraint: IndexTerm,
    candidates: Candidates,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Candidates {
    Contiguous(usize),
    Explicit(Vec<IndexTerm>),
}

impl MapPlan {
    pub(crate) fn new(
        permission: &IndexTerm,
        var: &Sym,
        var_bt: &BaseType,
        ctx: &GenModule,
        max_array_length: usize,
    ) -> Result<Self, MapDomainError> {
        let max_array_length = max_array_length.clamp(
            1,
            austen_lib::gen_dsl::choice::translate::MAX_MAP_ITERATIONS as usize,
        );
        let source = match symbolic_map_bounds(permission, var) {
            Ok(source) => source,
            Err(MapDomainError::Unsupported) => {
                return Self::static_bounded(permission, var, var_bt, ctx, max_array_length);
            }
            Err(error) => return Err(error),
        };
        if source.index_bt != *var_bt || source.lower.bt != *var_bt || source.upper.bt != *var_bt {
            return Err(MapDomainError::Unsupported);
        }
        let candidate_count = potential_count(&source, var_bt, ctx, max_array_length);
        let lower = effective_lower(&source, var_bt)?;
        let upper = effective_upper(&source, var_bt)?;
        let nonempty = substitute_var(permission, var, lower.clone());
        let bounded_length = match type_cardinality(var_bt) {
            Some(cardinality) if cardinality <= max_array_length => IndexTerm::boolean(true),
            _ => {
                let first_past_cap = offset_from(lower.clone(), max_array_length, var_bt)?;
                negate(substitute_var(permission, var, first_past_cap))
            }
        };
        let length_constraint = bool_binop(BinOp::Implies, nonempty.clone(), bounded_length);

        Ok(Self {
            lower,
            upper,
            nonempty,
            length_constraint,
            candidates: Candidates::Contiguous(candidate_count),
        })
    }

    fn static_bounded(
        permission: &IndexTerm,
        var: &Sym,
        var_bt: &BaseType,
        ctx: &GenModule,
        max_array_length: usize,
    ) -> Result<Self, MapDomainError> {
        let lower =
            austen_lib::gen_dsl::passes::term_bounds::lower_bound_opt(var, var_bt, permission)
                .ok_or(MapDomainError::Unsupported)?;
        let upper =
            austen_lib::gen_dsl::passes::term_bounds::upper_bound_opt(var, var_bt, permission)
                .ok_or(MapDomainError::Unsupported)?;
        if lower.bt != *var_bt
            || upper.bt != *var_bt
            || lower.free_vars().contains(var)
            || upper.free_vars().contains(var)
        {
            return Err(MapDomainError::Unsupported);
        }
        let lo = endpoint_value(&lower, ctx).ok_or(MapDomainError::Unevaluatable)?;
        let inclusive_hi = endpoint_value(&upper, ctx).ok_or(MapDomainError::Unevaluatable)?;
        let hi = inclusive_hi
            .checked_add(1)
            .ok_or(MapDomainError::Unevaluatable)?;
        let span = if hi <= lo {
            0
        } else {
            hi.checked_sub(lo).ok_or(MapDomainError::Unevaluatable)?
        };
        if span > max_array_length as i128 {
            return Err(MapDomainError::TooLarge { lo, hi });
        }

        let mut explicit_candidates = Vec::with_capacity(span as usize);
        for key in lo..hi {
            explicit_candidates.push(
                IndexTerm::num_lit(BigInt::from(key), var_bt)
                    .ok_or(MapDomainError::Unevaluatable)?,
            );
        }
        let nonempty = explicit_candidates
            .iter()
            .cloned()
            .map(|key| Self::permission_at(permission, var, key))
            .reduce(|left, right| bool_binop(BinOp::Or, left, right))
            .unwrap_or_else(|| IndexTerm::boolean(false));
        Ok(Self {
            lower,
            upper,
            nonempty,
            length_constraint: IndexTerm::boolean(true),
            candidates: Candidates::Explicit(explicit_candidates),
        })
    }

    pub(crate) fn lower(&self) -> &IndexTerm {
        &self.lower
    }

    pub(crate) fn upper(&self) -> &IndexTerm {
        &self.upper
    }

    pub(crate) fn nonempty(&self) -> &IndexTerm {
        &self.nonempty
    }

    pub(crate) fn length_constraint(&self) -> &IndexTerm {
        &self.length_constraint
    }

    pub(crate) fn candidate_count(&self) -> usize {
        match &self.candidates {
            Candidates::Contiguous(count) => *count,
            Candidates::Explicit(keys) => keys.len(),
        }
    }

    pub(crate) fn is_contiguous(&self) -> bool {
        matches!(self.candidates, Candidates::Contiguous(_))
    }

    pub(crate) fn candidate(&self, offset: usize) -> Result<IndexTerm, MapDomainError> {
        if offset >= self.candidate_count() {
            return Err(MapDomainError::TooLarge {
                lo: 0,
                hi: i128::try_from(offset).unwrap_or(i128::MAX),
            });
        }
        match &self.candidates {
            Candidates::Explicit(keys) => Ok(keys[offset].clone()),
            Candidates::Contiguous(_) if offset == 0 => Ok(self.lower.clone()),
            Candidates::Contiguous(_) => offset_from(self.lower.clone(), offset, &self.lower.bt),
        }
    }

    pub(crate) fn permission_at(permission: &IndexTerm, var: &Sym, key: IndexTerm) -> IndexTerm {
        substitute_var(permission, var, key)
    }
}

fn effective_lower(bounds: &SymbolicMapBounds, bt: &BaseType) -> Result<IndexTerm, MapDomainError> {
    if bounds.lower_inclusive {
        Ok(bounds.lower.clone())
    } else {
        offset_from(bounds.lower.clone(), 1, bt)
    }
}

fn effective_upper(bounds: &SymbolicMapBounds, bt: &BaseType) -> Result<IndexTerm, MapDomainError> {
    if bounds.upper_inclusive {
        Ok(bounds.upper.clone())
    } else {
        let one = offset_literal(1, bt).ok_or(MapDomainError::Unevaluatable)?;
        Ok(binop(BinOp::Sub, bounds.upper.clone(), one, bt))
    }
}

fn offset_from(base: IndexTerm, offset: usize, bt: &BaseType) -> Result<IndexTerm, MapDomainError> {
    let offset = offset_literal(offset, bt).ok_or(MapDomainError::Unevaluatable)?;
    Ok(binop(BinOp::Add, base, offset, bt))
}

fn offset_literal(offset: usize, bt: &BaseType) -> Option<IndexTerm> {
    let value = BigInt::from(offset);
    let value = match bt {
        BaseType::Bits(sign, width) => normalise_to_range_big(*sign, *width, &value),
        BaseType::Integer => value,
        _ => return None,
    };
    IndexTerm::num_lit(value, bt)
}

fn binop(op: BinOp, left: IndexTerm, right: IndexTerm, bt: &BaseType) -> IndexTerm {
    IndexTerm {
        term: Term::Binop(op, Box::new(left), Box::new(right)),
        bt: bt.clone(),
    }
}

pub(crate) fn bool_binop(op: BinOp, left: IndexTerm, right: IndexTerm) -> IndexTerm {
    IndexTerm {
        term: Term::Binop(op, Box::new(left), Box::new(right)),
        bt: BaseType::Bool,
    }
}

pub(crate) fn negate(term: IndexTerm) -> IndexTerm {
    IndexTerm {
        term: Term::Unop(UnOp::Not, Box::new(term)),
        bt: BaseType::Bool,
    }
}

pub(crate) fn substitute_var(term: &IndexTerm, var: &Sym, value: IndexTerm) -> IndexTerm {
    let mut subst = Substitution::new();
    subst.insert(var.clone(), value);
    term.subst(&subst)
}

fn endpoint_value(term: &IndexTerm, ctx: &GenModule) -> Option<i128> {
    eval_in(term, &SymMap::<Value>::new(), ctx)?.as_int()
}

fn numeric_endpoints(bounds: &SymbolicMapBounds, ctx: &GenModule) -> (Option<i128>, Option<i128>) {
    let lower = endpoint_value(&bounds.lower, ctx).and_then(|n| {
        if bounds.lower_inclusive {
            Some(n)
        } else {
            n.checked_add(1)
        }
    });
    let upper = endpoint_value(&bounds.upper, ctx).and_then(|n| {
        if bounds.upper_inclusive {
            Some(n)
        } else {
            n.checked_sub(1)
        }
    });
    (lower, upper)
}

fn potential_count(
    bounds: &SymbolicMapBounds,
    bt: &BaseType,
    ctx: &GenModule,
    max_array_length: usize,
) -> usize {
    let cap = type_cardinality(bt)
        .unwrap_or(max_array_length)
        .min(max_array_length);
    let count = match numeric_endpoints(bounds, ctx) {
        (Some(lower), Some(upper)) => upper.checked_sub(lower).and_then(|n| n.checked_add(1)),
        (None, Some(upper)) if matches!(bt, BaseType::Bits(Sign::Unsigned, _)) => {
            upper.checked_add(1)
        }
        _ => return cap,
    };
    match count {
        Some(count) if count <= 0 => 0,
        Some(count) => usize::try_from(count).unwrap_or(cap).min(cap),
        None => cap,
    }
}

fn type_cardinality(bt: &BaseType) -> Option<usize> {
    let BaseType::Bits(_, width) = bt else {
        return None;
    };
    1usize.checked_shl(u32::from(*width))
}
