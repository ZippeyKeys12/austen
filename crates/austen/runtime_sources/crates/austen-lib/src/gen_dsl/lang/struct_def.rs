use serde::{Deserialize, Serialize};

use crate::gen_dsl::lang::id::Id;
use crate::gen_dsl::lang::sctype::Sctype;
use crate::gen_dsl::lang::sym::Sym;

pub const MAX_STRUCT_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructDef {
    pub members: Vec<(Id, Sctype)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructLayout {
    pub offsets: Vec<u64>,
    pub sizes: Vec<u64>,
    pub size: u64,
    pub align: u64,
}

impl StructDef {
    pub fn layout_in(&self, structs: &[(Sym, StructDef)]) -> Option<StructLayout> {
        self.layout_at(structs, 0)
    }

    fn layout_at(&self, structs: &[(Sym, StructDef)], depth: usize) -> Option<StructLayout> {
        if depth >= MAX_STRUCT_DEPTH {
            return None;
        }
        let mut offsets = Vec::with_capacity(self.members.len());
        let mut sizes = Vec::with_capacity(self.members.len());
        let mut end: u64 = 0;
        let mut align: u64 = 1;
        for (_, ct) in &self.members {
            let (member_size, member_align) = size_align_at(ct, structs, depth)?;
            let offset = align_up(end, member_align)?;
            offsets.push(offset);
            sizes.push(member_size);
            end = offset.checked_add(member_size)?;
            align = align.max(member_align);
        }
        Some(StructLayout {
            offsets,
            sizes,
            size: align_up(end, align)?,
            align,
        })
    }
}

pub(in crate::gen_dsl) fn size_at(
    ct: &Sctype,
    structs: &[(Sym, StructDef)],
    depth: usize,
) -> Option<u64> {
    Some(size_align_at(ct, structs, depth)?.0)
}

fn size_align_at(ct: &Sctype, structs: &[(Sym, StructDef)], depth: usize) -> Option<(u64, u64)> {
    if depth >= MAX_STRUCT_DEPTH {
        return None;
    }
    match ct {
        Sctype::Struct(tag) => {
            let def = find(structs, tag)?;
            let mut end: u64 = 0;
            let mut align: u64 = 1;
            for (_, member) in &def.members {
                let (member_size, member_align) = size_align_at(member, structs, depth + 1)?;
                end = align_up(end, member_align)?.checked_add(member_size)?;
                align = align.max(member_align);
            }
            Some((align_up(end, align)?, align))
        }
        Sctype::Array(elem, n) => {
            let (elem_size, align) = size_align_at(elem, structs, depth)?;
            let size = match n {
                Some(n) => elem_size.checked_mul(u64::try_from(*n).ok()?)?,
                None => return Some((ct.byte_size()?, align)),
            };
            Some((size, align))
        }
        _ => ct.byte_size().map(|size| (size, size)),
    }
}

pub(in crate::gen_dsl) fn align_at(
    ct: &Sctype,
    structs: &[(Sym, StructDef)],
    depth: usize,
) -> Option<u64> {
    Some(size_align_at(ct, structs, depth)?.1)
}

fn find<'a>(structs: &'a [(Sym, StructDef)], tag: &Sym) -> Option<&'a StructDef> {
    structs.iter().find(|(t, _)| t == tag).map(|(_, d)| d)
}

fn align_up(n: u64, align: u64) -> Option<u64> {
    crate::gen_dsl::Congruence::aligned(align)?.first_at_or_after(n)
}
