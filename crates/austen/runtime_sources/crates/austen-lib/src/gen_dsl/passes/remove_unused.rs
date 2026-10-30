use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;

fn contains_constraint(gt: &GenTerm) -> bool {
    match &gt.term {
        GenTermKind::Arbitrary | GenTermKind::Return(_) => false,
        GenTermKind::WriteChoice { .. } | GenTermKind::Asgn { .. } | GenTermKind::Assert { .. } => {
            true
        }
        GenTermKind::Call { .. } => true,
        GenTermKind::LetStar { value, rest, .. } => {
            contains_constraint(value) || contains_constraint(rest)
        }
        GenTermKind::Ite { then_, else_, .. } => {
            contains_constraint(then_) || contains_constraint(else_)
        }
        GenTermKind::Map { body, .. } => contains_constraint(body),
        GenTermKind::Scope { body, .. } => contains_constraint(body),
    }
}

pub fn remove_unused(term: GenTerm) -> GenTerm {
    let out: Result<GenTerm, std::convert::Infallible> = term.map_post(&mut |node| {
        Ok(match node.term {
            GenTermKind::LetStar { var, value, rest }
                if !contains_constraint(&value) && !rest.free_vars().contains(&var) =>
            {
                *rest
            }
            GenTermKind::Assert {
                constraint: LogicalConstraint::T(it),
                rest,
            } if it.is_true() => *rest,
            term => GenTerm::from_parts(term, node.bt),
        })
    });
    out.unwrap()
}
