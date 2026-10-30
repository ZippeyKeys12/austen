use crate::gen_dsl::{
    BaseType, FunctionBody, IndexTerm, LogicalConstraint, Sym, SymMap, SymSet, Term,
};

use crate::spec::{ArgType, Lat, Request, SpecModule};

pub(in crate::spec) fn needs_global_closure(module: &SpecModule) -> bool {
    !module.globals.is_empty() && !module.logical_functions.is_empty()
}

pub(in crate::spec) fn close_logical_function_globals(module: &mut SpecModule) {
    if !needs_global_closure(module) {
        return;
    }

    let declared: SymSet = module
        .globals
        .iter()
        .map(|global| global.address.clone())
        .collect();
    let explicit: SymMap<SymSet> = module
        .logical_functions
        .iter()
        .map(|function| {
            (
                function.name.clone(),
                function
                    .args
                    .iter()
                    .map(|(arg, _)| arg.clone())
                    .filter(|arg| declared.contains(arg))
                    .collect(),
            )
        })
        .collect();

    let mut dependencies: SymMap<SymSet> = SymMap::new();
    let mut callees: SymMap<SymSet> = SymMap::new();
    for function in &module.logical_functions {
        let used = dependencies.entry(function.name.clone()).or_default();
        let called = callees.entry(function.name.clone()).or_default();
        let Some(body) = &function.body else {
            continue;
        };
        let mut free = body.term().free_vars_bts();
        for (arg, _) in &function.args {
            free.remove(arg);
        }
        used.extend(free.into_keys().filter(|sym| declared.contains(sym)));
        collect_applies(body.term(), called);
    }

    let dependencies =
        crate::spec::convert::close_over_callees(dependencies, &callees, |name, used| {
            if let Some(already_explicit) = explicit.get(name) {
                used.retain(|global| !already_explicit.contains(global));
            }
        });

    let suffixes: SymMap<Vec<(Sym, BaseType)>> = dependencies
        .into_iter()
        .map(|(name, used)| {
            let suffix = module
                .globals
                .iter()
                .filter(|global| used.contains(&global.address))
                .map(|global| (global.address.clone(), BaseType::loc_to(global.ty.clone())))
                .collect();
            (name, suffix)
        })
        .collect();

    for function in &mut module.logical_functions {
        if let Some(suffix) = suffixes.get(&function.name) {
            function.args.extend(suffix.iter().cloned());
        }
        if let Some(body) = &mut function.body {
            let body = match body {
                FunctionBody::Def(body) | FunctionBody::RecDef(body) => body,
            };
            rewrite_index_term(body, &suffixes);
        }
    }

    for predicate in &mut module.predicates {
        for clause in predicate.clauses.iter_mut().flatten() {
            rewrite_index_term(&mut clause.guard, &suffixes);
            rewrite_lat(&mut clause.packing_ft, &suffixes, &mut |terminal| {
                rewrite_index_term(terminal, &suffixes)
            });
        }
    }
    for function in &mut module.functions {
        if let Some(internal) = &mut function.internal {
            rewrite_arg_type(internal, &suffixes);
        }
        rewrite_lat(&mut function.ensures.body, &suffixes, &mut |_| {});
    }
}

pub(in crate::spec) fn collect_applies(term: &IndexTerm, out: &mut SymSet) {
    let _ = term.clone().map_post(&mut |node| {
        if let Term::Apply(name, _) = &node.term {
            out.insert(name.clone());
        }
        node
    });
}

fn rewrite_index_term(term: &mut IndexTerm, suffixes: &SymMap<Vec<(Sym, BaseType)>>) {
    let owned = std::mem::replace(term, IndexTerm::unit());
    *term = owned.map_post(&mut |mut node| {
        if let Term::Apply(name, args) = &mut node.term
            && let Some(suffix) = suffixes.get(name)
        {
            args.extend(
                suffix
                    .iter()
                    .map(|(sym, bt)| IndexTerm::sym(sym.clone(), bt.clone())),
            );
        }
        node
    });
}

fn rewrite_constraint(constraint: &mut LogicalConstraint, suffixes: &SymMap<Vec<(Sym, BaseType)>>) {
    match constraint {
        LogicalConstraint::T(term) => rewrite_index_term(term, suffixes),
        LogicalConstraint::Forall {
            permission, body, ..
        } => {
            rewrite_index_term(permission, suffixes);
            rewrite_index_term(body, suffixes);
        }
    }
}

fn rewrite_request(request: &mut Request, suffixes: &SymMap<Vec<(Sym, BaseType)>>) {
    match request {
        Request::P { pointer, iargs, .. } => {
            rewrite_index_term(pointer, suffixes);
            for arg in iargs {
                rewrite_index_term(arg, suffixes);
            }
        }
        Request::Q {
            pointer,
            permission,
            iargs,
            ..
        } => {
            rewrite_index_term(pointer, suffixes);
            rewrite_index_term(permission, suffixes);
            for arg in iargs {
                rewrite_index_term(arg, suffixes);
            }
        }
    }
}

fn rewrite_lat<I>(
    lat: &mut Lat<I>,
    suffixes: &SymMap<Vec<(Sym, BaseType)>>,
    terminal: &mut impl FnMut(&mut I),
) {
    let mut node = lat;
    loop {
        node = match node {
            Lat::Define((_, value), rest) => {
                rewrite_index_term(value, suffixes);
                rest
            }
            Lat::Resource((_, (request, _)), rest) => {
                rewrite_request(request, suffixes);
                rest
            }
            Lat::Constraint(constraint, rest) => {
                rewrite_constraint(constraint, suffixes);
                rest
            }
            Lat::I(value) => {
                terminal(value);
                return;
            }
        };
    }
}

fn rewrite_arg_type(arg_type: &mut ArgType<()>, suffixes: &SymMap<Vec<(Sym, BaseType)>>) {
    let mut node = arg_type;
    loop {
        node = match node {
            ArgType::Computational(_, rest) | ArgType::Ghost(_, rest) => rest,
            ArgType::L(lat) => {
                rewrite_lat(lat, suffixes, &mut |_| {});
                return;
            }
        };
    }
}
