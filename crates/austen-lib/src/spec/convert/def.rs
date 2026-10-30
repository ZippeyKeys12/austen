use crate::gen_dsl::{GenDef, GenTerm, Id, IndexTerm, Sym, SymMap, SymSet};
use crate::spec::{ArgType, Clause, ClauseShapeError, FunctionSpec, Lat, PredicateDef, SpecModule};

use crate::spec::convert::lat::transform_it_lat;
use crate::spec::convert::{ConvertError, Cx};

pub fn member_id(sym: &Sym) -> Id {
    Id(sym.name.clone().unwrap_or_else(|| sym.num.to_string()))
}

fn retain_declared_globals(vars: SymMap<crate::gen_dsl::BaseType>, declared: &SymSet) -> SymSet {
    vars.into_keys()
        .filter(|sym| declared.contains(sym))
        .collect()
}

pub(in crate::spec) fn predicate_global_dependencies(module: &SpecModule) -> SymMap<SymSet> {
    let declared: SymSet = module
        .globals
        .iter()
        .map(|global| global.address.clone())
        .collect();
    let dependencies: SymMap<SymSet> = module
        .predicates
        .iter()
        .map(|pred| {
            let mut used = SymSet::new();
            for clause in pred.clauses.iter().flatten() {
                used.extend(retain_declared_globals(
                    clause.guard.free_vars_bts(),
                    &declared,
                ));
                used.extend(retain_declared_globals(
                    clause
                        .packing_ft
                        .free_vars_bts(&|terminal| terminal.free_vars_bts()),
                    &declared,
                ));
            }
            (pred.name.clone(), used)
        })
        .collect();

    let mut callees: SymMap<SymSet> = SymMap::new();
    for pred in &module.predicates {
        callees
            .entry(pred.name.clone())
            .or_default()
            .extend(pred.callees());
    }
    crate::spec::convert::close_over_callees(dependencies, &callees, |_, _| {})
}

fn spec_global_dependencies(cx: &Cx<'_>, lat: &Lat<()>) -> SymSet {
    let declared = cx.global_symbols();
    let mut used = retain_declared_globals(lat.free_vars_bts(&|()| SymMap::new()), &declared);
    for (_, (request, _)) in lat.resource_requests() {
        if let Some(callee) = request.predicate_name()
            && let Some(callee_globals) = cx.predicate_globals.get(callee)
        {
            used.extend(callee_globals.iter().cloned());
        }
    }
    used
}

fn transform_clauses(
    cx: &mut Cx<'_>,
    pred: &PredicateDef,
    generated: &SymSet,
) -> Result<GenTerm, ConvertError> {
    let name = &pred.name;
    let (first, rest) = match pred.catch_all_clauses() {
        None => {
            return Err(ConvertError::UninterpretedPredicate { name: name.clone() });
        }
        Some(Err(ClauseShapeError::Empty)) | Some(Ok([])) => {
            return Err(ConvertError::NoClauses { name: name.clone() });
        }
        Some(Err(ClauseShapeError::GuardedLast)) => {
            return Err(ConvertError::GuardedLastClause { name: name.clone() });
        }
        Some(Ok([first, rest @ ..])) => (first, rest),
    };
    transform_clause_chain(cx, name, generated, first, rest)
}

fn transform_clause_chain(
    cx: &mut Cx<'_>,
    name: &Sym,
    generated: &SymSet,
    clause: &Clause,
    rest: &[Clause],
) -> Result<GenTerm, ConvertError> {
    let Some((next, tail)) = rest.split_first() else {
        return transform_it_lat(cx, generated, &clause.packing_ft);
    };
    let then_ = transform_it_lat(cx, generated, &clause.packing_ft)?;
    let else_ = transform_clause_chain(cx, name, generated, next, tail)?;
    GenTerm::try_ite(clause.guard.clone(), then_, else_).map_err(|(then_, else_)| {
        ConvertError::IteBranchMismatch {
            name: name.clone(),
            then_,
            else_,
        }
    })
}

pub fn transform_pred(cx: &mut Cx<'_>, pred: &PredicateDef) -> Result<GenDef, ConvertError> {
    let mut iargs = pred.generator_iargs();
    iargs.extend(cx.predicate_global_iargs(&pred.name));
    let generated: SymSet = iargs.iter().map(|(sym, _)| sym.clone()).collect();
    let body = transform_clauses(cx, pred, &generated)?;
    Ok(GenDef {
        filename: cx.module.filename.clone(),
        recursive: cx.recursive.contains(&pred.name),
        spec: false,
        name: pred.name.clone(),
        iargs,
        oarg: pred.oarg.clone(),
        c_types: None,
        size_arg: None,
        rec_size_arg: None,
        heap_base_arg: None,
        heap_size_arg: None,
        body,
    })
}

pub fn transform_spec(
    cx: &mut Cx<'_>,
    name: Sym,
    at: &ArgType<()>,
) -> Result<GenDef, ConvertError> {
    let lat = at.lat();
    let computational = at.computational();
    let declared_globals = cx.global_symbols();
    let used_globals = spec_global_dependencies(cx, lat);
    let iargs = cx.globals_in(&used_globals);
    let mut vars: Vec<_> = computational
        .iter()
        .filter(|(sym, _)| !declared_globals.contains(sym))
        .cloned()
        .collect();
    for (sym, bt) in lat.free_vars_bts(&|()| SymMap::new()) {
        if !declared_globals.contains(&sym) && !vars.iter().any(|(known, _)| *known == sym) {
            vars.push((sym, bt));
        }
    }

    let ret = IndexTerm::record(
        vars.iter()
            .map(|(sym, bt)| (member_id(sym), IndexTerm::sym(sym.clone(), bt.clone())))
            .collect(),
    )
    .map_err(|id| ConvertError::DuplicateRecordMember { id })?;
    let oarg = ret.bt.clone();

    let lat = lat.clone().map(&mut |()| ret.clone());
    let generated: SymSet = iargs.iter().map(|(sym, _)| sym.clone()).collect();
    let body = transform_it_lat(cx, &generated, &lat)?;

    Ok(GenDef {
        filename: cx.module.filename.clone(),
        recursive: false,
        spec: true,
        name,
        iargs,
        oarg,
        c_types: None,
        size_arg: None,
        rec_size_arg: None,
        heap_base_arg: None,
        heap_size_arg: None,
        body,
    })
}

pub fn recursive_predicates(module: &SpecModule) -> SymSet {
    let edges: SymMap<Vec<Sym>> = module
        .predicates
        .iter()
        .map(|pred| (pred.name.clone(), pred.callees()))
        .collect();
    self_reaching(&edges)
}

pub(in crate::spec) fn self_reaching(edges: &SymMap<Vec<Sym>>) -> SymSet {
    edges
        .keys()
        .filter(|name| reaches_itself(edges, name))
        .cloned()
        .collect()
}

fn reaches_itself(edges: &SymMap<Vec<Sym>>, start: &Sym) -> bool {
    let mut seen = SymSet::new();
    let mut stack: Vec<Sym> = edges.get(start).cloned().unwrap_or_default();
    while let Some(node) = stack.pop() {
        if node == *start {
            return true;
        }
        if !seen.insert(node.clone()) {
            continue;
        }
        stack.extend(edges.get(&node).cloned().unwrap_or_default());
    }
    false
}

pub fn spec_arg_type(func: &FunctionSpec) -> Result<ArgType<()>, ConvertError> {
    if let Some(at) = &func.internal {
        return Ok(at.clone());
    }
    let mut at = ArgType::L(crate::spec::Lat::I(()));
    for (i, param) in func.params.iter().enumerate().rev() {
        let bt = crate::spec::convert::param_base_type(&param.ty)
            .ok_or_else(|| ConvertError::UnsupportedParam(param.ty.clone()))?;
        let sym = Sym {
            name: Some(param.name.clone()),
            num: i as u64,
        };
        at = ArgType::Computational((sym, bt), Box::new(at));
    }
    Ok(at)
}
