use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::index_term::{IndexTerm, Pattern, PatternKind, Term};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::sym::{Sym, SymMap, SymSet};

enum Visit<'a> {
    Term(&'a IndexTerm),
    Bind(&'a Sym),
    Unbind(&'a Sym),
}

fn walk_free<'a>(
    roots: impl IntoIterator<Item = &'a IndexTerm>,
    mut record: impl FnMut(&'a Sym, &'a BaseType),
) {
    let mut stack: Vec<Visit<'a>> = Vec::new();
    let roots: Vec<&IndexTerm> = roots.into_iter().collect();
    for root in roots.into_iter().rev() {
        stack.push(Visit::Term(root));
    }

    let mut bound: std::collections::BTreeMap<&'a Sym, usize> = std::collections::BTreeMap::new();

    while let Some(task) = stack.pop() {
        let it = match task {
            Visit::Term(it) => it,
            Visit::Bind(s) => {
                *bound.entry(s).or_insert(0) += 1;
                continue;
            }
            Visit::Unbind(s) => {
                match bound.get_mut(s) {
                    Some(n) if *n > 1 => *n -= 1,
                    _ => {
                        bound.remove(s);
                    }
                }
                continue;
            }
        };
        match &it.term {
            Term::Const(_)
            | Term::SizeOf(_)
            | Term::OffsetOf(_, _)
            | Term::Nil(_)
            | Term::CnNone(_) => {}
            Term::Sym(s) => {
                if !bound.contains_key(s) {
                    record(s, &it.bt);
                }
            }
            Term::Unop(_, t)
            | Term::NthTuple(_, t)
            | Term::StructMember(t, _)
            | Term::RecordMember(t, _)
            | Term::Cast(_, t)
            | Term::MemberShift(t, _, _)
            | Term::HasAllocId(t)
            | Term::Head(t)
            | Term::Tail(t)
            | Term::Representable(_, t)
            | Term::Good(_, t)
            | Term::WrapI(_, t)
            | Term::MapConst(_, t)
            | Term::CnSome(t)
            | Term::IsSome(t)
            | Term::GetOpt(t) => stack.push(Visit::Term(t)),
            Term::Binop(_, t1, t2)
            | Term::Cons(t1, t2)
            | Term::MapGet(t1, t2)
            | Term::CopyAllocId { addr: t1, loc: t2 }
            | Term::Aligned { t: t1, align: t2 }
            | Term::StructUpdate {
                target: t1,
                value: t2,
                ..
            }
            | Term::RecordUpdate {
                target: t1,
                value: t2,
                ..
            }
            | Term::ArrayShift {
                base: t1,
                index: t2,
                ..
            } => {
                stack.push(Visit::Term(t2));
                stack.push(Visit::Term(t1));
            }
            Term::Ite(t1, t2, t3) | Term::MapSet(t1, t2, t3) => {
                stack.push(Visit::Term(t3));
                stack.push(Visit::Term(t2));
                stack.push(Visit::Term(t1));
            }
            Term::EachI { var, body, .. } | Term::MapDef(var, body) => {
                stack.push(Visit::Unbind(&var.0));
                stack.push(Visit::Term(body));
                stack.push(Visit::Bind(&var.0));
            }
            Term::Tuple(ts) | Term::Apply(_, ts) => {
                for t in ts.iter().rev() {
                    stack.push(Visit::Term(t));
                }
            }
            Term::Struct(_, ms) | Term::Record(ms) | Term::Constructor(_, ms) => {
                for (_, t) in ms.iter().rev() {
                    stack.push(Visit::Term(t));
                }
            }
            Term::Let((nm, bound_to), body) => {
                stack.push(Visit::Unbind(nm));
                stack.push(Visit::Term(body));
                stack.push(Visit::Bind(nm));
                stack.push(Visit::Term(bound_to));
            }
            Term::Match(scrutinee, cases) => {
                stack.push(Visit::Term(scrutinee));
                let mut binders = Vec::new();
                for (pat, body) in cases {
                    binders.clear();
                    pattern_binders(pat, &mut binders);
                    for s in &binders {
                        stack.push(Visit::Unbind(s));
                    }
                    stack.push(Visit::Term(body));
                    for s in &binders {
                        stack.push(Visit::Bind(s));
                    }
                }
            }
        }
    }
}

pub fn free_vars_bts_list<'a>(its: impl IntoIterator<Item = &'a IndexTerm>) -> SymMap<BaseType> {
    let mut out = SymMap::new();
    walk_free(its, |s, bt| {
        if !out.contains_key(s) {
            out.insert(s.clone(), bt.clone());
        }
    });
    out
}

fn pattern_binders<'a>(pat: &'a Pattern, out: &mut Vec<&'a Sym>) {
    let mut stack = vec![pat];
    while let Some(p) = stack.pop() {
        match &p.pattern {
            PatternKind::Sym(s) => out.push(s),
            PatternKind::Wild => {}
            PatternKind::Constructor(_, args) => {
                for (_, arg) in args {
                    stack.push(arg);
                }
            }
        }
    }
}

pub(in crate::gen_dsl) fn bound_by_pattern(pat: &Pattern, out: &mut SymSet) {
    let mut binders = Vec::new();
    pattern_binders(pat, &mut binders);
    out.extend(binders.into_iter().cloned());
}

impl IndexTerm {
    pub fn free_vars_bts(&self) -> SymMap<BaseType> {
        free_vars_bts_list([self])
    }

    pub fn free_vars(&self) -> SymSet {
        free_vars_list([self])
    }
}

pub(in crate::gen_dsl) fn free_vars_list<'a>(
    its: impl IntoIterator<Item = &'a IndexTerm>,
) -> SymSet {
    let mut out = SymSet::new();
    walk_free(its, |s, _| {
        out.insert(s.clone());
    });
    out
}

impl GenTerm {
    pub fn free_vars(&self) -> SymSet {
        let mut out = SymSet::new();
        gen_free_vars(self, &mut out);
        out
    }
}

fn gen_free_vars(gt: &GenTerm, out: &mut SymSet) {
    match &gt.term {
        GenTermKind::Arbitrary => {}
        GenTermKind::WriteChoice { value, rest, .. } => {
            out.extend(value.free_vars());
            gen_free_vars(rest, out);
        }
        GenTermKind::Return(it) => out.extend(it.free_vars()),
        GenTermKind::Call { args, .. } => {
            for arg in args {
                out.extend(arg.free_vars());
            }
        }
        GenTermKind::Asgn {
            addr, value, rest, ..
        } => {
            out.extend(addr.free_vars());
            out.extend(value.free_vars());
            gen_free_vars(rest, out);
        }
        GenTermKind::Assert { constraint, rest } => {
            out.extend(constraint.free_vars());
            gen_free_vars(rest, out);
        }
        GenTermKind::LetStar { var, value, rest } => {
            gen_free_vars(value, out);
            let mut inner = SymSet::new();
            gen_free_vars(rest, &mut inner);
            inner.remove(var);
            out.extend(inner);
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            out.extend(cond.free_vars());
            gen_free_vars(then_, out);
            gen_free_vars(else_, out);
        }
        GenTermKind::Map {
            var, perm, body, ..
        } => {
            let mut inner = SymSet::new();
            inner.extend(perm.free_vars());
            gen_free_vars(body, &mut inner);
            inner.remove(var);
            out.extend(inner);
        }
        GenTermKind::Scope { name, body } => {
            out.extend(name.free_vars());
            gen_free_vars(body, out);
        }
    }
}

impl LogicalConstraint {
    pub fn free_vars_bts(&self) -> SymMap<BaseType> {
        match self {
            LogicalConstraint::T(it) => it.free_vars_bts(),
            LogicalConstraint::Forall {
                var,
                permission,
                body,
            } => {
                let mut vars = free_vars_bts_list([permission, body]);
                vars.remove(&var.0);
                vars
            }
        }
    }

    pub fn free_vars(&self) -> SymSet {
        match self {
            LogicalConstraint::T(it) => it.free_vars(),
            LogicalConstraint::Forall {
                var,
                permission,
                body,
            } => {
                let mut out = SymSet::new();
                walk_free([permission, body], |s, _| {
                    if s != &var.0 {
                        out.insert(s.clone());
                    }
                });
                out
            }
        }
    }
}
