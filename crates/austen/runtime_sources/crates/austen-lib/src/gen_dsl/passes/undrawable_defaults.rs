use crate::gen_dsl::lang::eval::DefaultChoice;
use crate::gen_dsl::lang::gen_def::GenDef;
use crate::gen_dsl::lang::index_term::{Const, IndexTerm, Term};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::passes::partial_evaluation::rewrite_embedded;

pub fn undrawable_defaults_to_unknown(def: GenDef) -> GenDef {
    let GenDef {
        filename,
        recursive,
        spec,
        name,
        iargs,
        oarg,
        c_types,
        body,
        size_arg,
        rec_size_arg,
        heap_base_arg,
        heap_size_arg,
    } = def;
    let body = rewrite_embedded(body, &mut sanitize, &mut |lc| match lc {
        LogicalConstraint::T(it) => LogicalConstraint::T(sanitize(it)),
        LogicalConstraint::Forall {
            var,
            permission,
            body,
        } => LogicalConstraint::Forall {
            var,
            permission: sanitize(permission),
            body: sanitize(body),
        },
    });
    GenDef {
        filename,
        recursive,
        spec,
        name,
        iargs,
        oarg,
        c_types,
        body,
        size_arg,
        rec_size_arg,
        heap_base_arg,
        heap_size_arg,
    }
}

fn sanitize(it: IndexTerm) -> IndexTerm {
    it.map_post(&mut |node| {
        let (term, bt) = node.into_parts();
        match term {
            Term::Const(Const::InputDefault(ty)) if DefaultChoice::width_for(&ty).is_err() => {
                IndexTerm {
                    term: Term::Const(Const::Unknown(ty)),
                    bt,
                }
            }
            term => IndexTerm { term, bt },
        }
    })
}
