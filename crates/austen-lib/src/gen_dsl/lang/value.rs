use num_bigint::BigInt;
use serde::Serialize;

use crate::gen_dsl::lang::base_type::{BaseType, Sign, normalise_to_range_big};
use crate::gen_dsl::lang::id::Id;
use crate::gen_dsl::lang::index_term::{Const, IndexTerm, Term};
use crate::gen_dsl::lang::sym::Sym;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Value {
    Unit,
    Scalar(Scalar),
    Struct {
        tag: Sym,
        fields: Vec<(Id, Value)>,
    },
    Record(Vec<(Id, Value)>),
    Constructor {
        name: Sym,
        fields: Vec<(Id, Value)>,
    },
    Map {
        entries: Vec<(i128, Value)>,
        default: Option<Box<Value>>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub enum Scalar {
    Int(#[serde(serialize_with = "serialize_integer")] BigInt),
    Bits(
        Sign,
        u16,
        #[serde(serialize_with = "serialize_integer")] BigInt,
    ),
    Bool(bool),
    Pointer {
        addr: u64,
        #[serde(skip)]
        origin: PointerOrigin,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerOrigin {
    Heap,
    Global(u64),
    Opaque,
}

impl PartialEq for Scalar {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Scalar::Int(a), Scalar::Int(b)) => a == b,
            (Scalar::Bits(sa, wa, a), Scalar::Bits(sb, wb, b)) => (sa, wa, a) == (sb, wb, b),
            (Scalar::Bool(a), Scalar::Bool(b)) => a == b,
            (Scalar::Pointer { addr: a, .. }, Scalar::Pointer { addr: b, .. }) => a == b,
            _ => false,
        }
    }
}

impl Eq for Scalar {}

impl Scalar {
    pub const fn pointer(addr: u64) -> Scalar {
        Scalar::Pointer {
            addr,
            origin: PointerOrigin::Opaque,
        }
    }

    pub const fn heap_pointer(addr: u64) -> Scalar {
        Scalar::Pointer {
            addr,
            origin: PointerOrigin::Heap,
        }
    }

    pub const fn heap_address(&self) -> Option<u64> {
        match self {
            Scalar::Pointer {
                addr,
                origin: PointerOrigin::Heap,
            } if *addr != 0 => Some(*addr),
            _ => None,
        }
    }

    pub const fn global_pointer(addr: u64, sym: u64) -> Scalar {
        Scalar::Pointer {
            addr,
            origin: PointerOrigin::Global(sym),
        }
    }

    pub const fn as_pointer(&self) -> Option<(u64, PointerOrigin)> {
        match self {
            Scalar::Pointer { addr, origin } => Some((*addr, *origin)),
            _ => None,
        }
    }

    pub fn bits(sign: Sign, width: u16, v: impl Into<BigInt>) -> Option<Scalar> {
        normalize_bits(sign, width, v.into()).map(|n| Scalar::Bits(sign, width, n))
    }

    pub fn as_int(&self) -> Option<i128> {
        match self {
            Scalar::Int(n) | Scalar::Bits(_, _, n) => i128::try_from(n).ok(),
            Scalar::Bool(_) | Scalar::Pointer { .. } => None,
        }
    }

    pub fn as_integer(&self) -> Option<BigInt> {
        match self {
            Scalar::Int(n) | Scalar::Bits(_, _, n) => Some(n.clone()),
            Scalar::Bool(_) | Scalar::Pointer { .. } => None,
        }
    }
}

impl Value {
    pub fn to_literal(&self) -> Option<IndexTerm> {
        match self {
            Value::Unit => Some(IndexTerm::unit()),
            Value::Scalar(Scalar::Int(n)) => Some(IndexTerm {
                term: Term::Const(Const::Z(n.clone())),
                bt: BaseType::Integer,
            }),
            Value::Scalar(Scalar::Bits(sign, width, n)) => Some(IndexTerm {
                term: Term::Const(Const::Bits((*sign, *width), n.clone())),
                bt: BaseType::Bits(*sign, *width),
            }),
            Value::Scalar(Scalar::Bool(b)) => Some(IndexTerm::boolean(*b)),
            Value::Scalar(Scalar::Pointer { .. }) => None,
            Value::Struct { tag, fields } => {
                let members = fields
                    .iter()
                    .map(|(id, v)| Some((id.clone(), v.to_literal()?)))
                    .collect::<Option<Vec<_>>>()?;
                Some(IndexTerm::struct_(tag.clone(), members))
            }
            Value::Record(fields) => {
                let members = fields
                    .iter()
                    .map(|(id, v)| Some((id.clone(), v.to_literal()?)))
                    .collect::<Option<Vec<_>>>()?;
                IndexTerm::record(members).ok()
            }
            Value::Constructor { .. } | Value::Map { .. } => None,
        }
    }

    pub(in crate::gen_dsl) fn language_eq(&self, other: &Value) -> Option<bool> {
        match (self, other) {
            (Value::Unit, Value::Unit) => Some(true),
            (Value::Scalar(a), Value::Scalar(b)) => Some(a == b),
            (
                Value::Constructor {
                    name: left_name,
                    fields: left_fields,
                },
                Value::Constructor {
                    name: right_name,
                    fields: right_fields,
                },
            ) => {
                if left_name != right_name {
                    return Some(false);
                }
                if left_fields.len() != right_fields.len()
                    || left_fields
                        .iter()
                        .zip(right_fields)
                        .any(|((left, _), (right, _))| left != right)
                {
                    return None;
                }
                for ((_, left), (_, right)) in left_fields.iter().zip(right_fields) {
                    if !left.language_eq(right)? {
                        return Some(false);
                    }
                }
                Some(true)
            }
            (
                Value::Struct {
                    tag: left_tag,
                    fields: left_fields,
                },
                Value::Struct {
                    tag: right_tag,
                    fields: right_fields,
                },
            ) => {
                if left_tag != right_tag {
                    return None;
                }
                named_fields_eq(left_fields, right_fields)
            }
            (Value::Record(left_fields), Value::Record(right_fields)) => {
                named_fields_eq(left_fields, right_fields)
            }
            (
                Value::Map {
                    entries: left_entries,
                    default: left_default,
                },
                Value::Map {
                    entries: right_entries,
                    default: right_default,
                },
            ) => match (left_default, right_default) {
                (None, None) => Some(self == other),
                (Some(left_default), Some(right_default)) => {
                    if !left_default.language_eq(right_default)? {
                        return Some(false);
                    }
                    let mut keys: Vec<i128> = left_entries
                        .iter()
                        .chain(right_entries)
                        .map(|(key, _)| *key)
                        .collect();
                    keys.sort_unstable();
                    keys.dedup();
                    for key in keys {
                        if !self.map_get(key)?.language_eq(other.map_get(key)?)? {
                            return Some(false);
                        }
                    }
                    Some(true)
                }
                _ => None,
            },
            _ => None,
        }
    }

    pub fn as_scalar(&self) -> Option<Scalar> {
        match self {
            Value::Scalar(s) => Some(s.clone()),
            Value::Unit
            | Value::Struct { .. }
            | Value::Record(_)
            | Value::Constructor { .. }
            | Value::Map { .. } => None,
        }
    }

    pub fn field(&self, id: &Id) -> Option<&Value> {
        match self {
            Value::Struct { fields, .. } => fields.iter().find(|(f, _)| f == id).map(|(_, v)| v),
            Value::Unit
            | Value::Scalar(_)
            | Value::Record(_)
            | Value::Constructor { .. }
            | Value::Map { .. } => None,
        }
    }

    pub fn record_field(&self, id: &Id) -> Option<&Value> {
        match self {
            Value::Record(fields) => fields.iter().find(|(f, _)| f == id).map(|(_, v)| v),
            Value::Unit
            | Value::Scalar(_)
            | Value::Struct { .. }
            | Value::Constructor { .. }
            | Value::Map { .. } => None,
        }
    }

    pub fn field_mut(&mut self, id: &Id) -> Option<&mut Value> {
        match self {
            Value::Struct { fields, .. } => {
                fields.iter_mut().find(|(f, _)| f == id).map(|(_, v)| v)
            }
            Value::Unit
            | Value::Scalar(_)
            | Value::Record(_)
            | Value::Constructor { .. }
            | Value::Map { .. } => None,
        }
    }

    pub fn record_field_mut(&mut self, id: &Id) -> Option<&mut Value> {
        match self {
            Value::Record(fields) => fields.iter_mut().find(|(f, _)| f == id).map(|(_, v)| v),
            Value::Unit
            | Value::Scalar(_)
            | Value::Struct { .. }
            | Value::Constructor { .. }
            | Value::Map { .. } => None,
        }
    }

    pub fn iteration_map(entries: Vec<(i128, Value)>) -> Value {
        debug_assert!(entries.windows(2).all(|w| w[0].0 < w[1].0));
        Value::Map {
            entries,
            default: None,
        }
    }

    pub fn scalars(&self) -> impl Iterator<Item = &Scalar> {
        let mut pending = vec![self];
        std::iter::from_fn(move || {
            while let Some(value) = pending.pop() {
                match value {
                    Value::Unit => {}
                    Value::Scalar(scalar) => return Some(scalar),
                    Value::Struct { fields, .. }
                    | Value::Record(fields)
                    | Value::Constructor { fields, .. } => {
                        pending.extend(fields.iter().map(|(_, value)| value));
                    }
                    Value::Map { entries, default } => {
                        pending.extend(entries.iter().map(|(_, value)| value));
                        pending.extend(default.as_deref());
                    }
                }
            }
            None
        })
    }

    pub fn scalars_mut(&mut self) -> impl Iterator<Item = &mut Scalar> {
        let mut pending = vec![self];
        std::iter::from_fn(move || {
            while let Some(value) = pending.pop() {
                match value {
                    Value::Unit => {}
                    Value::Scalar(scalar) => return Some(scalar),
                    Value::Struct { fields, .. }
                    | Value::Record(fields)
                    | Value::Constructor { fields, .. } => {
                        pending.extend(fields.iter_mut().map(|(_, value)| value));
                    }
                    Value::Map { entries, default } => {
                        pending.extend(entries.iter_mut().map(|(_, value)| value));
                        pending.extend(default.as_deref_mut());
                    }
                }
            }
            None
        })
    }

    pub fn map_get(&self, key: i128) -> Option<&Value> {
        match self {
            Value::Map { entries, default } => entries
                .binary_search_by_key(&key, |(k, _)| *k)
                .ok()
                .map(|i| &entries[i].1)
                .or(default.as_deref()),
            Value::Unit
            | Value::Scalar(_)
            | Value::Struct { .. }
            | Value::Record(_)
            | Value::Constructor { .. } => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Scalar(Scalar::Bool(b)) => Some(*b),
            _ => None,
        }
    }

    pub fn as_integer(&self) -> Option<BigInt> {
        match self {
            Value::Scalar(scalar) => scalar.as_integer(),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i128> {
        match self {
            Value::Scalar(scalar) => scalar.as_int(),
            _ => None,
        }
    }
}

fn named_fields_eq(left: &[(Id, Value)], right: &[(Id, Value)]) -> Option<bool> {
    if left.len() != right.len()
        || has_duplicate_field(left)
        || has_duplicate_field(right)
        || left.iter().any(|(id, _)| named_field(right, id).is_none())
    {
        return None;
    }
    for (id, left_value) in left {
        if !left_value.language_eq(named_field(right, id)?)? {
            return Some(false);
        }
    }
    Some(true)
}

fn named_field<'a>(fields: &'a [(Id, Value)], id: &Id) -> Option<&'a Value> {
    fields.iter().find(|(f, _)| f == id).map(|(_, v)| v)
}

fn has_duplicate_field(fields: &[(Id, Value)]) -> bool {
    fields
        .iter()
        .enumerate()
        .any(|(at, (id, _))| named_field(&fields[..at], id).is_some())
}

impl From<Scalar> for Value {
    fn from(s: Scalar) -> Self {
        Value::Scalar(s)
    }
}

fn normalize_bits(sign: Sign, width: u16, value: BigInt) -> Option<BigInt> {
    if width == 0 {
        return None;
    }
    Some(normalise_to_range_big(sign, width, &value))
}

fn serialize_integer<S: serde::Serializer>(
    value: &BigInt,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if let Ok(value) = i64::try_from(value) {
        serializer.serialize_i64(value)
    } else if let Ok(value) = u64::try_from(value) {
        serializer.serialize_u64(value)
    } else {
        serializer.collect_str(value)
    }
}
