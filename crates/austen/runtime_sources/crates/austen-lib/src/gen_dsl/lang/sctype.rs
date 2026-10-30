use serde::{Deserialize, Serialize};

use crate::gen_dsl::lang::base_type::{BaseType, Sign};
use crate::gen_dsl::lang::sym::Sym;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum IntegerBaseType {
    Ichar,
    Short,
    Int,
    Long,
    LongLong,
    IntN(u16),
    IntLeastN(u16),
    IntFastN(u16),
    Intmax,
    Intptr,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum IntegerType {
    Char,
    Bool,
    Signed(IntegerBaseType),
    Unsigned(IntegerBaseType),
    Enum(Sym),
    WcharT,
    WintT,
    SizeT,
    PtrdiffT,
    PtraddrT,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(deny_unknown_fields)]
pub struct Qualifiers {
    pub const_: bool,
    pub restrict: bool,
    pub volatile: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Sctype {
    Void,
    Integer(IntegerType),
    Array(Box<Sctype>, Option<usize>),
    Pointer(Box<Sctype>),
    Struct(Sym),
    Function {
        return_type: (Qualifiers, Box<Sctype>),
        args: Vec<(Sctype, bool)>,
        variadic: bool,
    },
    Byte,
}

impl IntegerBaseType {
    fn byte_size(&self) -> Option<u64> {
        match self {
            IntegerBaseType::Ichar => Some(1),
            IntegerBaseType::Short => Some(2),
            IntegerBaseType::Int => Some(4),
            IntegerBaseType::Long | IntegerBaseType::LongLong => Some(8),
            IntegerBaseType::IntN(b)
            | IntegerBaseType::IntLeastN(b)
            | IntegerBaseType::IntFastN(b) => {
                matches!(b, 8 | 16 | 32 | 64).then(|| u64::from(*b) / 8)
            }
            IntegerBaseType::Intmax | IntegerBaseType::Intptr => Some(8),
        }
    }
}

impl IntegerType {
    pub fn is_signed(&self) -> Option<bool> {
        match self {
            IntegerType::Char
            | IntegerType::Signed(_)
            | IntegerType::WcharT
            | IntegerType::WintT
            | IntegerType::PtrdiffT => Some(true),
            IntegerType::Bool
            | IntegerType::Unsigned(_)
            | IntegerType::SizeT
            | IntegerType::PtraddrT => Some(false),
            IntegerType::Enum(_) => None,
        }
    }

    pub fn range(&self) -> Option<(num_bigint::BigInt, num_bigint::BigInt)> {
        use num_bigint::BigInt;
        let width = u16::try_from(self.byte_size()? * 8).ok()?;
        let signed = crate::gen_dsl::lang::base_type::bits_range(Sign::Signed, width);
        let unsigned = crate::gen_dsl::lang::base_type::bits_range(Sign::Unsigned, width);
        Some(match self {
            IntegerType::WcharT => unsigned,
            IntegerType::WintT => (BigInt::ZERO, signed.1),
            IntegerType::Char => signed,
            IntegerType::Signed(_) | IntegerType::PtrdiffT => signed,
            IntegerType::Bool
            | IntegerType::Unsigned(_)
            | IntegerType::SizeT
            | IntegerType::PtraddrT => unsigned,
            IntegerType::Enum(_) => return None,
        })
    }

    fn byte_size(&self) -> Option<u64> {
        match self {
            IntegerType::Char | IntegerType::Bool => Some(1),
            IntegerType::Signed(ibt) | IntegerType::Unsigned(ibt) => ibt.byte_size(),
            IntegerType::Enum(_) => None,
            IntegerType::WcharT | IntegerType::WintT => Some(4),
            IntegerType::SizeT | IntegerType::PtrdiffT | IntegerType::PtraddrT => Some(8),
        }
    }
}

impl Sctype {
    pub fn byte_size(&self) -> Option<u64> {
        match self {
            Sctype::Void | Sctype::Struct(_) | Sctype::Function { .. } => None,
            Sctype::Integer(it) => it.byte_size(),
            Sctype::Array(elem, Some(n)) => elem.byte_size()?.checked_mul(u64::try_from(*n).ok()?),
            Sctype::Array(_, None) => None,
            Sctype::Pointer(_) => Some(8),
            Sctype::Byte => Some(1),
        }
    }

    pub fn byte_size_in(
        &self,
        structs: &[(Sym, crate::gen_dsl::lang::struct_def::StructDef)],
    ) -> Option<u64> {
        crate::gen_dsl::lang::struct_def::size_at(self, structs, 0)
    }

    pub fn align_in(
        &self,
        structs: &[(Sym, crate::gen_dsl::lang::struct_def::StructDef)],
    ) -> Option<u64> {
        crate::gen_dsl::lang::struct_def::align_at(self, structs, 0)
    }

    pub fn to_base_type(&self) -> Option<BaseType> {
        match self {
            Sctype::Void => Some(BaseType::Unit),
            Sctype::Integer(it) => {
                let sign = if it.is_signed()? {
                    Sign::Signed
                } else {
                    Sign::Unsigned
                };
                Some(BaseType::Bits(
                    sign,
                    u16::try_from(it.byte_size()? * 8).ok()?,
                ))
            }
            Sctype::Array(elem, _) => Some(BaseType::make_map(
                BaseType::Bits(Sign::Unsigned, 64),
                elem.to_base_type()?,
            )),
            Sctype::Pointer(ct) if sizeable_somewhere(ct) => Some(BaseType::loc_to((**ct).clone())),
            Sctype::Pointer(_) => Some(BaseType::Loc(None)),
            Sctype::Struct(tag) => Some(BaseType::Struct(tag.clone())),
            Sctype::Byte => Some(BaseType::Option(Box::new(BaseType::MemByte))),
            Sctype::Function { .. } => None,
        }
    }
}

fn sizeable_somewhere(ct: &Sctype) -> bool {
    match ct {
        Sctype::Struct(_) => true,
        Sctype::Array(elem, Some(_)) => sizeable_somewhere(elem),
        other => other.byte_size().is_some(),
    }
}
