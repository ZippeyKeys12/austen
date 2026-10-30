use crate::gen_dsl::lang::gen_def::GenDef;
use crate::gen_dsl::lang::gen_module::call_graph_of;
use crate::gen_dsl::lang::sym::{Sym, SymSet};

pub fn prune_call_graph(defs: &mut Vec<(Sym, GenDef)>) {
    let graph = call_graph_of(defs);
    let mut reachable: SymSet = defs
        .iter()
        .filter(|(_, def)| def.spec)
        .map(|(name, _)| name.clone())
        .collect();
    let mut work: Vec<Sym> = reachable.iter().cloned().collect();
    while let Some(name) = work.pop() {
        if let Some(callees) = graph.get(&name) {
            for callee in callees {
                if reachable.insert(callee.clone()) {
                    work.push(callee.clone());
                }
            }
        }
    }
    defs.retain(|(name, _)| reachable.contains(name));
}
