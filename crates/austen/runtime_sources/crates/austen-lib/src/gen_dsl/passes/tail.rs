use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};

use super::TransformError;

pub(in crate::gen_dsl) fn map_tail(
    gt: GenTerm,
    f: &mut impl FnMut(GenTerm) -> Result<GenTerm, TransformError>,
) -> Result<GenTerm, TransformError> {
    let (term, bt) = gt.into_parts();
    Ok(match term {
        term @ (GenTermKind::Arbitrary
        | GenTermKind::Call { .. }
        | GenTermKind::Return(_)
        | GenTermKind::Map { .. }) => f(GenTerm::from_parts(term, bt))?,
        GenTermKind::WriteChoice { label, value, rest } => {
            GenTerm::write_choice(label, value, map_tail(*rest, f)?)
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => GenTerm::asgn(addr, ct, value, map_tail(*rest, f)?),
        GenTermKind::LetStar { var, value, rest } => {
            GenTerm::let_star((var, *value), map_tail(*rest, f)?)
        }
        GenTermKind::Assert { constraint, rest } => {
            GenTerm::assert(constraint, map_tail(*rest, f)?)
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            GenTerm::rebuild_ite(cond, map_tail(*then_, f)?, map_tail(*else_, f)?)
        }
        GenTermKind::Scope { name, body } => GenTerm::scope(name, map_tail(*body, f)?),
    })
}
