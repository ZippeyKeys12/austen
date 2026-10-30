use num_bigint::BigInt;

use crate::gen_dsl::lang::gen_def::GenDef;
use crate::gen_dsl::lang::gen_module::{call_graph_of, transitive_closure};
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::index_term::{BinOp, IndexTerm, Term};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::make_term::uintptr_bt;
use crate::gen_dsl::lang::sym::{Sym, SymGen, SymMap, SymSet};

struct Sig {
    size: Sym,
    rec: Option<Sym>,
}

pub fn size_generators(defs: &mut [(Sym, GenDef)], syms: &mut SymGen) {
    let reach = transitive_closure(&call_graph_of(defs));
    let on_cycle = |name: &Sym| is_recursive_edge(&reach, name, name);

    let sigs: SymMap<Sig> = defs
        .iter()
        .map(|(name, def)| {
            debug_assert!(
                def.size_arg.is_none() && def.rec_size_arg.is_none(),
                "size_generators is one-shot; {name:?} is already sized"
            );
            let sig = Sig {
                size: syms.fresh("size"),
                rec: on_cycle(name).then(|| syms.fresh("rec_size")),
            };
            (name.clone(), sig)
        })
        .collect();

    for (name, def) in defs.iter_mut() {
        let own = &sigs[name];
        let body = std::mem::replace(&mut def.body, GenTerm::return_(IndexTerm::unit()));
        let body = transform_gt(name, own, &sigs, &reach, body);
        let governing = own.rec.as_ref().unwrap_or(&own.size);
        def.body = GenTerm::assert(
            LogicalConstraint::T(IndexTerm::lt(size_lit(0), size_var(governing))),
            body,
        );
        def.iargs.push((own.size.clone(), uintptr_bt()));
        if let Some(rec) = &own.rec {
            def.iargs.push((rec.clone(), uintptr_bt()));
        }
        def.size_arg = Some(own.size.clone());
        def.rec_size_arg = own.rec.clone();
    }
}

fn is_recursive_edge(reach: &SymMap<SymSet>, home: &Sym, callee: &Sym) -> bool {
    reach.get(callee).is_some_and(|r| r.contains(home))
}

fn transform_gt(
    home: &Sym,
    own: &Sig,
    sigs: &SymMap<Sig>,
    reach: &SymMap<SymSet>,
    gt: GenTerm,
) -> GenTerm {
    let (term, bt) = gt.into_parts();
    let term = match term {
        GenTermKind::Ite { cond, then_, else_ } => GenTermKind::Ite {
            cond,
            then_: Box::new(transform_gt(home, own, sigs, reach, *then_)),
            else_: Box::new(transform_gt(home, own, sigs, reach, *else_)),
        },
        GenTermKind::Scope { name, body } => GenTermKind::Scope {
            name,
            body: Box::new(transform_gt(home, own, sigs, reach, *body)),
        },
        term => {
            let gt = GenTerm::from_parts(term, bt);
            let n = count_recursive_calls(home, reach, &gt);
            return size_calls(home, own, sigs, reach, n, gt);
        }
    };
    GenTerm::from_parts(term, bt)
}

fn count_recursive_calls(home: &Sym, reach: &SymMap<SymSet>, gt: &GenTerm) -> usize {
    match &gt.term {
        GenTermKind::Arbitrary | GenTermKind::Return(_) => 0,
        GenTermKind::Call { fsym, .. } => usize::from(is_recursive_edge(reach, home, fsym)),
        GenTermKind::Asgn { rest, .. }
        | GenTermKind::Assert { rest, .. }
        | GenTermKind::WriteChoice { rest, .. }
        | GenTermKind::Map { body: rest, .. }
        | GenTermKind::Scope { body: rest, .. } => count_recursive_calls(home, reach, rest),
        GenTermKind::LetStar { value, rest, .. } => {
            count_recursive_calls(home, reach, value) + count_recursive_calls(home, reach, rest)
        }
        GenTermKind::Ite { then_, else_, .. } => {
            count_recursive_calls(home, reach, then_).max(count_recursive_calls(home, reach, else_))
        }
    }
}

fn size_calls(
    home: &Sym,
    own: &Sig,
    sigs: &SymMap<Sig>,
    reach: &SymMap<SymSet>,
    n: usize,
    gt: GenTerm,
) -> GenTerm {
    let out: Result<GenTerm, std::convert::Infallible> = gt.map_pre(&mut |node| {
        let (term, bt) = node.into_parts();
        let GenTermKind::Call { fsym, mut args } = term else {
            return Ok(GenTerm::from_parts(term, bt));
        };
        if let Some(callee) = sigs.get(&fsym) {
            args.push(size_var(&own.size));
            if callee.rec.is_some() {
                if is_recursive_edge(reach, home, &fsym) {
                    let rec = own.rec.as_ref().expect(
                        "a recursive edge out of `home` puts `home` on a cycle, so it has a local size",
                    );
                    debug_assert!(n >= 1, "the count counted exactly these calls");
                    args.push(decremented(rec, n));
                } else {
                    args.push(size_var(&own.size));
                }
            }
        }
        Ok(GenTerm::from_parts(GenTermKind::Call { fsym, args }, bt))
    });
    let Ok(gt) = out;
    gt
}

fn size_var(sym: &Sym) -> IndexTerm {
    IndexTerm::sym(sym.clone(), uintptr_bt())
}

fn size_lit(k: u64) -> IndexTerm {
    IndexTerm::num_lit(BigInt::from(k), &uintptr_bt()).expect("a u64 fits its own base type")
}

fn decremented(rec: &Sym, n: usize) -> IndexTerm {
    let (op, k) = if n == 1 {
        (BinOp::Sub, 1)
    } else {
        (BinOp::Div, n as u64)
    };
    IndexTerm {
        term: Term::Binop(op, Box::new(size_var(rec)), Box::new(size_lit(k))),
        bt: uintptr_bt(),
    }
}
