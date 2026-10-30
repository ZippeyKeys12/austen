use crate::gen_dsl::lang::gen_def::GenDef;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::index_term::IndexTerm;
use crate::gen_dsl::lang::make_term::uintptr_bt;
use crate::gen_dsl::lang::sym::{Sym, SymGen, SymMap};

struct Sig {
    base: Sym,
    size: Sym,
}

pub fn heap_args(defs: &mut [(Sym, GenDef)], syms: &mut SymGen) {
    let sigs: SymMap<Sig> = defs
        .iter()
        .map(|(name, def)| {
            debug_assert!(
                def.heap_base_arg.is_none() && def.heap_size_arg.is_none(),
                "heap_args is one-shot; {name:?} already carries heap arguments"
            );
            let sig = Sig {
                base: syms.fresh("heap_base"),
                size: syms.fresh("heap_size"),
            };
            (name.clone(), sig)
        })
        .collect();

    for (name, def) in defs.iter_mut() {
        let own = &sigs[name];
        let body = std::mem::replace(&mut def.body, GenTerm::return_(IndexTerm::unit()));
        def.body = heap_calls(own, &sigs, body);
        def.iargs.push((own.base.clone(), uintptr_bt()));
        def.iargs.push((own.size.clone(), uintptr_bt()));
        def.heap_base_arg = Some(own.base.clone());
        def.heap_size_arg = Some(own.size.clone());
    }
}

fn heap_calls(own: &Sig, sigs: &SymMap<Sig>, gt: GenTerm) -> GenTerm {
    let out: Result<GenTerm, std::convert::Infallible> = gt.map_pre(&mut |node| {
        let (term, bt) = node.into_parts();
        let GenTermKind::Call { fsym, mut args } = term else {
            return Ok(GenTerm::from_parts(term, bt));
        };
        if sigs.contains_key(&fsym) {
            args.push(heap_var(&own.base));
            args.push(heap_var(&own.size));
        }
        Ok(GenTerm::from_parts(GenTermKind::Call { fsym, args }, bt))
    });
    let Ok(gt) = out;
    gt
}

fn heap_var(sym: &Sym) -> IndexTerm {
    IndexTerm::sym(sym.clone(), uintptr_bt())
}
