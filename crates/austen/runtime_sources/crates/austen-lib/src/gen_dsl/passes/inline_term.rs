use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::index_term::Term;
use crate::gen_dsl::lang::subst::Substitution;
use crate::gen_dsl::lang::sym::{Sym, SymGen, SymMap, SymSet};

fn of_symset(s: SymSet) -> SymMap<bool> {
    s.into_iter().map(|x| (x, false)).collect()
}

fn union(a: SymMap<bool>, b: SymMap<bool>) -> SymMap<bool> {
    let mut out = a;
    for (k, vb) in b {
        match out.entry(k) {
            std::collections::btree_map::Entry::Occupied(mut e) => {
                let va = *e.get();
                e.insert(!(va || vb));
            }
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert(vb);
            }
        }
    }
    out
}

fn single_use_subst(x: &Sym, repl: &GenTerm, gt: GenTerm) -> GenTerm {
    let out: Result<GenTerm, std::convert::Infallible> = gt.map_post(&mut |node| {
        Ok(match &node.term {
            GenTermKind::Return(it) if matches!(&it.term, Term::Sym(y) if y == x) => repl.clone(),
            _ => node,
        })
    });
    out.unwrap()
}

fn single_use_aux(gt: GenTerm) -> (GenTerm, SymMap<bool>) {
    let (term, bt) = gt.into_parts();
    match term {
        GenTermKind::Arbitrary => (GenTerm::from_parts(term, bt), SymMap::new()),
        GenTermKind::WriteChoice { label, value, rest } => {
            let uses = of_symset(value.free_vars());
            let (rest, uses_rest) = single_use_aux(*rest);
            (
                GenTerm::write_choice(label, value, rest),
                union(uses, uses_rest),
            )
        }
        GenTermKind::Call { ref args, .. } => {
            let uses = args
                .iter()
                .map(|a| of_symset(a.free_vars()))
                .fold(SymMap::new(), union);
            (GenTerm::from_parts(term, bt), uses)
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => {
            let uses = [&addr, &value]
                .into_iter()
                .map(|it| of_symset(it.free_vars()))
                .fold(SymMap::new(), union);
            let (rest, uses_rest) = single_use_aux(*rest);
            (GenTerm::asgn(addr, ct, value, rest), union(uses, uses_rest))
        }
        GenTermKind::LetStar { var, value, rest } => {
            let (rest, mut only_ret) = single_use_aux(*rest);
            only_ret.remove(&var);
            if only_ret.get(&var) == Some(&true) {
                (single_use_subst(&var, &value, rest), only_ret)
            } else {
                let (value, uses_value) = single_use_aux(*value);
                (
                    GenTerm::let_star((var, value), rest),
                    union(only_ret, uses_value),
                )
            }
        }
        GenTermKind::Return(it) => {
            let uses = match &it.term {
                Term::Sym(x) => SymMap::from([(x.clone(), true)]),
                _ => of_symset(it.free_vars()),
            };
            (GenTerm::return_(it), uses)
        }
        GenTermKind::Assert { constraint, rest } => {
            let uses = of_symset(constraint.free_vars());
            let (rest, uses_rest) = single_use_aux(*rest);
            (GenTerm::assert(constraint, rest), union(uses, uses_rest))
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            let uses = of_symset(cond.free_vars());
            let (then_, uses_then) = single_use_aux(*then_);
            let (else_, uses_else) = single_use_aux(*else_);
            (
                GenTerm::rebuild_ite(cond, then_, else_),
                [uses, uses_then, uses_else]
                    .into_iter()
                    .fold(SymMap::new(), union),
            )
        }
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => {
            let mut perm_vars = perm.free_vars();
            perm_vars.remove(&var);
            let uses = of_symset(perm_vars);
            let (body, uses_body) = single_use_aux(*body);
            let mut uses_body: SymMap<bool> = uses_body.into_keys().map(|k| (k, false)).collect();
            uses_body.remove(&var);
            (
                GenTerm::map((var, var_bt, perm), body),
                union(uses, uses_body),
            )
        }
        GenTermKind::Scope { name, body } => {
            let uses = of_symset(name.free_vars());
            let (body, uses_body) = single_use_aux(*body);
            (GenTerm::scope(name, body), union(uses, uses_body))
        }
    }
}

fn inline_returns(term: GenTerm, syms: &mut SymGen) -> GenTerm {
    let out: Result<GenTerm, std::convert::Infallible> = term.map_pre(&mut |node| {
        Ok(match node.term {
            GenTermKind::LetStar { var, value, rest }
                if matches!(value.term, GenTermKind::Return(_)) =>
            {
                let GenTermKind::Return(it) = value.term else {
                    unreachable!("guarded above");
                };
                (*rest).subst(&Substitution::from([(var, it)]), syms)
            }
            term => GenTerm::from_parts(term, node.bt),
        })
    });
    out.unwrap()
}

pub fn inline_term(term: GenTerm, syms: &mut SymGen) -> GenTerm {
    let (term, _uses) = single_use_aux(term);
    inline_returns(term, syms)
}
