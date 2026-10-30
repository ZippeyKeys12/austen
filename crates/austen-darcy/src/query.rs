use austen_lib::gen_dsl::{BaseType, Const, GenModule, Incoming, IndexTerm, Region, SymMap, Term};
use num_bigint::BigInt;
use z3::ast::{Ast, BV, Bool, forall_const};

use crate::path::{DrawVar, SymbolicPath};
use crate::smt::{
    ConstraintSolver, Env, MaxSatEngine, ModelValue, PointerEncoding, Program, SmtError,
    SolveStatus, SolverConfig, Translator,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Model(Vec<ModelValue>),
    Unsat,
    Unknown { reason: Option<String> },
}

pub(crate) fn skew_values(gathered: &SymbolicPath) -> Option<Vec<ModelValue>> {
    gathered.draws.iter().map(skew_value).collect()
}

fn skew_value(draw: &DrawVar) -> Option<ModelValue> {
    match draw.bt {
        BaseType::Bool => Some(ModelValue::Bool(draw.skew != 0)),
        BaseType::Bits(sign, width) if width != 0 => Some(ModelValue::Bits {
            sign,
            width,
            unsigned: austen_lib::gen_dsl::normalise_to_range_big(
                austen_lib::gen_dsl::Sign::Unsigned,
                width,
                &BigInt::from(draw.skew),
            ),
        }),
        BaseType::Loc(_) => Some(ModelValue::Pointer {
            alloc_id: None,
            addr: u64::try_from(draw.skew).ok()?,
        }),
        _ => None,
    }
}

struct SolverDraws {
    wanted: SymMap<BaseType>,
    retained: Vec<Option<ModelValue>>,
}

fn solver_draws(gathered: &SymbolicPath) -> SolverDraws {
    let mut wanted = SymMap::new();
    for variables in gathered
        .constraints
        .iter()
        .map(|c| c.free_vars_bts())
        .chain(gathered.pins.iter().map(|p| p.free_vars_bts()))
        .chain(gathered.resources.iter().flat_map(|r| {
            [
                r.active.free_vars_bts(),
                r.first_addr.free_vars_bts(),
                r.last_addr.free_vars_bts(),
            ]
        }))
    {
        wanted.extend(variables);
    }
    let mut retained = Vec::with_capacity(gathered.draws.len());
    for draw in &gathered.draws {
        let sample = (!wanted.contains_key(&draw.sym))
            .then(|| skew_value(draw))
            .flatten();
        if sample.is_none() {
            wanted.insert(draw.sym.clone(), draw.bt.clone());
        }
        retained.push(sample);
    }
    SolverDraws { wanted, retained }
}

pub fn solve(
    gathered: &SymbolicPath,
    gen_module: &GenModule,
    bounds: Region,
    config: &SolverConfig,
) -> Result<Outcome, SmtError> {
    let incoming = Incoming::from(bounds);
    solve_with_site(gathered, gen_module, &incoming, config, &mut None)
}

pub(crate) fn solve_with_site(
    gathered: &SymbolicPath,
    gen_module: &GenModule,
    incoming: &Incoming,
    config: &SolverConfig,
    site: &mut Option<austen_lib::gen_dsl::SiteId>,
) -> Result<Outcome, SmtError> {
    solve_with_engine(
        gathered,
        gen_module,
        incoming,
        config,
        MaxSatEngine::MaxRes,
        site,
    )
}

pub(crate) fn solve_with_engine(
    gathered: &SymbolicPath,
    gen_module: &GenModule,
    incoming: &Incoming,
    config: &SolverConfig,
    engine: MaxSatEngine,
    site: &mut Option<austen_lib::gen_dsl::SiteId>,
) -> Result<Outcome, SmtError> {
    config.scope(|| solve_in_context(gathered, gen_module, incoming, config, engine, site))
}

fn solve_in_context(
    gathered: &SymbolicPath,
    gen_module: &GenModule,
    incoming: &Incoming,
    config: &SolverConfig,
    engine: MaxSatEngine,
    site: &mut Option<austen_lib::gen_dsl::SiteId>,
) -> Result<Outcome, SmtError> {
    let mut tr = Translator::with_pointers(
        Program::new(&gen_module.structs)
            .and_functions(&gen_module.functions)
            .and_datatypes(&gen_module.datatypes),
        PointerEncoding::Address,
    );
    for ty in gathered.default_types(gen_module) {
        tr.input_default(&ty)?;
    }

    let SolverDraws { wanted, retained } = solver_draws(gathered);
    let mut vars = SymMap::new();
    for (sym, bt) in &wanted {
        let declared = tr.declare(sym, bt).inspect_err(|_| {
            *site = gathered
                .draws
                .iter()
                .position(|draw| &draw.sym == sym)
                .and_then(|i| gathered.sites.draws.get(i))
                .cloned()
                .flatten()
                .or_else(|| {
                    gathered
                        .constraints
                        .iter()
                        .position(|constraint| constraint.free_vars_bts().contains_key(sym))
                        .and_then(|i| gathered.sites.constraints.get(i))
                        .cloned()
                        .flatten()
                })
                .or_else(|| {
                    gathered
                        .pins
                        .iter()
                        .position(|pin| pin.free_vars_bts().contains_key(sym))
                        .and_then(|i| gathered.sites.pins.get(i))
                        .cloned()
                        .flatten()
                })
                .or_else(|| {
                    gathered
                        .resources
                        .iter()
                        .position(|resource| {
                            [&resource.active, &resource.first_addr, &resource.last_addr]
                                .iter()
                                .any(|term| term.free_vars_bts().contains_key(sym))
                        })
                        .and_then(|i| gathered.sites.resources.get(i))
                        .cloned()
                        .flatten()
                });
        })?;
        vars.insert(sym.clone(), declared);
    }
    let env = Env::Root(&vars);

    let solver = ConstraintSolver::with_skews_and_engine(config, engine);

    for (i, c) in gathered.constraints.iter().enumerate() {
        *site = gathered.sites.constraints.get(i).cloned().flatten();
        let f = tr.constraint(&env, c)?;
        assert_hard(&tr, &solver, &f);
    }
    for (i, p) in gathered.pins.iter().enumerate() {
        *site = gathered.sites.pins.get(i).cloned().flatten();
        let f = tr.boolean(&env, p)?;
        assert_hard(&tr, &solver, &f);
    }
    assert_resources(&mut tr, &env, &solver, gathered, incoming, site)?;

    for (i, v) in solver_owned(gathered, &retained) {
        *site = gathered.sites.draws.get(i).cloned().flatten();
        let lit =
            literal_at(&v.bt, v.skew).ok_or_else(|| SmtError::UnsupportedBaseType(v.bt.clone()))?;
        let want = IndexTerm::eq(IndexTerm::sym(v.sym.clone(), v.bt.clone()), lit);
        let f = tr.boolean(&env, &want)?;
        solver.assert_soft(&f);
    }

    let (status, model) = solver.check_with_model();
    match status {
        SolveStatus::Unsat => Ok(Outcome::Unsat),
        SolveStatus::Unknown { reason } => Ok(Outcome::Unknown { reason }),
        SolveStatus::Sat => {
            let model = model.expect("a Sat check has a model");
            let mut out = Vec::with_capacity(gathered.draws.len());
            for (i, (draw, sample)) in gathered.draws.iter().zip(retained).enumerate() {
                *site = gathered.sites.draws.get(i).cloned().flatten();
                let value = match sample {
                    Some(sample) => sample,
                    None => {
                        let expr =
                            tr.term(&env, &IndexTerm::sym(draw.sym.clone(), draw.bt.clone()))?;
                        tr.decode(&model, &expr)?
                    }
                };
                out.push(value);
            }
            Ok(Outcome::Model(out))
        }
    }
}

fn assert_hard(tr: &Translator<'_>, solver: &ConstraintSolver, fact: &Bool) {
    let defaults = tr.input_default_variables();
    if defaults.is_empty() {
        solver.assert(fact);
    } else {
        let variables: Vec<&dyn Ast> = defaults.iter().map(|value| value as &dyn Ast).collect();
        solver.assert(&forall_const(&variables, &[], fact));
    }
}

fn assert_resources(
    tr: &mut Translator<'_>,
    env: &Env<'_>,
    solver: &ConstraintSolver,
    gathered: &SymbolicPath,
    incoming: &Incoming,
    site: &mut Option<austen_lib::gen_dsl::SiteId>,
) -> Result<(), SmtError> {
    let claims = &gathered.resources;
    if claims.is_empty() {
        return Ok(());
    }
    let bv = |n: u64| BV::from_u64(n, 64);
    let zero = bv(0);

    struct Interval {
        active: Bool,
        first: BV,
        end: BV,
        nonempty: Bool,
    }

    let mut intervals = Vec::with_capacity(claims.len());
    for (i, claim) in claims.iter().enumerate() {
        *site = gathered.sites.resources.get(i).cloned().flatten();
        let active = tr.boolean(env, &claim.active)?;
        let first = tr.term(env, &claim.first_addr)?;
        let first = tr.as_address(&first)?;
        let last = tr.term(env, &claim.last_addr)?;
        let last = tr.as_address(&last)?;

        let address_valid = Bool::and(&[&first.eq(&zero).not(), &first.bvule(&last)]);
        let (valid, nonempty) = if claim.tail_bytes == 0 {
            (address_valid, Bool::from_bool(false))
        } else {
            let contained = incoming
                .write_regions()
                .map(|(_, region)| region)
                .filter(|bounds| claim.tail_bytes <= bounds.size)
                .map(|bounds| {
                    Bool::and(&[
                        &first.bvuge(bv(bounds.base)),
                        &last.bvule(bv(bounds.base + bounds.size - claim.tail_bytes)),
                    ])
                })
                .collect::<Vec<_>>();
            let contained = if contained.is_empty() {
                Bool::from_bool(false)
            } else {
                Bool::or(&contained)
            };
            (
                Bool::and(&[&address_valid, &contained]),
                Bool::from_bool(true),
            )
        };
        assert_hard(tr, solver, &active.implies(&valid));

        let end = last.bvadd(bv(claim.tail_bytes));
        intervals.push(Interval {
            active,
            first,
            end,
            nonempty,
        });
    }

    for i in 0..intervals.len() {
        for j in (i + 1)..intervals.len() {
            let a = &intervals[i];
            let b = &intervals[j];
            let both = Bool::and(&[&a.active, &a.nonempty, &b.active, &b.nonempty]);
            let apart = Bool::or(&[&a.end.bvule(&b.first), &b.end.bvule(&a.first)]);
            assert_hard(tr, solver, &both.implies(&apart));
        }
    }
    Ok(())
}

fn solver_owned<'a>(
    gathered: &'a SymbolicPath,
    retained: &'a [Option<ModelValue>],
) -> impl Iterator<Item = (usize, &'a DrawVar)> {
    gathered
        .draws
        .iter()
        .zip(retained)
        .enumerate()
        .filter_map(|(i, (draw, sample))| sample.is_none().then_some((i, draw)))
}

fn literal_at(bt: &BaseType, unsigned: i128) -> Option<IndexTerm> {
    match bt {
        BaseType::Bool => Some(IndexTerm::boolean(unsigned != 0)),
        BaseType::Bits(sign, width) => IndexTerm::num_lit(
            austen_lib::gen_dsl::normalise_to_range_big(*sign, *width, &BigInt::from(unsigned)),
            bt,
        ),
        BaseType::Loc(_) => Some(IndexTerm {
            term: Term::Const(Const::Pointer {
                alloc_id: BigInt::ZERO,
                addr: BigInt::from(unsigned),
            }),
            bt: bt.clone(),
        }),
        _ => None,
    }
}
