use std::collections::BTreeSet;

use crate::gen_dsl::{BaseType, Const, IndexTerm, LogicalFunctionDef, Sym, Term};

pub fn input_default_types(
    term: &IndexTerm,
    functions: &[(Sym, LogicalFunctionDef)],
) -> BTreeSet<BaseType> {
    let mut types = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut pending = vec![term];
    while let Some(term) = pending.pop() {
        let mut callees = Vec::new();
        let _ = term.clone().map_post(&mut |node| {
            match &node.term {
                Term::Const(Const::InputDefault(ty)) => {
                    types.insert(ty.clone());
                }
                Term::Apply(name, _) if seen.insert(name.clone()) => {
                    callees.push(name.clone());
                }
                _ => {}
            }
            node
        });
        for callee in callees {
            if let Some(body) = functions
                .iter()
                .find(|(name, _)| name == &callee)
                .and_then(|(_, function)| function.body.as_ref())
            {
                pending.push(body.term());
            }
        }
    }
    types
}
