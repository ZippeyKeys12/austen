use serde::{Deserialize, Serialize};

use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::id::Id;
use crate::gen_dsl::lang::sym::Sym;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConstructorDef {
    pub name: Sym,
    #[serde(default)]
    pub params: Vec<(Id, BaseType)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatatypeDef {
    pub name: Sym,
    #[serde(default)]
    pub constructors: Vec<ConstructorDef>,
}

impl DatatypeDef {
    pub fn constructor(&self, name: &Sym) -> Option<&ConstructorDef> {
        self.constructors.iter().find(|c| &c.name == name)
    }
}
