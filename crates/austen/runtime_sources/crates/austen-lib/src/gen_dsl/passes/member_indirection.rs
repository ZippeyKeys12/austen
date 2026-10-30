use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::free_vars::bound_by_pattern;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind, ScopeName};
use crate::gen_dsl::lang::id::Id;
use crate::gen_dsl::lang::index_term::{IndexTerm, Pattern, Term};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::sym::{Sym, SymGen, SymSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberKind {
    Struct(Vec<(Id, Sym)>),
    Record(Vec<(Id, Sym)>),
    Tuple(Vec<(usize, Sym)>),
}

impl MemberKind {
    fn syms(&self) -> impl Iterator<Item = &Sym> {
        match self {
            MemberKind::Struct(dict) | MemberKind::Record(dict) => {
                Box::new(dict.iter().map(|(_, z)| z)) as Box<dyn Iterator<Item = &Sym>>
            }
            MemberKind::Tuple(dict) => Box::new(dict.iter().map(|(_, z)| z)),
        }
    }
}

fn assoc<'a, K: PartialEq>(dict: &'a [(K, Sym)], key: &K) -> Option<&'a Sym> {
    dict.iter().find(|(k, _)| k == key).map(|(_, z)| z)
}

fn pattern_binds_any(pat: &Pattern, stop: &SymSet) -> bool {
    let mut bound = SymSet::new();
    bound_by_pattern(pat, &mut bound);
    bound.iter().any(|s| stop.contains(s))
}

fn replace_memberof_it(k: &MemberKind, sym: &Sym, stop: &SymSet, it: IndexTerm) -> IndexTerm {
    let (term, bt) = it.into_parts();
    let r = |b: Box<IndexTerm>| Box::new(replace_memberof_it(k, sym, stop, *b));
    let term = match term {
        Term::Const(_)
        | Term::Sym(_)
        | Term::SizeOf(_)
        | Term::OffsetOf(_, _)
        | Term::Nil(_)
        | Term::CnNone(_) => term,
        Term::NthTuple(n, base) => match (k, base.as_sym()) {
            (MemberKind::Tuple(dict), Some(y)) if y == sym => match assoc(dict, &n) {
                Some(z) => Term::Sym(z.clone()),
                None => Term::NthTuple(n, base),
            },
            _ => Term::NthTuple(n, r(base)),
        },
        Term::StructMember(base, m) => match (k, base.as_sym()) {
            (MemberKind::Struct(dict), Some(y)) if y == sym => match assoc(dict, &m) {
                Some(z) => Term::Sym(z.clone()),
                None => Term::StructMember(base, m),
            },
            _ => Term::StructMember(r(base), m),
        },
        Term::RecordMember(base, m) => match (k, base.as_sym()) {
            (MemberKind::Record(dict), Some(y)) if y == sym => match assoc(dict, &m) {
                Some(z) => Term::Sym(z.clone()),
                None => Term::RecordMember(base, m),
            },
            _ => Term::RecordMember(r(base), m),
        },
        Term::MemberShift(base, tag, m) => Term::MemberShift(base, tag, m),
        Term::Let((z, bound), body) => Term::Let((z, r(bound)), body),
        Term::EachI {
            start,
            var,
            end,
            body,
        } => {
            let body = if stop.contains(&var.0) { body } else { r(body) };
            Term::EachI {
                start,
                var,
                end,
                body,
            }
        }
        Term::MapDef(var, body) => {
            let body = if stop.contains(&var.0) { body } else { r(body) };
            Term::MapDef(var, body)
        }
        Term::Match(scrutinee, arms) => Term::Match(
            r(scrutinee),
            arms.into_iter()
                .map(|(pat, body)| {
                    let body = if pattern_binds_any(&pat, stop) {
                        body
                    } else {
                        replace_memberof_it(k, sym, stop, body)
                    };
                    (pat, body)
                })
                .collect(),
        ),
        Term::Unop(op, a) => Term::Unop(op, r(a)),
        Term::Binop(op, a, b) => Term::Binop(op, r(a), r(b)),
        Term::Ite(c, t, e) => Term::Ite(r(c), r(t), r(e)),
        Term::Tuple(items) => Term::Tuple(
            items
                .into_iter()
                .map(|i| replace_memberof_it(k, sym, stop, i))
                .collect(),
        ),
        Term::Struct(tag, members) => Term::Struct(
            tag,
            members
                .into_iter()
                .map(|(id, i)| (id, replace_memberof_it(k, sym, stop, i)))
                .collect(),
        ),
        Term::StructUpdate {
            target,
            member,
            value,
        } => Term::StructUpdate {
            target: r(target),
            member,
            value: r(value),
        },
        Term::Record(members) => Term::Record(
            members
                .into_iter()
                .map(|(id, i)| (id, replace_memberof_it(k, sym, stop, i)))
                .collect(),
        ),
        Term::RecordUpdate {
            target,
            member,
            value,
        } => Term::RecordUpdate {
            target: r(target),
            member,
            value: r(value),
        },
        Term::Constructor(tag, members) => Term::Constructor(
            tag,
            members
                .into_iter()
                .map(|(id, i)| (id, replace_memberof_it(k, sym, stop, i)))
                .collect(),
        ),
        Term::ArrayShift { base, ct, index } => Term::ArrayShift {
            base: r(base),
            ct,
            index: r(index),
        },
        Term::CopyAllocId { addr, loc } => Term::CopyAllocId {
            addr: r(addr),
            loc: r(loc),
        },
        Term::HasAllocId(a) => Term::HasAllocId(r(a)),
        Term::Cons(a, b) => Term::Cons(r(a), r(b)),
        Term::Head(a) => Term::Head(r(a)),
        Term::Tail(a) => Term::Tail(r(a)),
        Term::Representable(ct, a) => Term::Representable(ct, r(a)),
        Term::Good(ct, a) => Term::Good(ct, r(a)),
        Term::Aligned { t, align } => Term::Aligned {
            t: r(t),
            align: r(align),
        },
        Term::WrapI(ity, a) => Term::WrapI(ity, r(a)),
        Term::MapConst(mbt, a) => Term::MapConst(mbt, r(a)),
        Term::MapSet(m, key, value) => Term::MapSet(r(m), r(key), r(value)),
        Term::MapGet(m, key) => Term::MapGet(r(m), r(key)),
        Term::Apply(fsym, args) => Term::Apply(
            fsym,
            args.into_iter()
                .map(|a| replace_memberof_it(k, sym, stop, a))
                .collect(),
        ),
        Term::Cast(cbt, a) => Term::Cast(cbt, r(a)),
        Term::CnSome(a) => Term::CnSome(r(a)),
        Term::IsSome(a) => Term::IsSome(r(a)),
        Term::GetOpt(a) => Term::GetOpt(r(a)),
    };
    IndexTerm { term, bt }
}

pub fn replace_memberof_gt(k: &MemberKind, sym: &Sym, gt: GenTerm) -> GenTerm {
    let mut stop: SymSet = k.syms().cloned().collect();
    stop.insert(sym.clone());
    replace_gt(k, sym, &stop, gt)
}

fn replace_gt(k: &MemberKind, sym: &Sym, stop: &SymSet, gt: GenTerm) -> GenTerm {
    let (term, bt) = gt.into_parts();
    let repl = |it: IndexTerm| replace_memberof_it(k, sym, stop, it);
    match term {
        GenTermKind::Arbitrary => GenTerm::from_parts(term, bt),
        GenTermKind::WriteChoice { label, value, rest } => {
            GenTerm::write_choice(label, repl(value), replace_gt(k, sym, stop, *rest))
        }
        GenTermKind::Call { fsym, args } => {
            GenTerm::call(fsym, args.into_iter().map(repl).collect(), bt)
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => GenTerm::asgn(repl(addr), ct, repl(value), replace_gt(k, sym, stop, *rest)),
        GenTermKind::LetStar { var, value, rest } => {
            let value = replace_gt(k, sym, stop, *value);
            let rest = if stop.contains(&var) {
                *rest
            } else {
                replace_gt(k, sym, stop, *rest)
            };
            GenTerm::let_star((var, value), rest)
        }
        GenTermKind::Return(it) => GenTerm::return_(repl(it)),
        GenTermKind::Assert { constraint, rest } => {
            let constraint = match constraint {
                LogicalConstraint::T(it) => LogicalConstraint::T(repl(it)),
                LogicalConstraint::Forall {
                    var,
                    permission,
                    body,
                } => {
                    let sub = |it| if stop.contains(&var.0) { it } else { repl(it) };
                    LogicalConstraint::Forall {
                        permission: sub(permission),
                        body: sub(body),
                        var,
                    }
                }
            };
            GenTerm::assert(constraint, replace_gt(k, sym, stop, *rest))
        }
        GenTermKind::Ite { cond, then_, else_ } => GenTerm::from_parts(
            GenTermKind::Ite {
                cond: repl(cond),
                then_: Box::new(replace_gt(k, sym, stop, *then_)),
                else_: Box::new(replace_gt(k, sym, stop, *else_)),
            },
            bt,
        ),
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => {
            if stop.contains(&var) {
                GenTerm::map((var, var_bt, perm), *body)
            } else {
                GenTerm::map((var, var_bt, repl(perm)), replace_gt(k, sym, stop, *body))
            }
        }
        GenTermKind::Scope { name, body } => {
            let name = match name {
                name @ (ScopeName::Str(_) | ScopeName::Sid(_)) => name,
                ScopeName::Index(it) => ScopeName::Index(repl(it)),
            };
            GenTerm::scope(name, replace_gt(k, sym, stop, *body))
        }
    }
}

fn literal_of(node: &GenTerm) -> Option<&Term> {
    let GenTermKind::LetStar { value, .. } = &node.term else {
        return None;
    };
    let GenTermKind::Return(it) = &value.term else {
        return None;
    };
    match &it.term {
        lit @ (Term::Struct(..) | Term::Record(..) | Term::Tuple(..)) => Some(lit),
        _ => None,
    }
}

fn has_duplicate_ids(members: &[(Id, IndexTerm)]) -> bool {
    let mut seen = std::collections::BTreeSet::new();
    members.iter().any(|(id, _)| !seen.insert(id))
}

fn flatten(node: GenTerm, syms: &mut SymGen) -> GenTerm {
    match literal_of(&node) {
        None => return node,
        Some(Term::Struct(_, members) | Term::Record(members)) if has_duplicate_ids(members) => {
            return node;
        }
        Some(_) => {}
    }
    let GenTermKind::LetStar { var, value, rest } = node.term else {
        unreachable!("literal_of matched a let*");
    };
    let GenTermKind::Return(lit) = value.term else {
        unreachable!("literal_of matched a return");
    };
    match lit.into_parts().0 {
        Term::Struct(tag, xits) => flatten_fields(var, Some(tag), xits, *rest, syms),
        Term::Record(xits) => flatten_fields(var, None, xits, *rest, syms),
        Term::Tuple(items) => flatten_items(var, items, *rest, syms),
        _ => unreachable!("literal_of matched an aggregate"),
    }
}

fn flatten_fields(
    var: Sym,
    tag: Option<Sym>,
    xits: Vec<(Id, IndexTerm)>,
    rest: GenTerm,
    syms: &mut SymGen,
) -> GenTerm {
    let dict: Vec<(Id, Sym)> = xits
        .iter()
        .map(|(id, it)| {
            let z = it.as_sym().cloned().unwrap_or_else(|| syms.fresh_anon());
            (id.clone(), z)
        })
        .collect();
    let members: Vec<(Id, IndexTerm)> = xits
        .iter()
        .map(|(id, it)| {
            let z = assoc(&dict, id).expect("dict covers every member").clone();
            (id.clone(), IndexTerm::sym(z, it.bt.clone()))
        })
        .collect();
    let (k, literal) = match tag {
        Some(tag) => (
            MemberKind::Struct(dict.clone()),
            IndexTerm::struct_(tag, members),
        ),
        None => (
            MemberKind::Record(dict.clone()),
            IndexTerm::record(members).expect("duplicate members were declined"),
        ),
    };
    let rewritten = replace_memberof_gt(&k, &var, rest);
    let gt_main = GenTerm::let_star((var, GenTerm::return_(literal)), rewritten);
    xits.into_iter()
        .filter(|(_, it)| it.as_sym().is_none())
        .fold(gt_main, |acc, (id, it)| {
            let z = assoc(&dict, &id).expect("dict covers every member").clone();
            GenTerm::let_star((z, GenTerm::return_(it)), acc)
        })
}

fn flatten_items(var: Sym, items: Vec<IndexTerm>, rest: GenTerm, syms: &mut SymGen) -> GenTerm {
    let dict: Vec<(usize, Sym)> = items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let z = it.as_sym().cloned().unwrap_or_else(|| syms.fresh_anon());
            (i, z)
        })
        .collect();
    let rebuilt: Vec<IndexTerm> = items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let z = assoc(&dict, &i).expect("dict covers every item").clone();
            IndexTerm::sym(z, it.bt.clone())
        })
        .collect();
    let literal = IndexTerm {
        bt: BaseType::Tuple(rebuilt.iter().map(|it| it.bt.clone()).collect()),
        term: Term::Tuple(rebuilt),
    };
    let k = MemberKind::Tuple(dict.clone());
    let rewritten = replace_memberof_gt(&k, &var, rest);
    let gt_main = GenTerm::let_star((var, GenTerm::return_(literal)), rewritten);
    items
        .into_iter()
        .enumerate()
        .filter(|(_, it)| it.as_sym().is_none())
        .fold(gt_main, |acc, (i, it)| {
            let z = assoc(&dict, &i).expect("dict covers every item").clone();
            GenTerm::let_star((z, GenTerm::return_(it)), acc)
        })
}

pub fn member_indirection(term: GenTerm, syms: &mut SymGen) -> GenTerm {
    let out: Result<GenTerm, std::convert::Infallible> =
        term.map_post(&mut |node| Ok(flatten(node, syms)));
    out.unwrap()
}
