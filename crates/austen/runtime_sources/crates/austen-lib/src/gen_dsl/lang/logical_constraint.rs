use serde::{Deserialize, Serialize};

use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::index_term::IndexTerm;
use crate::gen_dsl::lang::sym::Sym;

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LogicalConstraint {
    T(IndexTerm),
    Forall {
        var: (Sym, BaseType),
        permission: IndexTerm,
        body: IndexTerm,
    },
}
