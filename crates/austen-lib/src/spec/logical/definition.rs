use crate::gen_dsl::{BaseType, IndexTerm, Sym};
use serde::{Deserialize, Serialize};

use crate::spec::logical::lat::Lat;

pub type PackingFt = Lat<IndexTerm>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clause {
    pub guard: IndexTerm,
    pub packing_ft: PackingFt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClauseShapeError {
    Empty,
    GuardedLast,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PredicateDef {
    pub name: Sym,
    pub pointer: Sym,
    #[serde(default)]
    pub iargs: Vec<(Sym, BaseType)>,
    pub oarg: BaseType,
    #[serde(default)]
    pub clauses: Option<Vec<Clause>>,
}

impl PredicateDef {
    pub fn generator_iargs(&self) -> Vec<(Sym, BaseType)> {
        std::iter::once((self.pointer.clone(), BaseType::Loc(None)))
            .chain(self.iargs.iter().cloned())
            .collect()
    }

    pub fn callees(&self) -> Vec<Sym> {
        self.clauses
            .iter()
            .flatten()
            .flat_map(|clause| clause.packing_ft.resource_requests())
            .filter_map(|(_, (req, _))| req.predicate_name().cloned())
            .collect()
    }

    pub fn catch_all_clauses(&self) -> Option<Result<&[Clause], ClauseShapeError>> {
        let clauses = self.clauses.as_deref()?;
        let Some(last) = clauses.last() else {
            return Some(Err(ClauseShapeError::Empty));
        };
        if !last.guard.is_true() {
            return Some(Err(ClauseShapeError::GuardedLast));
        }
        Some(Ok(clauses))
    }
}
