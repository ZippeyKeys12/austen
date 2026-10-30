use crate::gen_dsl::choice::translate::call_key;
use crate::gen_dsl::choice::tree::SiteGen;
use crate::gen_dsl::{GenTerm, GenTermKind, ScopeKey, SymGen};

pub fn reify_tree(term: GenTerm, syms: &mut SymGen, sites: &mut SiteGen) -> GenTerm {
    embed(term, sites, &mut Recording::On { syms })
}

pub fn scope_choices(term: GenTerm) -> GenTerm {
    embed(term, &mut SiteGen::new(), &mut Recording::Off)
}

enum Recording<'a> {
    On { syms: &'a mut SymGen },
    Off,
}

fn embed(term: GenTerm, sites: &mut SiteGen, mode: &mut Recording) -> GenTerm {
    let (term, bt) = term.into_parts();
    match term {
        GenTermKind::Arbitrary => match mode {
            Recording::On { syms } => {
                GenTerm::recorded_arbitrary_value(sites.label(), syms.fresh_anon(), bt)
            }
            Recording::Off => GenTerm::from_parts(GenTermKind::Arbitrary, bt),
        },
        GenTermKind::LetStar { var, value, rest } => match (&value.term, &mut *mode) {
            (GenTermKind::Arbitrary, Recording::On { .. }) => {
                let label = sites.label();
                GenTerm::recorded_arbitrary(label, var, value.bt, embed(*rest, sites, mode))
            }
            _ => {
                let value = embed(*value, sites, mode);
                GenTerm::let_star((var, value), embed(*rest, sites, mode))
            }
        },
        GenTermKind::WriteChoice { label, value, rest } => {
            GenTerm::write_choice(label, value, embed(*rest, sites, mode))
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => GenTerm::asgn(addr, ct, value, embed(*rest, sites, mode)),
        GenTermKind::Assert { constraint, rest } => {
            GenTerm::assert(constraint, embed(*rest, sites, mode))
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            let then_sid = sites.scope("then");
            let then_ = embed(*then_, sites, mode);
            let else_sid = sites.scope("else");
            let else_ = embed(*else_, sites, mode);
            GenTerm::ite(
                cond,
                GenTerm::scope(then_sid, then_),
                GenTerm::scope(else_sid, else_),
            )
        }
        GenTermKind::Call { fsym, args } => {
            let ScopeKey::Str(tag) = call_key(&fsym) else {
                unreachable!("call keys are always strings")
            };
            GenTerm::scope(sites.scope(tag), GenTerm::call(fsym, args, bt))
        }
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => {
            let tag = var.name.clone().unwrap_or_else(|| "map".into());
            let sid = sites.scope(tag);
            let body = embed(*body, sites, mode);
            GenTerm::scope(sid, GenTerm::scoped_map((var, var_bt, perm), body))
        }
        GenTermKind::Scope { name, body } => GenTerm::scope(name, embed(*body, sites, mode)),
        GenTermKind::Return(value) => GenTerm::return_(value),
    }
}
