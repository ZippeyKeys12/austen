use crate::gen_dsl::lang::gen_def::GenDef;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::sym::{Sym, SymGen, SymMap};
use crate::gen_dsl::passes::inline_gen::splice;

pub fn unfold(defs: &mut [(Sym, GenDef)], max: usize, syms: &mut SymGen) {
    if max == 0 {
        return;
    }
    let lookup: SymMap<GenDef> = defs
        .iter()
        .map(|(name, def)| (name.clone(), def.clone()))
        .collect();
    for (_, def) in defs.iter_mut() {
        def.body = walk(&lookup, max, 0, def.body.clone(), syms);
    }
}

fn walk(
    lookup: &SymMap<GenDef>,
    max: usize,
    depth: usize,
    term: GenTerm,
    syms: &mut SymGen,
) -> GenTerm {
    let (term, bt) = term.into_parts();
    match term {
        GenTermKind::Arbitrary | GenTermKind::Return(_) => GenTerm::from_parts(term, bt),
        GenTermKind::WriteChoice { label, value, rest } => {
            GenTerm::write_choice(label, value, walk(lookup, max, depth, *rest, syms))
        }
        GenTermKind::Call { fsym, args } => {
            if depth < max
                && let Some(gd) = lookup.get(&fsym)
            {
                let body = splice(gd, &args, syms);
                return walk(lookup, max, depth + 1, body, syms);
            }
            GenTerm::from_parts(GenTermKind::Call { fsym, args }, bt)
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => GenTerm::asgn(addr, ct, value, walk(lookup, max, depth, *rest, syms)),
        GenTermKind::LetStar { var, value, rest } => GenTerm::let_star(
            (var, walk(lookup, max, depth, *value, syms)),
            walk(lookup, max, depth, *rest, syms),
        ),
        GenTermKind::Assert { constraint, rest } => {
            GenTerm::assert(constraint, walk(lookup, max, depth, *rest, syms))
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            let then_ = walk(lookup, max, depth, *then_, syms);
            let else_ = walk(lookup, max, depth, *else_, syms);
            GenTerm::rebuild_ite(cond, then_, else_)
        }
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => GenTerm::map((var, var_bt, perm), walk(lookup, max, depth, *body, syms)),
        GenTermKind::Scope { name, body } => {
            GenTerm::scope(name, walk(lookup, max, depth, *body, syms))
        }
    }
}
