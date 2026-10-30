use std::borrow::Cow;

use crate::gen_dsl::{GenTerm, GenTermKind, IndexTerm, ScopeName, Substitution, Sym, SymGen};

impl GenTerm {
    pub fn subst(self, sub: &Substitution, syms: &mut SymGen) -> Self {
        subst_gen(self, sub, syms)
    }
}

fn shadow<'a>(sub: &'a Substitution, binder: &Sym) -> Cow<'a, Substitution> {
    if sub.contains_key(binder) {
        let mut owned = sub.clone();
        owned.remove(binder);
        Cow::Owned(owned)
    } else {
        Cow::Borrowed(sub)
    }
}

fn captures(sub: &Substitution, binder: &Sym) -> bool {
    sub.values().any(|repl| repl.free_vars().contains(binder))
}

pub(in crate::gen_dsl) fn fresh_same(syms: &mut SymGen, sym: &Sym) -> Sym {
    match &sym.name {
        Some(name) => syms.fresh(name),
        None => syms.fresh_anon(),
    }
}

pub(in crate::gen_dsl) fn subst_gen(
    term: GenTerm,
    sub: &Substitution,
    syms: &mut SymGen,
) -> GenTerm {
    if sub.is_empty() {
        return term;
    }
    let (term, bt) = term.into_parts();
    match term {
        GenTermKind::Arbitrary => GenTerm::from_parts(term, bt),
        GenTermKind::WriteChoice { label, value, rest } => {
            GenTerm::write_choice(label, value.subst(sub), subst_gen(*rest, sub, syms))
        }
        GenTermKind::Call { fsym, args } => {
            let args = args.iter().map(|a| a.subst(sub)).collect();
            GenTerm::call(fsym, args, bt)
        }
        GenTermKind::Return(it) => GenTerm::return_(it.subst(sub)),
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => GenTerm::asgn(
            addr.subst(sub),
            ct,
            value.subst(sub),
            subst_gen(*rest, sub, syms),
        ),
        GenTermKind::Assert { constraint, rest } => {
            GenTerm::assert(constraint.subst(sub), subst_gen(*rest, sub, syms))
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            let cond = cond.subst(sub);
            let then_ = subst_gen(*then_, sub, syms);
            let else_ = subst_gen(*else_, sub, syms);
            GenTerm::rebuild_ite(cond, then_, else_)
        }
        GenTermKind::Scope { name, body } => {
            let name = match name {
                name @ (ScopeName::Str(_) | ScopeName::Sid(_)) => name,
                ScopeName::Index(it) => ScopeName::Index(it.subst(sub)),
            };
            GenTerm::scope(name, subst_gen(*body, sub, syms))
        }
        GenTermKind::LetStar { var, value, rest } => {
            let value = subst_gen(*value, sub, syms);
            let sub = shadow(sub, &var);
            if sub.is_empty() {
                return GenTerm::let_star((var, value), *rest);
            }
            if captures(&sub, &var) {
                let renamed = fresh_same(syms, &var);
                let rename =
                    Substitution::from([(var, IndexTerm::sym(renamed.clone(), value.bt.clone()))]);
                let rest = subst_gen(*rest, &rename, syms);
                GenTerm::let_star((renamed, value), subst_gen(rest, sub.as_ref(), syms))
            } else {
                GenTerm::let_star((var, value), subst_gen(*rest, sub.as_ref(), syms))
            }
        }
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => {
            let sub = shadow(sub, &var);
            if sub.is_empty() {
                return GenTerm::map((var, var_bt, perm), *body);
            }
            if captures(&sub, &var) {
                let renamed = fresh_same(syms, &var);
                let rename =
                    Substitution::from([(var, IndexTerm::sym(renamed.clone(), var_bt.clone()))]);
                let perm = perm.subst(&rename).subst(sub.as_ref());
                let body = subst_gen(subst_gen(*body, &rename, syms), sub.as_ref(), syms);
                GenTerm::map((renamed, var_bt, perm), body)
            } else {
                GenTerm::map(
                    (var, var_bt, perm.subst(sub.as_ref())),
                    subst_gen(*body, sub.as_ref(), syms),
                )
            }
        }
    }
}
