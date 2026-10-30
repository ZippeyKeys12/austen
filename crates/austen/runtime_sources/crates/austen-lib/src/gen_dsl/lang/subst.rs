use std::borrow::Cow;

use crate::gen_dsl::lang::free_vars::bound_by_pattern;
use crate::gen_dsl::lang::index_term::{IndexTerm, Term};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::sym::{Sym, SymMap, SymSet};

pub type Substitution = SymMap<IndexTerm>;

fn without<'a>(sub: &'a Substitution, bound: &Sym) -> Cow<'a, Substitution> {
    if sub.contains_key(bound) {
        let mut owned = sub.clone();
        owned.remove(bound);
        Cow::Owned(owned)
    } else {
        Cow::Borrowed(sub)
    }
}

fn without_all<'a>(sub: &'a Substitution, bound: &SymSet) -> Cow<'a, Substitution> {
    if bound.iter().any(|s| sub.contains_key(s)) {
        let mut owned = sub.clone();
        owned.retain(|s, _| !bound.contains(s));
        Cow::Owned(owned)
    } else {
        Cow::Borrowed(sub)
    }
}

impl IndexTerm {
    pub fn subst(&self, sub: &Substitution) -> IndexTerm {
        self.subst_inner(sub)
    }

    fn subst_inner(&self, sub: &Substitution) -> IndexTerm {
        let same = |term: Term| IndexTerm {
            term,
            bt: self.bt.clone(),
        };
        let go = |t: &IndexTerm| Box::new(t.subst_inner(sub));
        let go_all = |ts: &[IndexTerm]| ts.iter().map(|t| t.subst_inner(sub)).collect::<Vec<_>>();
        let go_members = |ms: &[(crate::gen_dsl::lang::id::Id, IndexTerm)]| {
            ms.iter()
                .map(|(id, t)| (id.clone(), t.subst_inner(sub)))
                .collect::<Vec<_>>()
        };

        match &self.term {
            Term::Const(_)
            | Term::SizeOf(_)
            | Term::OffsetOf(_, _)
            | Term::Nil(_)
            | Term::CnNone(_) => self.clone(),

            Term::Sym(s) => match sub.get(s) {
                Some(replacement) => replacement.clone(),
                None => self.clone(),
            },

            Term::EachI {
                start,
                var,
                end,
                body,
            } => {
                let inner = without(sub, &var.0);
                same(Term::EachI {
                    start: *start,
                    var: var.clone(),
                    end: *end,
                    body: Box::new(body.subst_inner(&inner)),
                })
            }
            Term::MapDef(var, body) => {
                let inner = without(sub, &var.0);
                same(Term::MapDef(
                    var.clone(),
                    Box::new(body.subst_inner(&inner)),
                ))
            }
            Term::Let((nm, bound), body) => {
                let inner = without(sub, nm);
                same(Term::Let(
                    (nm.clone(), go(bound)),
                    Box::new(body.subst_inner(&inner)),
                ))
            }
            Term::Match(scrutinee, cases) => same(Term::Match(
                go(scrutinee),
                cases
                    .iter()
                    .map(|(pat, body)| {
                        let mut bound = SymSet::new();
                        bound_by_pattern(pat, &mut bound);
                        let inner = without_all(sub, &bound);
                        (pat.clone(), body.subst_inner(&inner))
                    })
                    .collect(),
            )),

            Term::Unop(op, t) => same(Term::Unop(*op, go(t))),
            Term::Binop(op, a, b) => same(Term::Binop(*op, go(a), go(b))),
            Term::Ite(c, a, b) => same(Term::Ite(go(c), go(a), go(b))),
            Term::Tuple(ts) => same(Term::Tuple(go_all(ts))),
            Term::NthTuple(n, t) => same(Term::NthTuple(*n, go(t))),
            Term::Struct(tag, ms) => same(Term::Struct(tag.clone(), go_members(ms))),
            Term::StructMember(t, m) => same(Term::StructMember(go(t), m.clone())),
            Term::StructUpdate {
                target,
                member,
                value,
            } => same(Term::StructUpdate {
                target: go(target),
                member: member.clone(),
                value: go(value),
            }),
            Term::Record(ms) => same(Term::Record(go_members(ms))),
            Term::RecordMember(t, m) => same(Term::RecordMember(go(t), m.clone())),
            Term::RecordUpdate {
                target,
                member,
                value,
            } => same(Term::RecordUpdate {
                target: go(target),
                member: member.clone(),
                value: go(value),
            }),
            Term::Constructor(tag, ms) => same(Term::Constructor(tag.clone(), go_members(ms))),
            Term::MemberShift(t, tag, m) => same(Term::MemberShift(go(t), tag.clone(), m.clone())),
            Term::ArrayShift { base, ct, index } => same(Term::ArrayShift {
                base: go(base),
                ct: ct.clone(),
                index: go(index),
            }),
            Term::CopyAllocId { addr, loc } => same(Term::CopyAllocId {
                addr: go(addr),
                loc: go(loc),
            }),
            Term::HasAllocId(t) => same(Term::HasAllocId(go(t))),
            Term::Cons(h, t) => same(Term::Cons(go(h), go(t))),
            Term::Head(t) => same(Term::Head(go(t))),
            Term::Tail(t) => same(Term::Tail(go(t))),
            Term::Representable(ct, t) => same(Term::Representable(ct.clone(), go(t))),
            Term::Good(ct, t) => same(Term::Good(ct.clone(), go(t))),
            Term::Aligned { t, align } => same(Term::Aligned {
                t: go(t),
                align: go(align),
            }),
            Term::WrapI(ity, t) => same(Term::WrapI(ity.clone(), go(t))),
            Term::MapConst(bt, t) => same(Term::MapConst(bt.clone(), go(t))),
            Term::MapSet(m, k, v) => same(Term::MapSet(go(m), go(k), go(v))),
            Term::MapGet(m, k) => same(Term::MapGet(go(m), go(k))),
            Term::Apply(f, args) => same(Term::Apply(f.clone(), go_all(args))),
            Term::Cast(bt, t) => same(Term::Cast(bt.clone(), go(t))),
            Term::CnSome(t) => same(Term::CnSome(go(t))),
            Term::IsSome(t) => same(Term::IsSome(go(t))),
            Term::GetOpt(t) => same(Term::GetOpt(go(t))),
        }
    }
}

impl LogicalConstraint {
    pub fn subst(&self, sub: &Substitution) -> LogicalConstraint {
        match self {
            LogicalConstraint::T(it) => LogicalConstraint::T(it.subst(sub)),
            LogicalConstraint::Forall {
                var,
                permission,
                body,
            } => {
                let inner = without(sub, &var.0);
                LogicalConstraint::Forall {
                    var: var.clone(),
                    permission: permission.subst(&inner),
                    body: body.subst(&inner),
                }
            }
        }
    }
}
