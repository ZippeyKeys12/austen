use crate::gen_dsl::{
    BaseType, GenDef, GenTerm, GenTermKind, IndexTerm, Sctype, StructDef, Sym, SymMap, Term, reduce,
};

type Claims = SymMap<Vec<Option<Sctype>>>;

pub fn annotate_pointees(defs: &mut [(Sym, GenDef)], structs: &[(Sym, StructDef)]) {
    let claims = claim_table(defs, structs);
    for (_, def) in defs.iter_mut() {
        let claimed = claimed_pointers(&def.body, &claims, structs);
        annotate(&mut def.body, &claimed);
    }
}

fn claim_table(defs: &[(Sym, GenDef)], structs: &[(Sym, StructDef)]) -> Claims {
    let mut claims: Claims = defs
        .iter()
        .map(|(name, def)| (name.clone(), vec![None; def.iargs.len()]))
        .collect();
    let rounds = defs.iter().map(|(_, def)| def.iargs.len()).sum::<usize>() + 1;
    for _ in 0..rounds {
        let mut changed = false;
        for (name, def) in defs {
            let claimed = claimed_pointers(&def.body, &claims, structs);
            let Some(slots) = claims.get_mut(name) else {
                continue;
            };
            for (slot, (param, bt)) in slots.iter_mut().zip(&def.iargs) {
                if !bt.is_loc() {
                    continue;
                }
                if let Some(ct) = claimed.get(param) {
                    changed |= widen(slot, ct, structs);
                }
            }
        }
        if !changed {
            break;
        }
    }
    claims
}

fn widen(slot: &mut Option<Sctype>, ct: &Sctype, structs: &[(Sym, StructDef)]) -> bool {
    let size = |ct: &Sctype| ct.byte_size_in(structs).unwrap_or(0);
    match slot {
        Some(have) if size(have) >= size(ct) => false,
        _ => {
            *slot = Some(ct.clone());
            true
        }
    }
}

fn claimed_pointers(
    body: &GenTerm,
    claims: &Claims,
    structs: &[(Sym, StructDef)],
) -> SymMap<Sctype> {
    let mut env = SymMap::new();
    let mut out = SymMap::new();
    collect(body, claims, structs, &mut env, &mut out);
    out
}

fn collect(
    term: &GenTerm,
    claims: &Claims,
    structs: &[(Sym, StructDef)],
    env: &mut SymMap<IndexTerm>,
    out: &mut SymMap<Sctype>,
) {
    let record = |target: Option<Sym>, ct: &Sctype, out: &mut SymMap<Sctype>| {
        let Some(target) = target else { return };
        if ct.byte_size_in(structs).is_none() {
            return;
        }
        let mut slot = out.get(&target).cloned();
        if widen(&mut slot, ct, structs) {
            out.insert(target, slot.expect("a widened slot holds a type"));
        }
    };
    match &term.term {
        GenTermKind::LetStar { var, value, rest } => {
            collect(value, claims, structs, env, out);
            if let Some(it) = result_term(value) {
                env.insert(var.clone(), it.clone());
            }
            collect(rest, claims, structs, env, out);
        }
        GenTermKind::Asgn { addr, ct, rest, .. } => {
            record(reduce_to_sym(addr, env), ct, out);
            collect(rest, claims, structs, env, out);
        }
        GenTermKind::Call { fsym, args } => {
            if let Some(params) = claims.get(fsym) {
                for (arg, claimed) in args.iter().zip(params) {
                    if let Some(ct) = claimed {
                        record(reduce_to_sym(arg, env), ct, out);
                    }
                }
            }
        }
        GenTermKind::Assert { rest, .. }
        | GenTermKind::WriteChoice { rest, .. }
        | GenTermKind::Scope { body: rest, .. }
        | GenTermKind::Map { body: rest, .. } => collect(rest, claims, structs, env, out),
        GenTermKind::Ite { then_, else_, .. } => {
            collect(then_, claims, structs, env, out);
            collect(else_, claims, structs, env, out);
        }
        GenTermKind::Arbitrary | GenTermKind::Return(_) => {}
    }
}

fn result_term(term: &GenTerm) -> Option<&IndexTerm> {
    match &term.term {
        GenTermKind::Return(it) => Some(it),
        GenTermKind::LetStar { rest, .. }
        | GenTermKind::Asgn { rest, .. }
        | GenTermKind::Assert { rest, .. }
        | GenTermKind::WriteChoice { rest, .. }
        | GenTermKind::Scope { body: rest, .. } => result_term(rest),
        _ => None,
    }
}

fn reduce_to_sym(it: &IndexTerm, env: &SymMap<IndexTerm>) -> Option<Sym> {
    match reduce(it, env).into_parts().0 {
        Term::Sym(sym) => Some(sym),
        _ => None,
    }
}

fn annotate(term: &mut GenTerm, claimed: &SymMap<Sctype>) {
    match &mut term.term {
        GenTermKind::LetStar { var, value, rest } => {
            if matches!(value.term, GenTermKind::Arbitrary)
                && value.bt == BaseType::Loc(None)
                && let Some(ct) = claimed.get(var)
            {
                value.bt = BaseType::loc_to(ct.clone());
            }
            annotate(value, claimed);
            annotate(rest, claimed);
        }
        GenTermKind::Asgn { rest, .. }
        | GenTermKind::WriteChoice { rest, .. }
        | GenTermKind::Assert { rest, .. }
        | GenTermKind::Scope { body: rest, .. }
        | GenTermKind::Map { body: rest, .. } => annotate(rest, claimed),
        GenTermKind::Ite { then_, else_, .. } => {
            annotate(then_, claimed);
            annotate(else_, claimed);
        }
        GenTermKind::Arbitrary | GenTermKind::Return(_) | GenTermKind::Call { .. } => {}
    }
}
