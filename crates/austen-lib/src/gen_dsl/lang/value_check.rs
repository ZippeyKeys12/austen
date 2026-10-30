use num_bigint::BigInt;

use crate::gen_dsl::lang::base_type::{BaseType, bits_range};
use crate::gen_dsl::lang::index_term::{BinOp, Const, IndexTerm, Term};
use crate::gen_dsl::lang::make_term::uintptr_bt;
use crate::gen_dsl::lang::sctype::Sctype;
use crate::gen_dsl::lang::struct_def::StructDef;
use crate::gen_dsl::lang::sym::Sym;

pub const MAX_VALUE_CHECK_DEPTH: usize = 64;

pub const SYNTHETIC_SYM_BASE: u64 = 1 << 60;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValueCheckError {
    #[error("no LP64 layout for C type {0:?}")]
    UnsizedCtype(Sctype),
    #[error("no definition for struct {tag:?}")]
    UnknownStruct { tag: Sym },
    #[error("expected {expected:?}, got {actual:?}")]
    TypeMismatch {
        expected: BaseType,
        actual: BaseType,
    },
    #[error("no value check at base type {0:?}")]
    UnsupportedBaseType(BaseType),
    #[error("value check nests deeper than {limit}")]
    TooDeep { limit: usize },
}

pub fn good(
    ct: &Sctype,
    about: &IndexTerm,
    structs: &[(Sym, StructDef)],
) -> Result<IndexTerm, ValueCheckError> {
    value_check(true, ct, about, structs)
}

pub fn good_pointer_alignment(
    pointee: &Sctype,
    structs: &[(Sym, StructDef)],
) -> Result<u64, ValueCheckError> {
    match pointee {
        Sctype::Void | Sctype::Function { .. } => Ok(1),
        other => other
            .align_in(structs)
            .ok_or_else(|| ValueCheckError::UnsizedCtype(other.clone())),
    }
}

pub fn representable(
    ct: &Sctype,
    about: &IndexTerm,
    structs: &[(Sym, StructDef)],
) -> Result<IndexTerm, ValueCheckError> {
    value_check(false, ct, about, structs)
}

pub fn value_check(
    good: bool,
    ct: &Sctype,
    about: &IndexTerm,
    structs: &[(Sym, StructDef)],
) -> Result<IndexTerm, ValueCheckError> {
    value_check_at(good, ct, about, structs, 0)
}

fn value_check_at(
    good: bool,
    ct: &Sctype,
    about: &IndexTerm,
    structs: &[(Sym, StructDef)],
    depth: usize,
) -> Result<IndexTerm, ValueCheckError> {
    if depth >= MAX_VALUE_CHECK_DEPTH {
        return Err(ValueCheckError::TooDeep {
            limit: MAX_VALUE_CHECK_DEPTH,
        });
    }
    let deeper = depth + 1;
    match ct {
        Sctype::Void | Sctype::Byte => Ok(IndexTerm::boolean(true)),
        Sctype::Integer(it) => {
            let (min, max) = it
                .range()
                .ok_or_else(|| ValueCheckError::UnsizedCtype(ct.clone()))?;
            in_z_range(about, &min, &max)
        }
        Sctype::Pointer(_) if !good => Ok(IndexTerm::boolean(true)),
        Sctype::Pointer(pointee) => {
            let align = good_pointer_alignment(pointee, structs)?;
            let align = IndexTerm::num_lit(BigInt::from(align), &uintptr_bt())
                .ok_or_else(|| ValueCheckError::UnsizedCtype((**pointee).clone()))?;
            Ok(divisible(about, &align))
        }
        Sctype::Struct(tag) => {
            let def = structs
                .iter()
                .find(|(t, _)| t == tag)
                .map(|(_, d)| d)
                .ok_or_else(|| ValueCheckError::UnknownStruct { tag: tag.clone() })?;
            let mut acc = IndexTerm::boolean(true);
            for (id, member_ct) in &def.members {
                let member_bt = member_ct
                    .to_base_type()
                    .ok_or_else(|| ValueCheckError::UnsizedCtype(member_ct.clone()))?;
                let projected = IndexTerm::struct_member(about.clone(), id.clone(), member_bt);
                let check = value_check_at(good, member_ct, &projected, structs, deeper)?;
                acc = IndexTerm::and2(acc, check);
            }
            Ok(acc)
        }
        Sctype::Array(item, Some(n)) => {
            let n = i64::try_from(*n).map_err(|_| ValueCheckError::UnsizedCtype(ct.clone()))?;
            if n == 0 {
                return Ok(IndexTerm::boolean(true));
            }
            let (index_bt, item_bt) = match &about.bt {
                BaseType::Map(k, v) => ((**k).clone(), (**v).clone()),
                other => {
                    return Err(ValueCheckError::TypeMismatch {
                        expected: BaseType::make_map(uintptr_bt(), BaseType::Unit),
                        actual: other.clone(),
                    });
                }
            };
            let sym = fresh_index(about);
            let index = IndexTerm::sym(sym.clone(), index_bt.clone());
            let element = IndexTerm {
                term: Term::MapGet(Box::new(about.clone()), Box::new(index)),
                bt: item_bt,
            };
            let body = value_check_at(good, item, &element, structs, deeper)?;
            Ok(IndexTerm {
                term: Term::EachI {
                    start: 0,
                    var: (sym, index_bt),
                    end: n - 1,
                    body: Box::new(body),
                },
                bt: BaseType::Bool,
            })
        }
        Sctype::Array(_, None) | Sctype::Function { .. } => {
            Err(ValueCheckError::UnsizedCtype(ct.clone()))
        }
    }
}

fn fresh_index(about: &IndexTerm) -> Sym {
    let taken = about.free_vars();
    let mut num = SYNTHETIC_SYM_BASE;
    while taken.contains(&Sym { name: None, num }) {
        num += 1;
    }
    Sym {
        name: Some("i".into()),
        num,
    }
}

pub fn divisible(t: &IndexTerm, align: &IndexTerm) -> IndexTerm {
    let addr = IndexTerm::cast(uintptr_bt(), t.clone());
    let rem = IndexTerm {
        term: Term::Binop(BinOp::Mod, Box::new(addr), Box::new(align.clone())),
        bt: uintptr_bt(),
    };
    let zero = IndexTerm {
        term: Term::Const(Const::Bits(
            (crate::gen_dsl::lang::base_type::Sign::Unsigned, 64),
            BigInt::ZERO,
        )),
        bt: uintptr_bt(),
    };
    IndexTerm::eq(rem, zero)
}

fn intptr_bt() -> BaseType {
    BaseType::Bits(crate::gen_dsl::lang::base_type::Sign::Signed, 64)
}

pub fn in_z_range(
    within: &IndexTerm,
    min: &BigInt,
    max: &BigInt,
) -> Result<IndexTerm, ValueCheckError> {
    match &within.bt {
        BaseType::Integer => {
            let lo = IndexTerm::num_lit(min.clone(), &BaseType::Integer)
                .ok_or(ValueCheckError::UnsupportedBaseType(BaseType::Integer))?;
            let hi = IndexTerm::num_lit(max.clone(), &BaseType::Integer)
                .ok_or(ValueCheckError::UnsupportedBaseType(BaseType::Integer))?;
            Ok(IndexTerm::and2(
                IndexTerm::le(lo, within.clone()),
                IndexTerm::le(within.clone(), hi),
            ))
        }
        BaseType::Bits(sign, width) => {
            let (min_possible, max_possible) = bits_range(*sign, *width);
            let bound = |limit: &BigInt, lower: bool| -> IndexTerm {
                let (implied, impossible) = if lower {
                    (*limit <= min_possible, *limit > max_possible)
                } else {
                    (max_possible <= *limit, *limit < min_possible)
                };
                if implied {
                    IndexTerm::boolean(true)
                } else if impossible {
                    IndexTerm::boolean(false)
                } else {
                    let lit = IndexTerm::num_lit(limit.clone(), &within.bt)
                        .expect("a limit inside the type's range is representable in it");
                    if lower {
                        IndexTerm::le(lit, within.clone())
                    } else {
                        IndexTerm::le(within.clone(), lit)
                    }
                }
            };
            Ok(IndexTerm::and2(bound(min, true), bound(max, false)))
        }
        BaseType::Loc(_) => {
            let as_unsigned = in_z_range(&IndexTerm::cast(uintptr_bt(), within.clone()), min, max)?;
            let as_signed = in_z_range(&IndexTerm::cast(intptr_bt(), within.clone()), min, max)?;
            Ok(IndexTerm {
                term: Term::Binop(BinOp::Or, Box::new(as_unsigned), Box::new(as_signed)),
                bt: BaseType::Bool,
            })
        }
        other => Err(ValueCheckError::UnsupportedBaseType(other.clone())),
    }
}
