use crate::gen_dsl::{GenFailure, Sctype, StructDef, Sym};

pub const DEFAULT_POINTEE_BYTES: u64 = 8;

pub fn pointee_bytes(
    pointee: Option<&Sctype>,
    structs: &[(Sym, StructDef)],
    fallback: u64,
) -> Result<u64, GenFailure> {
    match pointee {
        Some(ct) => ct
            .byte_size_in(structs)
            .ok_or_else(|| GenFailure::UnsizedCtype { ct: ct.clone() }),
        None => Ok(fallback.max(1)),
    }
}
