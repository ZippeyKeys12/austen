use serde::{Deserialize, Serialize};

use crate::gen_dsl::lang::id::Id;
use crate::gen_dsl::lang::sctype::Sctype;
use crate::gen_dsl::lang::sym::Sym;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Sign {
    Signed,
    Unsigned,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BaseType {
    Unit,
    Bool,
    Integer,
    MemByte,
    Bits(Sign, u16),
    Real,
    AllocId,
    Loc(Option<Box<Sctype>>),
    CType,
    Struct(Sym),
    Datatype(Sym),
    Record(MemberTypes),
    Map(Box<BaseType>, Box<BaseType>),
    List(Box<BaseType>),
    Tuple(Vec<BaseType>),
    Set(Box<BaseType>),
    Option(Box<BaseType>),
}

pub type MemberTypes = Vec<(Id, BaseType)>;

impl BaseType {
    pub const fn loc() -> BaseType {
        BaseType::Loc(None)
    }

    pub fn loc_to(ct: Sctype) -> BaseType {
        BaseType::Loc(Some(Box::new(ct)))
    }

    pub fn pointee(&self) -> Option<&Sctype> {
        match self {
            BaseType::Loc(ct) => ct.as_deref(),
            _ => None,
        }
    }

    pub fn is_loc(&self) -> bool {
        matches!(self, BaseType::Loc(_))
    }

    pub fn erase_pointees(&self) -> BaseType {
        let erase_members = |ms: &MemberTypes| -> MemberTypes {
            ms.iter()
                .map(|(id, bt)| (id.clone(), bt.erase_pointees()))
                .collect()
        };
        match self {
            BaseType::Loc(_) => BaseType::Loc(None),
            BaseType::Record(ms) => BaseType::Record(erase_members(ms)),
            BaseType::Map(k, v) => BaseType::make_map(k.erase_pointees(), v.erase_pointees()),
            BaseType::List(bt) => BaseType::List(Box::new(bt.erase_pointees())),
            BaseType::Set(bt) => BaseType::Set(Box::new(bt.erase_pointees())),
            BaseType::Option(bt) => BaseType::Option(Box::new(bt.erase_pointees())),
            BaseType::Tuple(bts) => {
                BaseType::Tuple(bts.iter().map(BaseType::erase_pointees).collect())
            }
            other => other.clone(),
        }
    }

    pub fn make_map(index: BaseType, value: BaseType) -> BaseType {
        BaseType::Map(Box::new(index), Box::new(value))
    }

    pub fn is_bits(&self) -> Option<(Sign, u16)> {
        match self {
            BaseType::Bits(sign, width) => Some((*sign, *width)),
            _ => None,
        }
    }
}

pub const CARRIER_BITS: u16 = 128;

pub fn normalise_to_range(sign: Sign, width: u16, v: i128) -> Option<i128> {
    if width == 0 || width > CARRIER_BITS {
        return None;
    }
    let mask = if width == 128 {
        !0
    } else {
        (1u128 << width) - 1
    };
    let u = (v as u128) & mask;
    match sign {
        Sign::Unsigned => i128::try_from(u).ok(),
        Sign::Signed if u & (1u128 << (width - 1)) == 0 => Some(u as i128),
        Sign::Signed => Some(u.wrapping_sub(mask).wrapping_sub(1) as i128),
    }
}

pub fn bits_residue(width: u16, n: &num_bigint::BigInt) -> num_bigint::BigInt {
    use num_bigint::BigInt;
    let modulus = BigInt::from(1) << width;
    ((n % &modulus) + &modulus) % &modulus
}

pub fn normalise_to_range_big(
    sign: Sign,
    width: u16,
    n: &num_bigint::BigInt,
) -> num_bigint::BigInt {
    use num_bigint::BigInt;
    if let Ok(value) = i128::try_from(n)
        && let Some(value) = normalise_to_range(sign, width, value)
    {
        return BigInt::from(value);
    }
    let residue = bits_residue(width, n);
    match sign {
        Sign::Unsigned => residue,
        Sign::Signed if residue >= (BigInt::from(1) << width.saturating_sub(1)) => {
            residue - (BigInt::from(1) << width)
        }
        Sign::Signed => residue,
    }
}

pub fn bits_range(sign: Sign, width: u16) -> (num_bigint::BigInt, num_bigint::BigInt) {
    use num_bigint::BigInt;
    let one = BigInt::from(1);
    match sign {
        Sign::Unsigned => (BigInt::ZERO, (one << width) - 1),
        Sign::Signed if width == 0 => (BigInt::ZERO, BigInt::from(-1)),
        Sign::Signed => {
            let half = one << (width - 1);
            (-half.clone(), half - 1)
        }
    }
}
