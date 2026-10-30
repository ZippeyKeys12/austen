use crate::gen_dsl::{BaseType, Const, Sign};
use num_bigint::BigInt;
use num_rational::BigRational;
use z3::Model;
use z3::ast::{Ast, BV, Int, Real};

use super::error::SmtError;
use super::translate::SmtExpr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelValue {
    Bool(bool),
    Int(BigInt),
    Real(BigRational),
    Null,
    Bits {
        sign: Sign,
        width: u16,
        unsigned: BigInt,
    },
    Pointer {
        alloc_id: Option<BigInt>,
        addr: u64,
    },
    AllocId(BigInt),
    Unit,
}

impl ModelValue {
    pub fn signed_value(&self) -> Option<BigInt> {
        match self {
            ModelValue::Bits {
                sign,
                width,
                unsigned,
            } => Some(crate::gen_dsl::normalise_to_range_big(
                *sign, *width, unsigned,
            )),
            _ => None,
        }
    }

    pub fn choice_bits(&self) -> Option<(u16, BigInt)> {
        match self {
            ModelValue::Bits {
                width, unsigned, ..
            } => Some((*width, unsigned.clone())),
            ModelValue::Pointer { addr, .. } => Some((64, BigInt::from(*addr))),
            ModelValue::Null => Some((64, BigInt::ZERO)),
            _ => None,
        }
    }

    pub fn to_const(&self) -> Const {
        match self {
            ModelValue::Bool(b) => Const::Bool(*b),
            ModelValue::Int(n) => Const::Z(n.clone()),
            ModelValue::Real(q) => Const::Q(q.clone()),
            ModelValue::Bits { sign, width, .. } => Const::Bits(
                (*sign, *width),
                self.signed_value().expect("a Bits has a signed value"),
            ),
            ModelValue::Null => Const::Null,
            ModelValue::Pointer { alloc_id, addr } => Const::Pointer {
                alloc_id: alloc_id.clone().unwrap_or(BigInt::ZERO),
                addr: BigInt::from(*addr),
            },
            ModelValue::AllocId(n) => Const::AllocId(n.clone()),
            ModelValue::Unit => Const::Unit,
        }
    }
}

pub(in crate::gen_dsl) fn decode(model: &Model, expr: &SmtExpr) -> Result<ModelValue, SmtError> {
    let bt = expr.bt().clone();
    let fail = |why: String| SmtError::ModelDecode {
        bt: bt.clone(),
        why,
    };
    match &bt {
        BaseType::Bool => {
            let b = expr
                .ast()
                .as_bool()
                .ok_or_else(|| fail("not a boolean-sorted term".into()))?;
            let v = evaluated(model, &b).ok_or_else(|| fail(NO_VALUE.into()))?;
            Ok(ModelValue::Bool(
                v.as_bool()
                    .ok_or_else(|| fail("not a boolean literal".into()))?,
            ))
        }
        BaseType::Integer => {
            let i = expr
                .ast()
                .as_int()
                .ok_or_else(|| fail("not an integer-sorted term".into()))?;
            let v = evaluated(model, &i).ok_or_else(|| fail(NO_VALUE.into()))?;
            Ok(ModelValue::Int(int_value(&v).ok_or_else(|| {
                fail(format!("cannot read {v} as an integer"))
            })?))
        }
        BaseType::Real => {
            let r = expr
                .ast()
                .as_real()
                .ok_or_else(|| fail("not a real-sorted term".into()))?;
            let v = evaluated(model, &r).ok_or_else(|| fail(NO_VALUE.into()))?;
            Ok(ModelValue::Real(real_value(&v).ok_or_else(|| {
                fail(format!("cannot read {v} as a rational"))
            })?))
        }
        BaseType::Bits(sign, width) => {
            let bv = expr
                .ast()
                .as_bv()
                .ok_or_else(|| fail("not a bitvector-sorted term".into()))?;
            let unsigned = bv_value(model, &bv, *width)
                .ok_or_else(|| fail("the model had no value for some slice".into()))?;
            Ok(ModelValue::Bits {
                sign: *sign,
                width: *width,
                unsigned,
            })
        }
        BaseType::Loc(_) => {
            let bv = expr
                .ast()
                .as_bv()
                .ok_or_else(|| fail("not a bitvector-sorted term".into()))?;
            let unsigned = bv_value(model, &bv, 64).ok_or_else(|| fail(NO_VALUE.into()))?;
            let addr = u64::try_from(&unsigned).expect("a 64-bit read fits u64");
            Ok(ModelValue::Pointer {
                alloc_id: None,
                addr,
            })
        }
        BaseType::AllocId => {
            let i = expr
                .ast()
                .as_int()
                .ok_or_else(|| fail("not an integer-sorted term".into()))?;
            let v = evaluated(model, &i).ok_or_else(|| fail(NO_VALUE.into()))?;
            Ok(ModelValue::AllocId(int_value(&v).ok_or_else(|| {
                fail(format!("cannot read {v} as an allocation id"))
            })?))
        }
        BaseType::Unit => Ok(ModelValue::Unit),
        other => Err(SmtError::ModelDecode {
            bt: other.clone(),
            why: "only scalars decode; aggregates have no `ModelValue` yet".to_owned(),
        }),
    }
}

fn bv_value(model: &Model, bv: &BV, width: u16) -> Option<BigInt> {
    if width <= 64 {
        return Some(BigInt::from(evaluated(model, bv)?.as_u64()?));
    }
    let mut acc = BigInt::ZERO;
    let mut low: u32 = 0;
    let width = u32::from(width);
    while low < width {
        let high = (low + 64).min(width) - 1;
        let value = evaluated(model, &bv.extract(high, low))?;
        acc |= BigInt::from(value.as_u64()?) << low;
        low += 64;
    }
    Some(acc)
}

fn evaluated<T: Ast>(model: &Model, ast: &T) -> Option<T> {
    model.eval(ast, true).map(|v| v.simplify())
}

const NO_VALUE: &str = "the model had no value";

fn int_value(v: &Int) -> Option<BigInt> {
    if let Some(n) = v.as_i64() {
        return Some(BigInt::from(n));
    }
    parse_signed(&v.to_string())
}

fn real_value(v: &Real) -> Option<BigRational> {
    if let Some((n, d)) = v.as_rational()
        && d != 0
    {
        return Some(BigRational::new(BigInt::from(n), BigInt::from(d)));
    }
    parse_z3_real(&v.to_string())
}

fn parse_z3_real(text: &str) -> Option<BigRational> {
    let first_digit = text.find(|c: char| c.is_ascii_digit())?;
    let negative = text[..first_digit].contains('-');
    let parts = scan_decimals(text)?;
    let zero = BigRational::from_integer(BigInt::ZERO);
    let q = match parts.as_slice() {
        [x] => x.clone(),
        [n, d] if *d != zero => n / d,
        _ => return None,
    };
    Some(if negative { -q } else { q })
}

fn scan_decimals(text: &str) -> Option<Vec<BigRational>> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        let integral: String = chars[start..i].iter().collect();
        let mut fractional = String::new();
        if i < chars.len() && chars[i] == '.' {
            let from = i + 1;
            let mut j = from;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            if j > from {
                fractional = chars[from..j].iter().collect();
                i = j;
            }
        }
        let scale = BigInt::from(10).pow(u32::try_from(fractional.len()).ok()?);
        let numerator = format!("{integral}{fractional}").parse::<BigInt>().ok()?;
        out.push(BigRational::new(numerator, scale));
    }
    Some(out)
}

fn parse_signed(text: &str) -> Option<BigInt> {
    let negative = text.contains('-');
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let n = digits.parse::<BigInt>().ok()?;
    Some(if negative { -n } else { n })
}
