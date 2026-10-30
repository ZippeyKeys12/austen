use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::gen_module::GenModule;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::index_term::UnOp;
use crate::gen_dsl::lang::index_term::{Const, IndexTerm, Term};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::subst::Substitution;
use crate::gen_dsl::lang::sym::{Sym, SymGen, SymMap};

pub const MAX_UNFOLD: usize = 16;

pub fn unfold_maps(term: GenTerm, syms: &mut SymGen) -> GenTerm {
    let out: Result<GenTerm, std::convert::Infallible> = term.map_pre(&mut |node| {
        Ok(match node.term {
            GenTermKind::Map {
                var,
                var_bt,
                perm,
                body,
            } if !is_statically_bounded(&perm, &var) => {
                match crate::gen_dsl::choice::translate::map_start(
                    &perm,
                    &var,
                    &SymMap::new(),
                    &GenModule::default(),
                ) {
                    Ok(lo) => unfold(var, var_bt, perm, *body, lo, syms),
                    Err(_) => GenTerm::map((var, var_bt, perm), *body),
                }
            }
            term => GenTerm::from_parts(term, node.bt),
        })
    });
    out.unwrap()
}

fn unfold(
    var: Sym,
    var_bt: BaseType,
    perm: IndexTerm,
    body: GenTerm,
    lo: i128,
    syms: &mut SymGen,
) -> GenTerm {
    let Some(mut keys) = (0..=MAX_UNFOLD as i128)
        .map(|n| {
            lo.checked_add(n)
                .and_then(|k| IndexTerm::num_lit(num_bigint::BigInt::from(k), &var_bt))
        })
        .collect::<Option<Vec<_>>>()
    else {
        return GenTerm::map((var, var_bt, perm), body);
    };
    let limit = keys.pop().expect("the range yields MAX_UNFOLD + 1 keys");
    let start = keys[0].clone();

    let map_bt = BaseType::make_map(var_bt.clone(), body.bt.clone());
    let index = IndexTerm::sym(var.clone(), var_bt.clone());
    let empty = GenTerm::map(
        (
            var.clone(),
            var_bt.clone(),
            IndexTerm::and2(
                IndexTerm::le(start.clone(), index.clone()),
                IndexTerm::lt(index, start),
            ),
        ),
        GenTerm::return_(IndexTerm {
            term: Term::Const(Const::Unknown(body.bt.clone())),
            bt: body.bt.clone(),
        }),
    );

    let mut level = empty.clone();
    for key in keys.into_iter().rev() {
        let sub = Substitution::from([(var.clone(), key.clone())]);
        let elem = syms.fresh("unfolded");
        let acc = syms.fresh("unfolded_map");
        let set = IndexTerm {
            term: Term::MapSet(
                Box::new(IndexTerm::sym(acc.clone(), map_bt.clone())),
                Box::new(key),
                Box::new(IndexTerm::sym(elem.clone(), body.bt.clone())),
            ),
            bt: map_bt.clone(),
        };
        let taken = GenTerm::let_star(
            (elem, body.clone().subst(&sub, syms)),
            GenTerm::let_star((acc, level), GenTerm::return_(set)),
        );
        level = GenTerm::ite(perm.subst(&sub), taken, empty.clone());
    }

    let fits = IndexTerm {
        term: Term::Unop(
            UnOp::Not,
            Box::new(perm.subst(&Substitution::from([(var, limit)]))),
        ),
        bt: BaseType::Bool,
    };
    GenTerm::assert(LogicalConstraint::T(fits), level)
}

fn is_statically_bounded(perm: &IndexTerm, var: &Sym) -> bool {
    crate::gen_dsl::choice::translate::map_domain(perm, var, &SymMap::new(), &GenModule::default())
        .is_ok()
}
