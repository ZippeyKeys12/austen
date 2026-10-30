use std::collections::{BTreeSet, VecDeque};

use crate::gen_dsl::{
    BaseType, Const, DefaultError, DefaultTypes, FunctionBody, IndexTerm, LogicalConstraint, Sym,
    SymSet, Term,
};

use crate::spec::post::Post;
use crate::spec::{ArgType, Clause, Ensures, Lat, PredicateDef, Request, RequestName, SpecModule};

pub(super) fn collect_post_defaults_with_unsupported(
    module: &SpecModule,
    ens: &Ensures,
) -> (DefaultTypes, Option<DefaultError>) {
    DefaultTypes::supported(collect_post_default_types(module, ens))
}

fn collect_post_default_types(module: &SpecModule, ens: &Ensures) -> BTreeSet<BaseType> {
    struct Walk<'a> {
        module: &'a SpecModule,
        types: BTreeSet<BaseType>,
        predicates: SymSet,
        functions: SymSet,
        predicate_queue: VecDeque<Sym>,
        function_queue: VecDeque<Sym>,
    }
    impl Walk<'_> {
        fn term(&mut self, term: &IndexTerm) {
            let _ = term.clone().map_post(&mut |node| {
                match &node.term {
                    Term::Const(Const::InputDefault(ty)) => {
                        self.types.insert(ty.clone());
                    }
                    Term::Apply(name, _) if self.functions.insert(name.clone()) => {
                        self.function_queue.push_back(name.clone());
                    }
                    _ => {}
                }
                node
            });
        }
        fn chain<I>(&mut self, chain: &Lat<I>, terminal: impl FnOnce(&mut Self, &I)) {
            let mut node = chain;
            loop {
                node = match node {
                    Lat::Define((_, term), rest) => {
                        self.term(term);
                        rest
                    }
                    Lat::Constraint(constraint, rest) => {
                        match constraint {
                            LogicalConstraint::T(term) => self.term(term),
                            LogicalConstraint::Forall {
                                permission, body, ..
                            } => {
                                self.term(permission);
                                self.term(body);
                            }
                        }
                        rest
                    }
                    Lat::Resource((_, (request, _)), rest) => {
                        if let Some(name) = request.predicate_name()
                            && self.predicates.insert(name.clone())
                        {
                            self.predicate_queue.push_back(name.clone());
                        }
                        let (pointer, iargs) = match request {
                            Request::P { pointer, iargs, .. } => (pointer, iargs),
                            Request::Q {
                                pointer,
                                permission,
                                iargs,
                                ..
                            } => {
                                self.term(permission);
                                (pointer, iargs)
                            }
                        };
                        self.term(pointer);
                        for arg in iargs {
                            self.term(arg);
                        }
                        rest
                    }
                    Lat::I(value) => {
                        terminal(self, value);
                        return;
                    }
                };
            }
        }
    }
    let mut walk = Walk {
        module,
        types: BTreeSet::new(),
        predicates: SymSet::new(),
        functions: SymSet::new(),
        predicate_queue: VecDeque::new(),
        function_queue: VecDeque::new(),
    };
    walk.chain(&ens.body, |_, _| {});
    while !walk.predicate_queue.is_empty() || !walk.function_queue.is_empty() {
        while let Some(name) = walk.predicate_queue.pop_front() {
            if let Some(pred) = walk.module.predicate(&name) {
                for clause in pred.clauses.iter().flatten() {
                    walk.term(&clause.guard);
                    walk.chain(&clause.packing_ft, |walk, term| walk.term(term));
                }
            }
        }
        while let Some(name) = walk.function_queue.pop_front() {
            if let Some(function) = walk.module.logical_function(&name)
                && let Some(FunctionBody::Def(body) | FunctionBody::RecDef(body)) = &function.body
            {
                walk.term(body);
            }
        }
    }
    walk.types
}

pub fn transform_post(module: &SpecModule, at: &ArgType<()>, ens: &Ensures) -> Post {
    let mut bound = generator_bindings(at);
    bound.extend(module.globals.iter().map(|global| global.address.clone()));
    let unresolved: Vec<String> = ens
        .free_vars_bts()
        .into_keys()
        .filter(|sym| !bound.contains(sym))
        .map(|sym| sym.name.clone().unwrap_or_else(|| sym.num.to_string()))
        .collect();
    if !unresolved.is_empty() {
        return Post::Unsupported {
            reason: format!(
                "ensures mentions variables the generator never binds: {}",
                unresolved.join(", ")
            ),
        };
    }

    let mut requested = SymSet::new();
    let mut queue = VecDeque::new();
    if let Err(reason) = validate_chain(module, &ens.body, None, &mut requested, &mut queue) {
        return Post::Unsupported { reason };
    }
    while let Some((pred, clauses)) = queue.pop_front() {
        for (clause_index, clause) in clauses.iter().enumerate() {
            let terminal_bt = &chain_terminal(&clause.packing_ft).bt;
            if *terminal_bt != pred.oarg {
                return Post::Unsupported {
                    reason: format!(
                        "ensures predicate {} clause {clause_index} returns {terminal_bt:?}, \
                         but the predicate declares {:?}",
                        display_sym(&pred.name),
                        pred.oarg,
                    ),
                };
            }
            if let Err(reason) = validate_chain(
                module,
                &clause.packing_ft,
                Some(pred),
                &mut requested,
                &mut queue,
            ) {
                return Post::Unsupported { reason };
            }
        }
    }

    Post::Check
}

fn validate_chain<'a, I>(
    module: &'a SpecModule,
    lat: &Lat<I>,
    owner: Option<&PredicateDef>,
    requested: &mut SymSet,
    queue: &mut VecDeque<(&'a PredicateDef, &'a [Clause])>,
) -> Result<(), String> {
    for (_, (request, bt)) in lat.resource_requests() {
        let value_bt = match request {
            Request::P { .. } => bt,
            Request::Q { q, permission, .. } => {
                let checked = crate::spec::post::check_quantified_request(bt, &q.1).and_then(
                    |(_, value_bt)| {
                        crate::spec::post::check_quantified_permission(permission, &q.0)
                            .map(|_| value_bt)
                    },
                );
                match checked {
                    Ok(value_bt) => value_bt,
                    Err(reason) => {
                        return Err(match owner {
                            Some(pred) => {
                                format!("ensures predicate {}: {reason}", display_sym(&pred.name))
                            }
                            None => reason.to_string(),
                        });
                    }
                }
            }
        };
        let (name, pointer, iargs) = match request {
            Request::P {
                name,
                pointer,
                iargs,
            }
            | Request::Q {
                name,
                pointer,
                iargs,
                ..
            } => (name, pointer, iargs),
        };
        if !pointer.bt.is_loc() {
            return Err(format!(
                "{} resource pointer is not a location",
                owner_prefix(owner)
            ));
        }
        if let RequestName::Owned(ct, _) = name {
            if !iargs.is_empty() {
                return Err(format!(
                    "{} Owned request passes index arguments",
                    owner_prefix(owner)
                ));
            }
            if let Err(reason) = crate::spec::post::check_owned_request(ct, value_bt) {
                return Err(format!("{}: {reason}", owner_prefix(owner)));
            }
        }
        if let Some(name) = request.predicate_name() {
            let Some(pred) = module.predicate(name) else {
                return Err(format!(
                    "{} takes unknown predicate {}",
                    owner_prefix(owner),
                    display_sym(name)
                ));
            };
            let clauses = validate_predicate_call(iargs, value_bt, pred, owner)?;
            if requested.insert(name.clone()) {
                queue.push_back((pred, clauses));
            }
        }
    }
    Ok(())
}

fn validate_predicate_call<'a>(
    iargs: &[IndexTerm],
    result_bt: &BaseType,
    pred: &'a PredicateDef,
    owner: Option<&PredicateDef>,
) -> Result<&'a [Clause], String> {
    crate::spec::post::check_predicate_call(pred, iargs, result_bt).map_err(|reason| {
        format!(
            "{} call to predicate {}: {reason}",
            owner_prefix(owner),
            display_sym(&pred.name)
        )
    })
}

fn chain_terminal<I>(lat: &Lat<I>) -> &I {
    let mut chain = lat;
    loop {
        chain = match chain {
            Lat::Define(_, rest) | Lat::Resource(_, rest) | Lat::Constraint(_, rest) => rest,
            Lat::I(value) => return value,
        };
    }
}

fn owner_prefix(owner: Option<&PredicateDef>) -> String {
    match owner {
        Some(pred) => format!("ensures predicate {}", display_sym(&pred.name)),
        None => "ensures".into(),
    }
}

fn display_sym(sym: &Sym) -> String {
    sym.name.clone().unwrap_or_else(|| sym.num.to_string())
}

fn generator_bindings(at: &ArgType<()>) -> SymSet {
    let mut bound = SymSet::new();
    let mut node = at;
    let lat = loop {
        match node {
            ArgType::Computational((sym, _), rest) | ArgType::Ghost((sym, _), rest) => {
                bound.insert(sym.clone());
                node = rest;
            }
            ArgType::L(lat) => break lat,
        }
    };
    bound.extend(
        lat.free_vars_bts(&|()| crate::gen_dsl::SymMap::new())
            .into_keys(),
    );
    let mut chain = lat;
    loop {
        chain = match chain {
            Lat::Define((x, _), rest) | Lat::Resource((x, _), rest) => {
                bound.insert(x.clone());
                rest
            }
            Lat::Constraint(_, rest) => rest,
            Lat::I(()) => return bound,
        };
    }
}
