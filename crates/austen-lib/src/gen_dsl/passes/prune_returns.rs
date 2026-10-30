use std::collections::BTreeSet;

use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::gen_def::GenDef;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind, ScopeName};
use crate::gen_dsl::lang::id::Id;
use crate::gen_dsl::lang::index_term::{Const, IndexTerm, Term};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::sctype::Sctype;
use crate::gen_dsl::lang::struct_def::StructDef;
use crate::gen_dsl::lang::sym::{Sym, SymGen, SymMap, SymSet};
use crate::gen_dsl::passes::TransformError;
use crate::gen_dsl::passes::tail::map_tail;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PruneRequest {
    Entire,
    Nothing,
    Struct(Vec<Id>),
    Record(Vec<Id>),
    Tuple(Vec<usize>),
}

#[derive(Default)]
struct Usage {
    used_directly: bool,
    struct_members: BTreeSet<Id>,
    record_members: BTreeSet<Id>,
    tuple_indices: Vec<usize>,
}

impl Usage {
    fn direct() -> Self {
        Usage {
            used_directly: true,
            ..Usage::default()
        }
    }

    fn merge(mut self, other: Usage) -> Usage {
        self.used_directly |= other.used_directly;
        self.struct_members.extend(other.struct_members);
        self.record_members.extend(other.record_members);
        self.tuple_indices.extend(other.tuple_indices);
        self
    }
}

fn find_struct<'a>(structs: &'a [(Sym, StructDef)], tag: &Sym) -> Option<&'a StructDef> {
    structs.iter().find(|(s, _)| s == tag).map(|(_, d)| d)
}

pub(super) fn render(sym: &Sym) -> String {
    match &sym.name {
        Some(n) => n.clone(),
        None => sym.num.to_string(),
    }
}

fn analyze_it(sym: &Sym, it: &IndexTerm) -> Usage {
    let one = |t: &IndexTerm| analyze_it(sym, t);
    let two = |a: &IndexTerm, b: &IndexTerm| analyze_it(sym, a).merge(analyze_it(sym, b));
    match &it.term {
        Term::Sym(s) if s == sym => Usage::direct(),
        Term::StructMember(base, member) => match &base.term {
            Term::Sym(s) if s == sym => Usage {
                struct_members: BTreeSet::from([member.clone()]),
                ..Usage::default()
            },
            _ => one(base),
        },
        Term::RecordMember(base, member) => match &base.term {
            Term::Sym(s) if s == sym => Usage {
                record_members: BTreeSet::from([member.clone()]),
                ..Usage::default()
            },
            _ => one(base),
        },
        Term::NthTuple(n, base) => match &base.term {
            Term::Sym(s) if s == sym => Usage {
                tuple_indices: vec![*n],
                ..Usage::default()
            },
            _ => one(base),
        },
        Term::Const(_)
        | Term::Sym(_)
        | Term::SizeOf(_)
        | Term::OffsetOf(_, _)
        | Term::Nil(_)
        | Term::CnNone(_) => Usage::default(),
        Term::Unop(_, t)
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
        | Term::GetOpt(t) => one(t),
        Term::EachI { body, .. } | Term::MapDef(_, body) => one(body),
        Term::Binop(_, a, b)
        | Term::Cons(a, b)
        | Term::MapGet(a, b)
        | Term::CopyAllocId { addr: a, loc: b }
        | Term::Aligned { t: a, align: b }
        | Term::StructUpdate {
            target: a,
            value: b,
            ..
        }
        | Term::RecordUpdate {
            target: a,
            value: b,
            ..
        }
        | Term::ArrayShift {
            base: a, index: b, ..
        } => two(a, b),
        Term::Let((_, a), b) => two(a, b),
        Term::Ite(a, b, c) | Term::MapSet(a, b, c) => two(a, b).merge(one(c)),
        Term::Tuple(ts) | Term::Apply(_, ts) => {
            ts.iter().fold(Usage::default(), |u, t| u.merge(one(t)))
        }
        Term::Struct(_, ms) | Term::Record(ms) | Term::Constructor(_, ms) => ms
            .iter()
            .fold(Usage::default(), |u, (_, t)| u.merge(one(t))),
        Term::Match(scrutinee, cases) => cases
            .iter()
            .fold(one(scrutinee), |u, (_, body)| u.merge(one(body))),
    }
}

fn analyze_lc(sym: &Sym, lc: &LogicalConstraint) -> Usage {
    match lc {
        LogicalConstraint::T(it) => analyze_it(sym, it),
        LogicalConstraint::Forall {
            permission, body, ..
        } => analyze_it(sym, permission).merge(analyze_it(sym, body)),
    }
}

fn analyze_gt(sym: &Sym, gt: &GenTerm) -> Usage {
    match &gt.term {
        GenTermKind::Arbitrary => Usage::default(),
        GenTermKind::WriteChoice { value, rest, .. } => {
            analyze_it(sym, value).merge(analyze_gt(sym, rest))
        }
        GenTermKind::Call { args, .. } => args
            .iter()
            .fold(Usage::default(), |u, a| u.merge(analyze_it(sym, a))),
        GenTermKind::Asgn {
            addr, value, rest, ..
        } => analyze_it(sym, addr)
            .merge(analyze_it(sym, value))
            .merge(analyze_gt(sym, rest)),
        GenTermKind::LetStar { value, rest, .. } => {
            analyze_gt(sym, value).merge(analyze_gt(sym, rest))
        }
        GenTermKind::Return(it) => analyze_it(sym, it),
        GenTermKind::Assert { constraint, rest } => {
            analyze_lc(sym, constraint).merge(analyze_gt(sym, rest))
        }
        GenTermKind::Ite { cond, then_, else_ } => analyze_it(sym, cond)
            .merge(analyze_gt(sym, then_))
            .merge(analyze_gt(sym, else_)),
        GenTermKind::Map { perm, body, .. } => analyze_it(sym, perm).merge(analyze_gt(sym, body)),
        GenTermKind::Scope { name, body } => {
            let name_usage = match name {
                ScopeName::Str(_) | ScopeName::Sid(_) => Usage::default(),
                ScopeName::Index(it) => analyze_it(sym, it),
            };
            name_usage.merge(analyze_gt(sym, body))
        }
    }
}

fn struct_request(sym: &Sym, members: Vec<Id>, term: &GenTerm) -> PruneRequest {
    let analysis = analyze_gt(sym, term);
    if analysis.used_directly {
        return PruneRequest::Nothing;
    }
    let all: BTreeSet<Id> = members.into_iter().collect();
    let unused: Vec<Id> = all.difference(&analysis.struct_members).cloned().collect();
    if unused.is_empty() {
        PruneRequest::Nothing
    } else {
        PruneRequest::Struct(unused)
    }
}

fn record_request(sym: &Sym, members: Vec<Id>, term: &GenTerm) -> PruneRequest {
    let analysis = analyze_gt(sym, term);
    if analysis.used_directly {
        return PruneRequest::Nothing;
    }
    let all: BTreeSet<Id> = members.into_iter().collect();
    let unused: Vec<Id> = all.difference(&analysis.record_members).cloned().collect();
    if unused.is_empty() {
        PruneRequest::Nothing
    } else {
        PruneRequest::Record(unused)
    }
}

fn tuple_request(sym: &Sym, length: usize, term: &GenTerm) -> PruneRequest {
    let analysis = analyze_gt(sym, term);
    if analysis.used_directly {
        return PruneRequest::Nothing;
    }
    let used: BTreeSet<usize> = analysis.tuple_indices.iter().copied().collect();
    let unused: Vec<usize> = (0..length).filter(|i| !used.contains(i)).collect();
    if unused.is_empty() {
        PruneRequest::Nothing
    } else {
        PruneRequest::Tuple(unused)
    }
}

fn get_request(
    structs: &[(Sym, StructDef)],
    sym: &Sym,
    bt: &BaseType,
    term: &GenTerm,
) -> Result<PruneRequest, TransformError> {
    if !term.free_vars().contains(sym) {
        return Ok(PruneRequest::Entire);
    }
    Ok(match bt {
        BaseType::Struct(tag) => {
            let def = find_struct(structs, tag)
                .ok_or_else(|| TransformError::UnknownStruct { tag: tag.clone() })?;
            struct_request(
                sym,
                def.members.iter().map(|(id, _)| id.clone()).collect(),
                term,
            )
        }
        BaseType::Record(members) => record_request(
            sym,
            members.iter().map(|(id, _)| id.clone()).collect(),
            term,
        ),
        BaseType::Tuple(items) => tuple_request(sym, items.len(), term),
        _ => PruneRequest::Nothing,
    })
}

type Requests = SymMap<Vec<PruneRequest>>;

fn merge_requests(mut a: Requests, b: Requests) -> Requests {
    for (k, mut v) in b {
        a.entry(k).or_default().append(&mut v);
    }
    a
}

fn get_requests(structs: &[(Sym, StructDef)], gt: &GenTerm) -> Result<Requests, TransformError> {
    Ok(match &gt.term {
        GenTermKind::LetStar { var, value, rest } => {
            let from_rest = get_requests(structs, rest)?;
            if let GenTermKind::Call { fsym, .. } = &value.term {
                let request = get_request(structs, var, &value.bt, rest)?;
                merge_requests(SymMap::from([(fsym.clone(), vec![request])]), from_rest)
            } else {
                merge_requests(get_requests(structs, value)?, from_rest)
            }
        }
        GenTermKind::Asgn { rest, .. }
        | GenTermKind::WriteChoice { rest, .. }
        | GenTermKind::Assert { rest, .. } => get_requests(structs, rest)?,
        GenTermKind::Ite { then_, else_, .. } => {
            merge_requests(get_requests(structs, then_)?, get_requests(structs, else_)?)
        }
        GenTermKind::Map { body, .. } | GenTermKind::Scope { body, .. } => {
            get_requests(structs, body)?
        }
        GenTermKind::Call { fsym, .. } => {
            SymMap::from([(fsym.clone(), vec![PruneRequest::Nothing])])
        }
        GenTermKind::Arbitrary | GenTermKind::Return(_) => Requests::new(),
    })
}

fn modify_return_type(bt: BaseType, request: &PruneRequest) -> BaseType {
    match request {
        PruneRequest::Entire => BaseType::Unit,
        PruneRequest::Nothing | PruneRequest::Struct(_) => bt,
        PruneRequest::Record(remove) => match bt {
            BaseType::Record(members) => BaseType::Record(
                members
                    .into_iter()
                    .filter(|(id, _)| !remove.contains(id))
                    .collect(),
            ),
            other => other,
        },
        PruneRequest::Tuple(remove) => match bt {
            BaseType::Tuple(items) => BaseType::Tuple(
                items
                    .into_iter()
                    .enumerate()
                    .filter(|(i, _)| !remove.contains(i))
                    .map(|(_, bt)| bt)
                    .collect(),
            ),
            other => other,
        },
    }
}

fn default_of(sct: &Sctype) -> Result<IndexTerm, TransformError> {
    let bt = sct
        .to_base_type()
        .ok_or_else(|| TransformError::UnsupportedSctype { ct: sct.clone() })?;
    Ok(IndexTerm {
        term: Term::Const(Const::Unknown(bt.clone())),
        bt,
    })
}

fn rewrite_return(
    structs: &[(Sym, StructDef)],
    oarg: &BaseType,
    request: &PruneRequest,
    it: IndexTerm,
    node_bt: BaseType,
) -> Result<GenTerm, TransformError> {
    let unchanged = |it: IndexTerm, bt: BaseType| GenTerm::from_parts(GenTermKind::Return(it), bt);
    match request {
        PruneRequest::Entire => Ok(GenTerm::return_(IndexTerm::unit())),
        PruneRequest::Nothing => Ok(unchanged(it, node_bt)),
        PruneRequest::Struct(remove) => {
            let BaseType::Struct(oarg_tag) = oarg else {
                return Ok(unchanged(it, node_bt));
            };
            let def =
                find_struct(structs, oarg_tag).ok_or_else(|| TransformError::UnknownStruct {
                    tag: oarg_tag.clone(),
                })?;
            let Term::Struct(tag, members) = &it.term else {
                return Ok(unchanged(it, node_bt));
            };
            let mut new_members = Vec::new();
            for (id, sct) in &def.members {
                let value = if remove.contains(id) {
                    default_of(sct)?
                } else {
                    match members.iter().find(|(mid, _)| mid == id) {
                        Some((_, v)) => v.clone(),
                        None => default_of(sct)?,
                    }
                };
                new_members.push((id.clone(), value));
            }
            Ok(GenTerm::return_(IndexTerm::struct_(
                tag.clone(),
                new_members,
            )))
        }
        PruneRequest::Record(remove) => {
            let Term::Record(members) = &it.term else {
                return Ok(unchanged(it, node_bt));
            };
            let kept: Vec<(Id, IndexTerm)> = members
                .iter()
                .filter(|(id, _)| !remove.contains(id))
                .cloned()
                .collect();
            let bt = BaseType::Record(
                kept.iter()
                    .map(|(id, t)| (id.clone(), t.bt.clone()))
                    .collect(),
            );
            Ok(GenTerm::return_(IndexTerm {
                term: Term::Record(kept),
                bt,
            }))
        }
        PruneRequest::Tuple(remove) => {
            let Term::Tuple(items) = &it.term else {
                return Ok(unchanged(it, node_bt));
            };
            let kept: Vec<IndexTerm> = items
                .iter()
                .enumerate()
                .filter(|(i, _)| !remove.contains(i))
                .map(|(_, t)| t.clone())
                .collect();
            let bt = BaseType::Tuple(kept.iter().map(|t| t.bt.clone()).collect());
            Ok(GenTerm::return_(IndexTerm {
                term: Term::Tuple(kept),
                bt,
            }))
        }
    }
}

fn transform_body(
    structs: &[(Sym, StructDef)],
    oarg: &BaseType,
    request: &PruneRequest,
    gt: GenTerm,
) -> Result<GenTerm, TransformError> {
    map_tail(gt, &mut |tail| {
        let (term, bt) = tail.into_parts();
        match term {
            GenTermKind::Return(it) => rewrite_return(structs, oarg, request, it, bt),
            term => Ok(GenTerm::from_parts(term, bt)),
        }
    })
}

fn create_variant(
    structs: &[(Sym, StructDef)],
    syms: &mut SymGen,
    gd: &GenDef,
    request: &PruneRequest,
) -> Result<GenDef, TransformError> {
    let name_str = format!("{}_pruned{}", render(&gd.name), syms.watermark());
    let name = syms.fresh(&name_str);
    let oarg = modify_return_type(gd.oarg.clone(), request);
    let body = transform_body(structs, &gd.oarg, request, gd.body.clone())?;
    Ok(GenDef {
        name,
        oarg,
        body,
        ..gd.clone()
    })
}

fn adapt_one(sym: &Sym, new_bt: &BaseType, request: &PruneRequest, it: IndexTerm) -> IndexTerm {
    let (term, bt) = it.into_parts();
    match term {
        Term::Sym(s) if s == *sym => IndexTerm::sym(s, new_bt.clone()),
        Term::NthTuple(n, base) if matches!(&base.term, Term::Sym(s) if s == sym) => {
            if let PruneRequest::Tuple(remove) = request {
                debug_assert!(
                    !remove.contains(&n),
                    "a pruned tuple index survived into an adapted use"
                );
                let new_n = n - remove.iter().filter(|i| **i < n).count();
                IndexTerm {
                    term: Term::NthTuple(new_n, base),
                    bt,
                }
            } else {
                IndexTerm {
                    term: Term::NthTuple(n, base),
                    bt,
                }
            }
        }
        term => IndexTerm { term, bt },
    }
}

fn adapt_symbol_uses(
    sym: &Sym,
    new_bt: &BaseType,
    request: &PruneRequest,
    term: GenTerm,
) -> GenTerm {
    match request {
        PruneRequest::Struct(_) | PruneRequest::Nothing => term,
        PruneRequest::Entire | PruneRequest::Record(_) | PruneRequest::Tuple(_) => {
            adapt_term(sym, new_bt, request, term)
        }
    }
}

fn adapt_it(sym: &Sym, new_bt: &BaseType, request: &PruneRequest, it: IndexTerm) -> IndexTerm {
    it.map_post(&mut |t| adapt_one(sym, new_bt, request, t))
}

fn adapt_lc(
    sym: &Sym,
    new_bt: &BaseType,
    request: &PruneRequest,
    lc: LogicalConstraint,
) -> LogicalConstraint {
    match lc {
        LogicalConstraint::T(it) => LogicalConstraint::T(adapt_it(sym, new_bt, request, it)),
        LogicalConstraint::Forall {
            var,
            permission,
            body,
        } => LogicalConstraint::Forall {
            var,
            permission: adapt_it(sym, new_bt, request, permission),
            body: adapt_it(sym, new_bt, request, body),
        },
    }
}

fn adapt_term(sym: &Sym, new_bt: &BaseType, request: &PruneRequest, gt: GenTerm) -> GenTerm {
    let ad = |it: IndexTerm| adapt_it(sym, new_bt, request, it);
    let (term, bt) = gt.into_parts();
    match term {
        GenTermKind::Arbitrary => GenTerm::from_parts(term, bt),
        GenTermKind::WriteChoice { label, value, rest } => {
            GenTerm::write_choice(label, ad(value), adapt_term(sym, new_bt, request, *rest))
        }
        GenTermKind::Call { fsym, args } => {
            GenTerm::call(fsym, args.into_iter().map(ad).collect(), bt)
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => GenTerm::asgn(
            ad(addr),
            ct,
            ad(value),
            adapt_term(sym, new_bt, request, *rest),
        ),
        GenTermKind::Return(it) => GenTerm::return_(ad(it)),
        GenTermKind::Assert { constraint, rest } => GenTerm::assert(
            adapt_lc(sym, new_bt, request, constraint),
            adapt_term(sym, new_bt, request, *rest),
        ),
        GenTermKind::Ite { cond, then_, else_ } => GenTerm::rebuild_ite(
            ad(cond),
            adapt_term(sym, new_bt, request, *then_),
            adapt_term(sym, new_bt, request, *else_),
        ),
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => GenTerm::map(
            (var, var_bt, ad(perm)),
            adapt_term(sym, new_bt, request, *body),
        ),
        GenTermKind::LetStar { var, value, rest } => GenTerm::let_star(
            (var, adapt_term(sym, new_bt, request, *value)),
            adapt_term(sym, new_bt, request, *rest),
        ),
        GenTermKind::Scope { name, body } => {
            let name = match name {
                name @ (ScopeName::Str(_) | ScopeName::Sid(_)) => name,
                ScopeName::Index(it) => ScopeName::Index(ad(it)),
            };
            GenTerm::scope(name, adapt_term(sym, new_bt, request, *body))
        }
    }
}

type VariantsMap = SymMap<Vec<(PruneRequest, Sym, BaseType)>>;

fn update_call_sites(
    structs: &[(Sym, StructDef)],
    variants: &VariantsMap,
    gt: GenTerm,
) -> Result<GenTerm, TransformError> {
    let (term, bt) = gt.into_parts();
    Ok(match term {
        GenTermKind::LetStar { var, value, rest } => {
            let updated_rest = update_call_sites(structs, variants, *rest)?;
            let (value_term, inner_bt) = value.into_parts();
            if let GenTermKind::Call { fsym, args } = value_term {
                if let Some(list) = variants.get(&fsym) {
                    let local = get_request(structs, &var, &inner_bt, &updated_rest)?;
                    if let Some((_, vname, voarg)) = list.iter().find(|(req, _, _)| *req == local) {
                        let call = GenTerm::call(vname.clone(), args, voarg.clone());
                        let adapted = adapt_symbol_uses(&var, voarg, &local, updated_rest);
                        return Ok(GenTerm::let_star((var, call), adapted));
                    }
                }
                GenTerm::let_star((var, GenTerm::call(fsym, args, inner_bt)), updated_rest)
            } else {
                let value = update_call_sites(
                    structs,
                    variants,
                    GenTerm::from_parts(value_term, inner_bt),
                )?;
                GenTerm::let_star((var, value), updated_rest)
            }
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => GenTerm::asgn(
            addr,
            ct,
            value,
            update_call_sites(structs, variants, *rest)?,
        ),
        GenTermKind::Assert { constraint, rest } => {
            GenTerm::assert(constraint, update_call_sites(structs, variants, *rest)?)
        }
        GenTermKind::WriteChoice { label, value, rest } => {
            GenTerm::write_choice(label, value, update_call_sites(structs, variants, *rest)?)
        }
        GenTermKind::Ite { cond, then_, else_ } => GenTerm::rebuild_ite(
            cond,
            update_call_sites(structs, variants, *then_)?,
            update_call_sites(structs, variants, *else_)?,
        ),
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => GenTerm::map(
            (var, var_bt, perm),
            update_call_sites(structs, variants, *body)?,
        ),
        GenTermKind::Scope { name, body } => {
            GenTerm::scope(name, update_call_sites(structs, variants, *body)?)
        }
        term @ (GenTermKind::Arbitrary | GenTermKind::Return(_) | GenTermKind::Call { .. }) => {
            GenTerm::from_parts(term, bt)
        }
    })
}

fn callees(gt: &GenTerm, out: &mut SymSet) {
    match &gt.term {
        GenTermKind::Arbitrary | GenTermKind::Return(_) => {}
        GenTermKind::Call { fsym, .. } => {
            out.insert(fsym.clone());
        }
        GenTermKind::Asgn { rest, .. }
        | GenTermKind::WriteChoice { rest, .. }
        | GenTermKind::Assert { rest, .. } => callees(rest, out),
        GenTermKind::LetStar { value, rest, .. } => {
            callees(value, out);
            callees(rest, out);
        }
        GenTermKind::Ite { then_, else_, .. } => {
            callees(then_, out);
            callees(else_, out);
        }
        GenTermKind::Map { body, .. } | GenTermKind::Scope { body, .. } => callees(body, out),
    }
}

pub fn prune_returns(
    defs: &[(Sym, GenDef)],
    structs: &[(Sym, StructDef)],
    syms: &mut SymGen,
) -> Result<Vec<(Sym, GenDef)>, TransformError> {
    let mut all_requests = Requests::new();
    for (_, gd) in defs {
        all_requests = merge_requests(all_requests, get_requests(structs, &gd.body)?);
    }
    for reqs in all_requests.values_mut() {
        reqs.sort();
        reqs.dedup();
    }
    let mut with_variants: Vec<Vec<GenDef>> = Vec::new();
    let mut variants_map: VariantsMap = SymMap::new();
    for (sym, gd) in defs {
        let list: Vec<(PruneRequest, GenDef)> = match all_requests.get(sym) {
            None => vec![(PruneRequest::Nothing, gd.clone())],
            Some(requests) => {
                let has_nothing = requests.contains(&PruneRequest::Nothing);
                let mut list = Vec::new();
                if has_nothing {
                    list.push((PruneRequest::Nothing, gd.clone()));
                }
                for req in requests.iter().filter(|r| **r != PruneRequest::Nothing) {
                    list.push((req.clone(), create_variant(structs, syms, gd, req)?));
                }
                list
            }
        };
        variants_map.insert(
            sym.clone(),
            list.iter()
                .map(|(req, d)| (req.clone(), d.name.clone(), d.oarg.clone()))
                .collect(),
        );
        with_variants.push(list.into_iter().map(|(_, d)| d).collect());
    }
    let mut out: Vec<(Sym, GenDef)> = Vec::new();
    for list in with_variants {
        for mut gd in list {
            let body = std::mem::replace(&mut gd.body, GenTerm::return_(IndexTerm::unit()));
            gd.body = update_call_sites(structs, &variants_map, body)?;
            out.push((gd.name.clone(), gd));
        }
    }
    if cfg!(debug_assertions) {
        let inputs: SymSet = defs.iter().map(|(s, _)| s.clone()).collect();
        let outputs: SymSet = out.iter().map(|(s, _)| s.clone()).collect();
        for (_, gd) in &out {
            let mut called = SymSet::new();
            callees(&gd.body, &mut called);
            for fsym in called {
                debug_assert!(
                    outputs.contains(&fsym) || !inputs.contains(&fsym),
                    "call to {fsym:?} no longer resolves after prune_returns"
                );
            }
        }
    }
    Ok(out)
}
