use crate::gen_dsl::lang::gen_def::GenDef;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::index_term::IndexTerm;
use crate::gen_dsl::lang::sym::{Sym, SymMap};

fn get_change(def: &GenDef) -> Vec<usize> {
    if def.c_types.is_some() {
        return Vec::new();
    }
    let free = def.body.free_vars();
    def.iargs
        .iter()
        .enumerate()
        .filter(|(_, (x, _))| !free.contains(x))
        .map(|(i, _)| i)
        .collect()
}

fn get_changes(defs: &[(Sym, GenDef)]) -> SymMap<Vec<usize>> {
    defs.iter()
        .map(|(_, def)| (def.name.clone(), get_change(def)))
        .collect()
}

fn apply_changes_tm(changes: &SymMap<Vec<usize>>, tm: GenTerm) -> GenTerm {
    let out: Result<GenTerm, std::convert::Infallible> = tm.map_pre(&mut |node| {
        let (term, bt) = node.into_parts();
        let GenTermKind::Call { fsym, args } = term else {
            return Ok(GenTerm::from_parts(term, bt));
        };
        let args = match changes.get(&fsym) {
            None => args,
            Some(remove) => args
                .into_iter()
                .enumerate()
                .filter(|(i, _)| !remove.contains(i))
                .map(|(_, a)| a)
                .collect(),
        };
        Ok(GenTerm::from_parts(GenTermKind::Call { fsym, args }, bt))
    });
    out.unwrap()
}

fn apply_changes(changes: &SymMap<Vec<usize>>, defs: &mut [(Sym, GenDef)]) {
    for (_, def) in defs.iter_mut() {
        if let Some(remove) = changes.get(&def.name)
            && !remove.is_empty()
        {
            def.iargs = std::mem::take(&mut def.iargs)
                .into_iter()
                .enumerate()
                .filter(|(i, _)| !remove.contains(i))
                .map(|(_, arg)| arg)
                .collect();
        }
        let body = std::mem::replace(&mut def.body, GenTerm::return_(IndexTerm::unit()));
        def.body = apply_changes_tm(changes, body);
    }
}

pub fn prune_args(defs: &mut [(Sym, GenDef)]) {
    loop {
        let changes = get_changes(defs);
        if changes.values().all(|v| v.is_empty()) {
            break;
        }
        apply_changes(&changes, defs);
    }
}
