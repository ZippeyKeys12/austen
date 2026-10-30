use crate::gen_dsl::{GenDef, GenTerm, GenTermKind, IndexTerm, ScopeName, Sym, SymGen};

use super::choice_effects::ChoiceEffects;
use super::wrap_scopes;

pub fn normalize_scopes(defs: &mut [(Sym, GenDef)], syms: &mut SymGen) {
    let effects = ChoiceEffects::analyze(defs);
    for (_, def) in defs.iter_mut() {
        let body = std::mem::replace(&mut def.body, unit_return());
        def.body = sink(Vec::new(), body, &effects, syms);
    }
}

fn sink(
    active: Vec<ScopeName>,
    term: GenTerm,
    effects: &ChoiceEffects,
    syms: &mut SymGen,
) -> GenTerm {
    let (term, bt) = term.into_parts();
    match term {
        GenTermKind::Arbitrary | GenTermKind::Return(_) => GenTerm::from_parts(term, bt),

        GenTermKind::Call { fsym, args } => {
            let call = GenTerm::call(fsym.clone(), args, bt);
            if effects.callee(&fsym).may_write_choice() {
                wrap(&active, call)
            } else {
                call
            }
        }

        GenTermKind::WriteChoice { label, value, rest } if active.is_empty() => {
            GenTerm::write_choice(label, value, sink(active, *rest, effects, syms))
        }

        GenTermKind::WriteChoice { label, value, rest } if is_unit_return(&rest) => {
            wrap(&active, GenTerm::write_choice(label, value, *rest))
        }

        GenTermKind::WriteChoice { label, value, rest } => {
            let result = syms.fresh("scope_write");
            let fragment = wrap(&active, GenTerm::write_choice(label, value, unit_return()));
            let after = sink(active, *rest, effects, syms);
            GenTerm::let_star((result, fragment), after)
        }

        GenTermKind::Scope { name, body } => {
            if effects.may_write_choice(&body) {
                let mut nested = active;
                nested.push(name);
                sink(nested, *body, effects, syms)
            } else {
                sink(active, *body, effects, syms)
            }
        }

        GenTermKind::Assert { constraint, rest } => {
            GenTerm::assert(constraint, sink(active, *rest, effects, syms))
        }

        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => GenTerm::asgn(addr, ct, value, sink(active, *rest, effects, syms)),

        GenTermKind::LetStar { var, value, rest } if crosses_binding(&active, &var) => {
            let rebuilt = GenTerm::let_star(
                (var, sink(Vec::new(), *value, effects, syms)),
                sink(Vec::new(), *rest, effects, syms),
            );
            wrap(&active, rebuilt)
        }

        GenTermKind::LetStar { var, value, rest } => GenTerm::let_star(
            (var, sink(active.clone(), *value, effects, syms)),
            sink(active, *rest, effects, syms),
        ),

        GenTermKind::Ite { cond, then_, else_ } => GenTerm::ite(
            cond,
            sink(active.clone(), *then_, effects, syms),
            sink(active, *else_, effects, syms),
        ),

        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } if crosses_binding(&active, &var) => {
            let rebuilt = GenTerm::map((var, var_bt, perm), sink(Vec::new(), *body, effects, syms));
            wrap(&active, rebuilt)
        }

        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => GenTerm::map((var, var_bt, perm), sink(active, *body, effects, syms)),
    }
}

fn crosses_binding(active: &[ScopeName], var: &Sym) -> bool {
    active.iter().any(|name| name.free_vars().contains(var))
}

fn wrap(active: &[ScopeName], body: GenTerm) -> GenTerm {
    wrap_scopes(active.to_vec(), body)
}

fn unit_return() -> GenTerm {
    GenTerm::return_(IndexTerm::unit())
}

fn is_unit_return(term: &GenTerm) -> bool {
    matches!(&term.term, GenTermKind::Return(it) if *it == IndexTerm::unit())
}
