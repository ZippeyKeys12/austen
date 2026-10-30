use crate::gen_dsl::lang::base_type::Sign;
use crate::gen_dsl::lang::id::Id;
use crate::gen_dsl::lang::sctype::{IntegerType, Sctype};
use crate::gen_dsl::lang::struct_def::StructDef;
use crate::gen_dsl::lang::sym::Sym;
use crate::gen_dsl::lang::value::{Scalar, Value};

pub fn value_bytes(value: &Value, ct: &Sctype, structs: &[(Sym, StructDef)]) -> Option<Vec<u8>> {
    match ct {
        Sctype::Integer(IntegerType::Bool) => {
            let byte = match value.as_scalar()? {
                Scalar::Bool(b) => u8::from(b),
                Scalar::Int(n) | Scalar::Bits(_, _, n) => {
                    u8::try_from(n).ok().filter(|n| *n <= 1)?
                }
                _ => return None,
            };
            Some(vec![byte])
        }
        Sctype::Integer(_) | Sctype::Byte => {
            let size = ct.byte_size_in(structs)?;
            let Value::Scalar(Scalar::Int(v) | Scalar::Bits(_, _, v)) = value else {
                return None;
            };
            let v = i128::try_from(v).ok()?;
            let bits = 8 * u32::try_from(size).ok()?;
            let lo = -(1i128 << (bits - 1));
            let hi = 1i128 << bits;
            if v < lo || v >= hi {
                return None;
            }
            let raw = (v as u128) & ((1u128 << bits) - 1);
            Some(raw.to_le_bytes()[..size as usize].to_vec())
        }
        Sctype::Pointer(_) => match value.as_scalar()? {
            Scalar::Pointer { addr, .. } => Some(addr.to_le_bytes().to_vec()),
            _ => None,
        },
        Sctype::Struct(tag) => {
            let Value::Struct { tag: vtag, fields } = value else {
                return None;
            };
            if vtag != tag {
                return None;
            }
            let def = structs.iter().find(|(s, _)| s == tag).map(|(_, d)| d)?;
            let layout = def.layout_in(structs)?;
            if fields.len() != def.members.len() {
                return None;
            }
            let mut buf = vec![0u8; usize::try_from(layout.size).ok()?];
            for ((id, member_ct), offset) in def.members.iter().zip(layout.offsets.iter()) {
                let member = field_value(fields, id)?;
                let rendered = value_bytes(member, member_ct, structs)?;
                let at = usize::try_from(*offset).ok()?;
                let end = at.checked_add(rendered.len())?;
                buf.get_mut(at..end)?.copy_from_slice(&rendered);
            }
            Some(buf)
        }
        Sctype::Array(element, Some(length)) => {
            if !matches!(value, Value::Map { .. }) {
                return None;
            }
            let element_size = usize::try_from(element.byte_size_in(structs)?).ok()?;
            let mut buf = Vec::with_capacity(element_size.checked_mul(*length)?);
            for index in 0..*length {
                let rendered = value_bytes(value.map_get(index as i128)?, element, structs)?;
                if rendered.len() != element_size {
                    return None;
                }
                buf.extend_from_slice(&rendered);
            }
            Some(buf)
        }
        Sctype::Array(_, None) | Sctype::Void | Sctype::Function { .. } => None,
    }
}

pub fn array_value(
    bytes: &[u8],
    element_size: usize,
    length: usize,
    mut element: impl FnMut(&[u8]) -> Option<Value>,
) -> Option<Value> {
    if bytes.len() != element_size.checked_mul(length)? {
        return None;
    }
    let entries = (0..length)
        .map(|index| {
            let at = index * element_size;
            Some((index as i128, element(bytes.get(at..at + element_size)?)?))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Value::iteration_map(entries))
}

fn field_value<'a>(fields: &'a [(Id, Value)], id: &Id) -> Option<&'a Value> {
    fields.iter().find(|(f, _)| f == id).map(|(_, v)| v)
}

pub fn value_from_bytes(bytes: &[u8], ct: &Sctype, structs: &[(Sym, StructDef)]) -> Option<Value> {
    if bytes.len() as u64 != ct.byte_size_in(structs)? {
        return None;
    }
    match ct {
        Sctype::Integer(IntegerType::Bool) => match bytes {
            [0] => Some(Value::Scalar(Scalar::Bool(false))),
            [1] => Some(Value::Scalar(Scalar::Bool(true))),
            _ => None,
        },
        Sctype::Integer(ity) => {
            let signed = ity.is_signed()?;
            Some(Value::Scalar(decode_int(bytes, signed)?))
        }
        Sctype::Byte => Some(Value::Scalar(decode_int(bytes, false)?)),
        Sctype::Pointer(_) => {
            let addr = u64::from_le_bytes(bytes.try_into().ok()?);
            Some(Value::Scalar(Scalar::pointer(addr)))
        }
        Sctype::Struct(tag) => {
            let def = structs.iter().find(|(s, _)| s == tag).map(|(_, d)| d)?;
            let layout = def.layout_in(structs)?;
            let fields = def
                .members
                .iter()
                .zip(layout.offsets.iter())
                .map(|((id, member_ct), offset)| {
                    let at = usize::try_from(*offset).ok()?;
                    let size = usize::try_from(member_ct.byte_size_in(structs)?).ok()?;
                    let member = value_from_bytes(bytes.get(at..at + size)?, member_ct, structs)?;
                    Some((id.clone(), member))
                })
                .collect::<Option<Vec<_>>>()?;
            Some(Value::Struct {
                tag: tag.clone(),
                fields,
            })
        }
        Sctype::Array(element, Some(length)) => {
            let element_size = usize::try_from(element.byte_size_in(structs)?).ok()?;
            array_value(bytes, element_size, *length, |element_bytes| {
                value_from_bytes(element_bytes, element, structs)
            })
        }
        Sctype::Array(_, None) | Sctype::Void | Sctype::Function { .. } => None,
    }
}

fn decode_int(bytes: &[u8], signed: bool) -> Option<Scalar> {
    if bytes.is_empty() || bytes.len() > 8 {
        return None;
    }
    let mut raw = [0u8; 16];
    raw[..bytes.len()].copy_from_slice(bytes);
    let unsigned = u128::from_le_bytes(raw);
    let bits = 8 * bytes.len() as u16;
    let sign = if signed { Sign::Signed } else { Sign::Unsigned };
    Scalar::bits(sign, bits, unsigned as i128)
}
