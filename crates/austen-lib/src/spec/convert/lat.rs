use crate::gen_dsl::{BaseType, GenTerm, IndexTerm, SymMap, SymSet};
use crate::spec::{Lat, Request, RequestName};

use crate::spec::convert::{ConvertError, Cx};

fn transform_vars(
    generated: &SymSet,
    lat: &Lat<IndexTerm>,
) -> (SymSet, Vec<(crate::gen_dsl::Sym, BaseType)>) {
    let (bound, mut mentioned): (SymSet, SymMap<BaseType>) = match lat {
        Lat::Define((x, it), _) => (SymSet::from([x.clone()]), it.free_vars_bts()),
        Lat::Resource((x, (req, bt)), _) => {
            let mut vars = req.free_vars_bts();
            if matches!(
                req,
                Request::P {
                    name: RequestName::Owned(_, _),
                    ..
                }
            ) {
                vars.insert(x.clone(), bt.clone());
            }
            (SymSet::from([x.clone()]), vars)
        }
        Lat::Constraint(lc, _) => (SymSet::new(), lc.free_vars_bts()),
        Lat::I(it) => (SymSet::new(), it.free_vars_bts()),
    };
    mentioned.retain(|sym, _| !generated.contains(sym));
    let draws: Vec<_> = mentioned.into_iter().collect();
    let mut generated = generated.clone();
    generated.extend(draws.iter().map(|(sym, _)| sym.clone()));
    generated.extend(bound);
    (generated, draws)
}

fn map_bt(bt: &BaseType) -> Result<(BaseType, BaseType), ConvertError> {
    match bt {
        BaseType::Map(key, value) => Ok(((**key).clone(), (**value).clone())),
        other => Err(ConvertError::NotAMapBaseType { bt: other.clone() }),
    }
}

fn predicate_global_actuals(cx: &Cx<'_>, name: &crate::gen_dsl::Sym) -> Vec<IndexTerm> {
    cx.predicate_global_iargs(name)
        .into_iter()
        .map(|(sym, bt)| IndexTerm::sym(sym, bt))
        .collect()
}

pub fn transform_it_lat(
    cx: &mut Cx<'_>,
    generated: &SymSet,
    lat: &Lat<IndexTerm>,
) -> Result<GenTerm, ConvertError> {
    let (generated, draws) = transform_vars(generated, lat);
    let body = match lat {
        Lat::Define((x, it), rest) => {
            let rest = transform_it_lat(cx, &generated, rest)?;
            GenTerm::let_star((x.clone(), GenTerm::return_(it.clone())), rest)
        }

        Lat::Resource(
            (
                x,
                (
                    Request::P {
                        name: RequestName::Owned(ct, _),
                        pointer,
                        ..
                    },
                    bt,
                ),
            ),
            rest,
        ) => {
            let rest = transform_it_lat(cx, &generated, rest)?;
            GenTerm::asgn(
                pointer.clone(),
                ct.clone(),
                IndexTerm::sym(x.clone(), bt.clone()),
                rest,
            )
        }

        Lat::Resource(
            (
                x,
                (
                    Request::P {
                        name: RequestName::PName(fsym),
                        pointer,
                        iargs,
                    },
                    bt,
                ),
            ),
            rest,
        ) => {
            let rest = transform_it_lat(cx, &generated, rest)?;
            cx.request_predicate(fsym)?;
            let globals = predicate_global_actuals(cx, fsym);
            let args = std::iter::once(pointer.clone())
                .chain(iargs.iter().cloned())
                .chain(globals)
                .collect();
            GenTerm::let_star(
                (x.clone(), GenTerm::call(fsym.clone(), args, bt.clone())),
                rest,
            )
        }

        Lat::Resource(
            (
                x,
                (
                    Request::Q {
                        name: RequestName::Owned(ct, _),
                        pointer,
                        q,
                        step,
                        permission,
                        ..
                    },
                    bt,
                ),
            ),
            rest,
        ) => {
            let rest = transform_it_lat(cx, &generated, rest)?;
            let (key_bt, value_bt) = map_bt(bt)?;
            let drawn = cx.syms.fresh_anon();
            let value = IndexTerm::sym(drawn.clone(), value_bt.clone());
            let addr = IndexTerm::array_shift(
                pointer.clone(),
                step.clone(),
                IndexTerm::sym(q.0.clone(), key_bt.clone()),
            );
            let iteration = GenTerm::let_star(
                (drawn, GenTerm::arbitrary(value_bt)),
                GenTerm::asgn(addr, ct.clone(), value.clone(), GenTerm::return_(value)),
            );
            let gt_map = GenTerm::map((q.0.clone(), key_bt, permission.clone()), iteration);
            GenTerm::let_star((x.clone(), gt_map), rest)
        }

        Lat::Resource(
            (
                x,
                (
                    Request::Q {
                        name: RequestName::PName(fsym),
                        pointer,
                        q,
                        step,
                        permission,
                        iargs,
                    },
                    bt,
                ),
            ),
            rest,
        ) => {
            let rest = transform_it_lat(cx, &generated, rest)?;
            cx.request_predicate(fsym)?;
            let (_, value_bt) = map_bt(bt)?;
            let addr = IndexTerm::array_shift(
                pointer.clone(),
                step.clone(),
                IndexTerm::sym(q.0.clone(), q.1.clone()),
            );
            let globals = predicate_global_actuals(cx, fsym);
            let args = std::iter::once(addr)
                .chain(iargs.iter().cloned())
                .chain(globals)
                .collect();
            let taken = cx.syms.fresh_anon();
            let iteration = GenTerm::let_star(
                (
                    taken.clone(),
                    GenTerm::call(fsym.clone(), args, value_bt.clone()),
                ),
                GenTerm::return_(IndexTerm::sym(taken, value_bt)),
            );
            let gt_map = GenTerm::map((q.0.clone(), q.1.clone(), permission.clone()), iteration);
            GenTerm::let_star((x.clone(), gt_map), rest)
        }

        Lat::Constraint(lc, rest) => {
            GenTerm::assert(lc.clone(), transform_it_lat(cx, &generated, rest)?)
        }

        Lat::I(it) => GenTerm::return_(it.clone()),
    };

    Ok(draws.into_iter().rev().fold(body, |rest, (x, bt)| {
        GenTerm::let_star((x, GenTerm::arbitrary(bt)), rest)
    }))
}
