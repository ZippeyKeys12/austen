use std::collections::BTreeMap;

use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::gen_def::GenDef;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind, ScopeName};
use crate::gen_dsl::lang::id::Id;
use crate::gen_dsl::lang::index_term::{IndexTerm, Term};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::sctype::Sctype;
use crate::gen_dsl::lang::struct_def::StructDef;
use crate::gen_dsl::lang::sym::{Sym, SymGen};
use crate::gen_dsl::passes::TransformError;

fn render(sym: &Sym) -> String {
    match &sym.name {
        Some(n) => n.clone(),
        None => sym.num.to_string(),
    }
}

fn find_struct<'a>(structs: &'a [(Sym, StructDef)], tag: &Sym) -> Option<&'a StructDef> {
    structs.iter().find(|(s, _)| s == tag).map(|(_, d)| d)
}

#[derive(Default)]
struct Names {
    members: BTreeMap<(Sym, Id), Sym>,
    items: BTreeMap<(Sym, usize), Sym>,
}

impl Names {
    fn member(&mut self, syms: &mut SymGen, parent: &Sym, member: &Id) -> Sym {
        self.members
            .entry((parent.clone(), member.clone()))
            .or_insert_with(|| syms.fresh(&format!("{}_{}", render(parent), member.0)))
            .clone()
    }

    fn item(&mut self, syms: &mut SymGen, parent: &Sym, index: usize) -> Sym {
        self.items
            .entry((parent.clone(), index))
            .or_insert_with(|| syms.fresh(&format!("{}_{}", render(parent), index)))
            .clone()
    }
}

enum NewArgs {
    Struct(Sym, Vec<(Id, (Sym, NewArgs))>),
    Record(Vec<(Id, (Sym, NewArgs))>),
    Tuple(Vec<(Sym, NewArgs)>),
    Leaf(Sym, BaseType),
}

fn args_list_of(na: &NewArgs, out: &mut Vec<(Sym, BaseType)>) {
    match na {
        NewArgs::Struct(_, members) | NewArgs::Record(members) => {
            for (_, (_, child)) in members {
                args_list_of(child, out);
            }
        }
        NewArgs::Tuple(items) => {
            for (_, child) in items {
                args_list_of(child, out);
            }
        }
        NewArgs::Leaf(sym, bt) => out.push((sym.clone(), bt.clone())),
    }
}

enum ReplaceKind {
    Struct(Vec<(Id, Sym)>),
    Record(Vec<(Id, Sym)>),
    Tuple(Vec<Sym>),
}

fn replace_kind_of(na: &NewArgs) -> Option<ReplaceKind> {
    match na {
        NewArgs::Struct(_, members) => Some(ReplaceKind::Struct(
            members
                .iter()
                .map(|(id, (sym, _))| (id.clone(), sym.clone()))
                .collect(),
        )),
        NewArgs::Record(members) => Some(ReplaceKind::Record(
            members
                .iter()
                .map(|(id, (sym, _))| (id.clone(), sym.clone()))
                .collect(),
        )),
        NewArgs::Tuple(items) => Some(ReplaceKind::Tuple(
            items.iter().map(|(sym, _)| sym.clone()).collect(),
        )),
        NewArgs::Leaf(..) => None,
    }
}

fn it_of_new_args(na: &NewArgs) -> IndexTerm {
    match na {
        NewArgs::Struct(tag, members) => IndexTerm::struct_(
            tag.clone(),
            members
                .iter()
                .map(|(id, (_, child))| (id.clone(), it_of_new_args(child)))
                .collect(),
        ),
        NewArgs::Record(members) => {
            let its: Vec<(Id, IndexTerm)> = members
                .iter()
                .map(|(id, (_, child))| (id.clone(), it_of_new_args(child)))
                .collect();
            let bt = BaseType::Record(
                its.iter()
                    .map(|(id, it)| (id.clone(), it.bt.clone()))
                    .collect(),
            );
            IndexTerm {
                term: Term::Record(its),
                bt,
            }
        }
        NewArgs::Tuple(items) => {
            let its: Vec<IndexTerm> = items
                .iter()
                .map(|(_, child)| it_of_new_args(child))
                .collect();
            let bt = BaseType::Tuple(its.iter().map(|it| it.bt.clone()).collect());
            IndexTerm {
                term: Term::Tuple(its),
                bt,
            }
        }
        NewArgs::Leaf(sym, bt) => IndexTerm::sym(sym.clone(), bt.clone()),
    }
}

fn aggregate_rebinds<'a>(x: &Sym, na: &'a NewArgs, out: &mut Vec<(Sym, &'a NewArgs)>) {
    match na {
        NewArgs::Leaf(..) => {}
        NewArgs::Struct(_, members) | NewArgs::Record(members) => {
            out.push((x.clone(), na));
            for (_, (child_sym, child)) in members {
                aggregate_rebinds(child_sym, child, out);
            }
        }
        NewArgs::Tuple(items) => {
            out.push((x.clone(), na));
            for (child_sym, child) in items {
                aggregate_rebinds(child_sym, child, out);
            }
        }
    }
}

fn transform_iarg(
    structs: &[(Sym, StructDef)],
    names: &mut Names,
    syms: &mut SymGen,
    arg_sym: &Sym,
    arg_bt: &BaseType,
) -> Result<NewArgs, TransformError> {
    match arg_bt {
        BaseType::Struct(tag) => {
            let def = find_struct(structs, tag)
                .ok_or_else(|| TransformError::UnknownStruct { tag: tag.clone() })?;
            let mut members = Vec::new();
            for (member, sct) in &def.members {
                let bt = sct
                    .to_base_type()
                    .ok_or_else(|| TransformError::UnsupportedSctype { ct: sct.clone() })?;
                let fresh = names.member(syms, arg_sym, member);
                let child = transform_iarg(structs, names, syms, &fresh, &bt)?;
                members.push((member.clone(), (fresh, child)));
            }
            Ok(NewArgs::Struct(tag.clone(), members))
        }
        BaseType::Record(member_bts) => {
            let mut members = Vec::new();
            for (member, bt) in member_bts {
                let fresh = names.member(syms, arg_sym, member);
                let child = transform_iarg(structs, names, syms, &fresh, bt)?;
                members.push((member.clone(), (fresh, child)));
            }
            Ok(NewArgs::Record(members))
        }
        BaseType::Tuple(item_bts) => {
            let mut items = Vec::new();
            for (i, bt) in item_bts.iter().enumerate() {
                let fresh = names.item(syms, arg_sym, i);
                let child = transform_iarg(structs, names, syms, &fresh, bt)?;
                items.push((fresh, child));
            }
            Ok(NewArgs::Tuple(items))
        }
        _ => Ok(NewArgs::Leaf(arg_sym.clone(), arg_bt.clone())),
    }
}

fn replace_memberof_it(kind: &ReplaceKind, sym: &Sym, it: IndexTerm) -> IndexTerm {
    fn go(kind: &ReplaceKind, sym: &Sym, mut b: Box<IndexTerm>) -> Box<IndexTerm> {
        let taken = std::mem::replace(&mut *b, IndexTerm::unit());
        *b = replace_memberof_it(kind, sym, taken);
        b
    }
    let (term, bt) = it.into_parts();
    let term = match term {
        leaf @ (Term::Const(_)
        | Term::Sym(_)
        | Term::SizeOf(_)
        | Term::OffsetOf(_, _)
        | Term::Nil(_)
        | Term::CnNone(_)) => leaf,
        Term::Unop(op, t) => Term::Unop(op, go(kind, sym, t)),
        Term::Binop(op, a, b) => Term::Binop(op, go(kind, sym, a), go(kind, sym, b)),
        Term::Ite(c, a, b) => Term::Ite(go(kind, sym, c), go(kind, sym, a), go(kind, sym, b)),
        Term::EachI {
            start,
            var,
            end,
            body,
        } => Term::EachI {
            start,
            var,
            end,
            body: go(kind, sym, body),
        },
        Term::Tuple(ts) => Term::Tuple(
            ts.into_iter()
                .map(|t| replace_memberof_it(kind, sym, t))
                .collect(),
        ),
        Term::NthTuple(n, t) => match (kind, t.as_sym()) {
            (ReplaceKind::Tuple(syms), Some(y)) if y == sym && n < syms.len() => {
                Term::Sym(syms[n].clone())
            }
            _ => Term::NthTuple(n, go(kind, sym, t)),
        },
        Term::Struct(tag, ms) => Term::Struct(
            tag,
            ms.into_iter()
                .map(|(id, t)| (id, replace_memberof_it(kind, sym, t)))
                .collect(),
        ),
        Term::StructMember(t, x) => match (kind, t.as_sym()) {
            (ReplaceKind::Struct(dict), Some(y)) if y == sym => {
                match dict.iter().find(|(id, _)| *id == x) {
                    Some((_, target)) => Term::Sym(target.clone()),
                    None => Term::StructMember(t, x),
                }
            }
            _ => Term::StructMember(go(kind, sym, t), x),
        },
        Term::StructUpdate {
            target,
            member,
            value,
        } => Term::StructUpdate {
            target: go(kind, sym, target),
            member,
            value: go(kind, sym, value),
        },
        Term::Record(ms) => Term::Record(
            ms.into_iter()
                .map(|(id, t)| (id, replace_memberof_it(kind, sym, t)))
                .collect(),
        ),
        Term::RecordMember(t, x) => match (kind, t.as_sym()) {
            (ReplaceKind::Record(dict), Some(y)) if y == sym => {
                match dict.iter().find(|(id, _)| *id == x) {
                    Some((_, target)) => Term::Sym(target.clone()),
                    None => Term::RecordMember(t, x),
                }
            }
            _ => Term::RecordMember(go(kind, sym, t), x),
        },
        Term::RecordUpdate {
            target,
            member,
            value,
        } => Term::RecordUpdate {
            target: go(kind, sym, target),
            member,
            value: go(kind, sym, value),
        },
        Term::Constructor(tag, ms) => Term::Constructor(
            tag,
            ms.into_iter()
                .map(|(id, t)| (id, replace_memberof_it(kind, sym, t)))
                .collect(),
        ),
        Term::MemberShift(t, tag, member) => Term::MemberShift(t, tag, member),
        Term::ArrayShift { base, ct, index } => Term::ArrayShift {
            base: go(kind, sym, base),
            ct,
            index: go(kind, sym, index),
        },
        Term::CopyAllocId { addr, loc } => Term::CopyAllocId {
            addr: go(kind, sym, addr),
            loc: go(kind, sym, loc),
        },
        Term::HasAllocId(t) => Term::HasAllocId(go(kind, sym, t)),
        Term::Cons(a, b) => Term::Cons(go(kind, sym, a), go(kind, sym, b)),
        Term::Head(t) => Term::Head(go(kind, sym, t)),
        Term::Tail(t) => Term::Tail(go(kind, sym, t)),
        Term::Representable(ct, t) => Term::Representable(ct, go(kind, sym, t)),
        Term::Good(ct, t) => Term::Good(ct, go(kind, sym, t)),
        Term::Aligned { t, align } => Term::Aligned {
            t: go(kind, sym, t),
            align: go(kind, sym, align),
        },
        Term::WrapI(ity, t) => Term::WrapI(ity, go(kind, sym, t)),
        Term::MapConst(index_bt, t) => Term::MapConst(index_bt, go(kind, sym, t)),
        Term::MapSet(m, k, v) => Term::MapSet(go(kind, sym, m), go(kind, sym, k), go(kind, sym, v)),
        Term::MapGet(m, k) => Term::MapGet(go(kind, sym, m), go(kind, sym, k)),
        Term::MapDef(var, body) => Term::MapDef(var, go(kind, sym, body)),
        Term::Apply(name, args) => Term::Apply(
            name,
            args.into_iter()
                .map(|t| replace_memberof_it(kind, sym, t))
                .collect(),
        ),
        Term::Let((x, v), body) => Term::Let((x, go(kind, sym, v)), body),
        Term::Match(scrutinee, arms) => Term::Match(
            go(kind, sym, scrutinee),
            arms.into_iter()
                .map(|(p, t)| (p, replace_memberof_it(kind, sym, t)))
                .collect(),
        ),
        Term::Cast(target, t) => Term::Cast(target, go(kind, sym, t)),
        Term::CnSome(t) => Term::CnSome(go(kind, sym, t)),
        Term::IsSome(t) => Term::IsSome(go(kind, sym, t)),
        Term::GetOpt(t) => Term::GetOpt(go(kind, sym, t)),
    };
    IndexTerm { term, bt }
}

fn replace_memberof_gt(kind: &ReplaceKind, sym: &Sym, gt: GenTerm) -> GenTerm {
    let out: Result<GenTerm, std::convert::Infallible> = gt.map_pre(&mut |node| {
        let (term, bt) = node.into_parts();
        let term = match term {
            GenTermKind::Call { fsym, args } => GenTermKind::Call {
                fsym,
                args: args
                    .into_iter()
                    .map(|a| replace_memberof_it(kind, sym, a))
                    .collect(),
            },
            GenTermKind::WriteChoice { label, value, rest } => GenTermKind::WriteChoice {
                label,
                value: replace_memberof_it(kind, sym, value),
                rest,
            },
            GenTermKind::Asgn {
                addr,
                ct,
                value,
                rest,
            } => GenTermKind::Asgn {
                addr: replace_memberof_it(kind, sym, addr),
                ct,
                value: replace_memberof_it(kind, sym, value),
                rest,
            },
            GenTermKind::Return(it) => GenTermKind::Return(replace_memberof_it(kind, sym, it)),
            GenTermKind::Assert { constraint, rest } => GenTermKind::Assert {
                constraint: match constraint {
                    LogicalConstraint::T(it) => {
                        LogicalConstraint::T(replace_memberof_it(kind, sym, it))
                    }
                    LogicalConstraint::Forall {
                        var,
                        permission,
                        body,
                    } => LogicalConstraint::Forall {
                        var,
                        permission: replace_memberof_it(kind, sym, permission),
                        body: replace_memberof_it(kind, sym, body),
                    },
                },
                rest,
            },
            GenTermKind::Ite { cond, then_, else_ } => GenTermKind::Ite {
                cond: replace_memberof_it(kind, sym, cond),
                then_,
                else_,
            },
            GenTermKind::Map {
                var,
                var_bt,
                perm,
                body,
            } => GenTermKind::Map {
                var,
                var_bt,
                perm: replace_memberof_it(kind, sym, perm),
                body,
            },
            GenTermKind::Scope { name, body } => GenTermKind::Scope {
                name: match name {
                    name @ (ScopeName::Str(_) | ScopeName::Sid(_)) => name,
                    ScopeName::Index(it) => ScopeName::Index(replace_memberof_it(kind, sym, it)),
                },
                body,
            },
            other @ (GenTermKind::Arbitrary | GenTermKind::LetStar { .. }) => other,
        };
        Ok(GenTerm::from_parts(term, bt))
    });
    out.unwrap()
}

fn transform_it(
    structs: &[(Sym, StructDef)],
    it: IndexTerm,
    bt: &BaseType,
    out: &mut Vec<IndexTerm>,
) -> Result<(), TransformError> {
    match bt {
        BaseType::Struct(tag) => {
            let def = find_struct(structs, tag)
                .ok_or_else(|| TransformError::UnknownStruct { tag: tag.clone() })?;
            for (member, sct) in &def.members {
                let member_bt = sct
                    .to_base_type()
                    .ok_or_else(|| TransformError::UnsupportedSctype { ct: sct.clone() })?;
                let proj = IndexTerm::struct_member(it.clone(), member.clone(), member_bt.clone());
                transform_it(structs, proj, &member_bt, out)?;
            }
        }
        BaseType::Record(members) => {
            for (member, member_bt) in members {
                let proj = IndexTerm {
                    term: Term::RecordMember(Box::new(it.clone()), member.clone()),
                    bt: member_bt.clone(),
                };
                transform_it(structs, proj, member_bt, out)?;
            }
        }
        BaseType::Tuple(items) => {
            for (i, item_bt) in items.iter().enumerate() {
                let proj = IndexTerm {
                    term: Term::NthTuple(i, Box::new(it.clone())),
                    bt: item_bt.clone(),
                };
                transform_it(structs, proj, item_bt, out)?;
            }
        }
        _ => out.push(it),
    }
    Ok(())
}

fn flatten_calls(
    defs: &[(Sym, GenDef)],
    structs: &[(Sym, StructDef)],
    body: GenTerm,
) -> Result<GenTerm, TransformError> {
    body.map_pre(&mut |node| {
        let (term, bt) = node.into_parts();
        let GenTermKind::Call { fsym, args } = term else {
            return Ok(GenTerm::from_parts(term, bt));
        };
        let Some((_, callee)) = defs.iter().find(|(s, _)| *s == fsym) else {
            return Err(TransformError::UnknownCallee { fsym });
        };
        if args.len() != callee.iargs.len() {
            return Err(TransformError::CallArityMismatch {
                fsym,
                expected: callee.iargs.len(),
                actual: args.len(),
            });
        }
        let mut flat = Vec::new();
        for (arg, (_, arg_bt)) in args.into_iter().zip(&callee.iargs) {
            transform_it(structs, arg, arg_bt, &mut flat)?;
        }
        Ok(GenTerm::from_parts(
            GenTermKind::Call { fsym, args: flat },
            bt,
        ))
    })
}

fn replace_member_of(tm: GenTerm, x: &Sym, na: &NewArgs) -> GenTerm {
    match na {
        NewArgs::Leaf(..) => tm,
        NewArgs::Struct(_, members) | NewArgs::Record(members) => {
            let kind = replace_kind_of(na).expect("an aggregate node has a replace kind");
            let mut tm = replace_memberof_gt(&kind, x, tm);
            for (_, (child_sym, child)) in members {
                tm = replace_member_of(tm, child_sym, child);
            }
            tm
        }
        NewArgs::Tuple(items) => {
            let kind = replace_kind_of(na).expect("an aggregate node has a replace kind");
            let mut tm = replace_memberof_gt(&kind, x, tm);
            for (child_sym, child) in items {
                tm = replace_member_of(tm, child_sym, child);
            }
            tm
        }
    }
}

fn c_types_of_new_args(
    structs: &[(Sym, StructDef)],
    na: &NewArgs,
    ct: &Sctype,
    param: &Sym,
    out: &mut Vec<(Sym, Sctype)>,
) -> Result<(), TransformError> {
    match na {
        NewArgs::Leaf(sym, _) => out.push((sym.clone(), ct.clone())),
        NewArgs::Struct(tag, members) => {
            let def = find_struct(structs, tag)
                .ok_or_else(|| TransformError::UnknownStruct { tag: tag.clone() })?;
            debug_assert_eq!(
                members.len(),
                def.members.len(),
                "the na members and the struct's members derive from one list"
            );
            for ((_, (_, child)), (_, member_sct)) in members.iter().zip(&def.members) {
                c_types_of_new_args(structs, child, member_sct, param, out)?;
            }
        }
        NewArgs::Record(_) | NewArgs::Tuple(_) => {
            return Err(TransformError::AggregateCTypedParam {
                param: param.clone(),
            });
        }
    }
    Ok(())
}

fn transform_gd(
    defs: &[(Sym, GenDef)],
    structs: &[(Sym, StructDef)],
    names: &mut Names,
    syms: &mut SymGen,
    gd: &GenDef,
) -> Result<GenDef, TransformError> {
    let xds: Vec<(Sym, NewArgs)> = gd
        .iargs
        .iter()
        .map(|(sym, bt)| Ok((sym.clone(), transform_iarg(structs, names, syms, sym, bt)?)))
        .collect::<Result<_, TransformError>>()?;
    let mut iargs = Vec::new();
    for (_, na) in &xds {
        args_list_of(na, &mut iargs);
    }
    let mut body = flatten_calls(defs, structs, gd.body.clone())?;
    for (x, na) in &xds {
        body = replace_member_of(body, x, na);
    }
    let mut rebinds = Vec::new();
    for (x, na) in &xds {
        aggregate_rebinds(x, na, &mut rebinds);
    }
    for (x, na) in rebinds {
        body = GenTerm::let_star((x, GenTerm::return_(it_of_new_args(na))), body);
    }
    let c_types = gd
        .c_types
        .as_ref()
        .map(|cts| {
            let mut out = Vec::new();
            for (param, ct) in cts {
                match xds.iter().find(|(x, _)| render(x) == render(param)) {
                    Some((_, na)) => c_types_of_new_args(structs, na, ct, param, &mut out)?,
                    None => out.push((param.clone(), ct.clone())),
                }
            }
            Ok(out)
        })
        .transpose()?;
    Ok(GenDef {
        iargs,
        c_types,
        body,
        ..gd.clone()
    })
}

pub fn destruct_products(
    defs: &[(Sym, GenDef)],
    structs: &[(Sym, StructDef)],
    syms: &mut SymGen,
) -> Result<Vec<(Sym, GenDef)>, TransformError> {
    let mut names = Names::default();
    defs.iter()
        .map(|(name, gd)| {
            Ok((
                name.clone(),
                transform_gd(defs, structs, &mut names, syms, gd)?,
            ))
        })
        .collect()
}
