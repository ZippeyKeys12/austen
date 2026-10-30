use crate::gen_dsl::{BaseType, IndexTerm, LogicalConstraint, Sym, SymMap};
use serde::{Deserialize, Serialize};

use crate::spec::logical::lat::Lat;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ensures {
    pub var: (Sym, BaseType),
    pub body: Lat<()>,
}

impl Ensures {
    pub fn assert_true(return_var: Sym) -> Self {
        Self {
            var: (return_var, BaseType::Unit),
            body: Lat::Constraint(
                LogicalConstraint::T(IndexTerm::boolean(true)),
                Box::new(Lat::I(())),
            ),
        }
    }

    pub fn is_assert_true(&self) -> bool {
        match &self.body {
            Lat::I(()) => true,
            Lat::Constraint(LogicalConstraint::T(term), rest) => {
                term.is_true() && matches!(**rest, Lat::I(()))
            }
            _ => false,
        }
    }

    pub fn free_vars_bts(&self) -> SymMap<BaseType> {
        let mut vars = self.body.free_vars_bts(&|()| SymMap::new());
        vars.remove(&self.var.0);
        vars
    }
}
