use crate::gen_dsl::{
    BaseType, GenTerm, GenTermKind, Id, IndexTerm, Sctype, StructDef, Sym, SymGen, uintptr_bt,
};

use super::TransformError;

pub fn destruct_arbitrary(
    term: GenTerm,
    structs: &[(Sym, StructDef)],
    syms: &mut SymGen,
) -> Result<GenTerm, TransformError> {
    term.map_pre(&mut |node| {
        let GenTermKind::Arbitrary = &node.term else {
            return Ok(node);
        };
        let BaseType::Struct(tag) = &node.bt else {
            return Ok(node);
        };
        expand_struct(tag, structs, syms)
    })
}

fn base_type_of(ct: &Sctype) -> Result<BaseType, TransformError> {
    ct.to_base_type()
        .ok_or_else(|| TransformError::UnsupportedSctype { ct: ct.clone() })
}

fn expand_struct(
    tag: &Sym,
    structs: &[(Sym, StructDef)],
    syms: &mut SymGen,
) -> Result<GenTerm, TransformError> {
    let members = &structs
        .iter()
        .find(|(name, _)| name == tag)
        .ok_or_else(|| TransformError::UnknownStruct { tag: tag.clone() })?
        .1
        .members;

    let binders: Vec<(Sym, Id, Sctype)> = members
        .iter()
        .map(|(id, ct)| (syms.fresh_anon(), id.clone(), ct.clone()))
        .collect();

    let mut fields = Vec::with_capacity(binders.len());
    for (sym, id, ct) in &binders {
        fields.push((id.clone(), IndexTerm::sym(sym.clone(), base_type_of(ct)?)));
    }
    let assembled = GenTerm::return_(IndexTerm::struct_(tag.clone(), fields));

    binders
        .into_iter()
        .try_fold(assembled, |body, (sym, _, ct)| {
            Ok(GenTerm::let_star(
                (sym, arbitrary_of_sctype(&ct, structs, syms)?),
                body,
            ))
        })
}

fn arbitrary_of_sctype(
    ct: &Sctype,
    structs: &[(Sym, StructDef)],
    syms: &mut SymGen,
) -> Result<GenTerm, TransformError> {
    match ct {
        Sctype::Array(elem, Some(len)) => {
            let index = syms.fresh_anon();
            let bt = uintptr_bt();
            let bound = |n: u128| {
                IndexTerm::num_lit(n.into(), &bt)
                    .ok_or(TransformError::ArrayLenOutOfRange { len: *len })
            };
            let var = IndexTerm::sym(index.clone(), bt.clone());
            let permission = IndexTerm::and2(
                IndexTerm::le(bound(0)?, var.clone()),
                IndexTerm::lt(var, bound(u128::try_from(*len).unwrap_or(u128::MAX))?),
            );
            let body = arbitrary_of_sctype(elem, structs, syms)?;
            Ok(GenTerm::map((index, bt, permission), body))
        }
        Sctype::Array(_, None) => Err(TransformError::UnsizedArray),
        Sctype::Struct(tag) => expand_struct(tag, structs, syms),
        other => Ok(GenTerm::arbitrary(base_type_of(other)?)),
    }
}
