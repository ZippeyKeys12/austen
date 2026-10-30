use crate::gen_dsl::{IntegerBaseType, IntegerType, Qualifiers, Sctype, Sym};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub use crate::gen_dsl::GlobalLinkage;

use crate::spec::logical::{ArgType, Ensures};

pub const INTERNAL_GLOBAL_PREFIX: &str = "__austen_global_";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FunctionLinkage {
    #[default]
    External,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CType {
    Int { bits: u8, signed: bool },
    Bool,
    Void,
    Pointer { pointee: Box<CType> },
    Array { element: Box<CType>, length: usize },
    Named { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeDef {
    pub name: String,
    #[serde(default)]
    pub tag: Option<Sym>,
    #[serde(default)]
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: CType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalSpec {
    pub address: Sym,
    pub c_name: String,
    #[serde(rename = "type")]
    pub ty: Sctype,
    pub linkage: GlobalLinkage,
    #[serde(default)]
    pub qualifiers: Qualifiers,
}

impl GlobalSpec {
    pub fn link_name(&self) -> String {
        match &self.linkage {
            GlobalLinkage::External => self.c_name.clone(),
            GlobalLinkage::Internal { .. } => {
                format!("{INTERNAL_GLOBAL_PREFIX}{}", self.address.num)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum GeneratorSource {
    File(PathBuf),
    Entry {
        file: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entry: Option<String>,
    },
}

impl GeneratorSource {
    pub fn file(&self) -> &Path {
        match self {
            Self::File(file) | Self::Entry { file, .. } => file,
        }
    }

    pub fn entry_or<'a>(&'a self, function: &'a str) -> &'a str {
        match self {
            Self::Entry {
                entry: Some(entry), ..
            } => entry,
            _ => function,
        }
    }

    pub fn set_file(&mut self, path: PathBuf) {
        match self {
            Self::File(file) | Self::Entry { file, .. } => *file = path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator: Option<GeneratorSource>,
    #[serde(default)]
    pub linkage: FunctionLinkage,
    #[serde(default)]
    pub params: Vec<Field>,
    #[serde(rename = "return", default = "CType::void")]
    pub ret: CType,
    #[serde(default)]
    pub internal: Option<ArgType<()>>,
    pub ensures: Ensures,
}

impl CType {
    pub(in crate::spec) fn void() -> Self {
        CType::Void
    }

    pub fn to_sctype(&self, tags: &dyn Fn(&str) -> Option<Sym>) -> Option<Sctype> {
        Some(match self {
            CType::Int { bits, signed } => {
                let base = match bits {
                    8 => IntegerBaseType::Ichar,
                    16 => IntegerBaseType::Short,
                    32 => IntegerBaseType::Int,
                    64 => IntegerBaseType::Long,
                    _ => return None,
                };
                Sctype::Integer(if *signed {
                    IntegerType::Signed(base)
                } else {
                    IntegerType::Unsigned(base)
                })
            }
            CType::Bool => Sctype::Integer(IntegerType::Bool),
            CType::Void => Sctype::Void,
            CType::Pointer { pointee } => Sctype::Pointer(Box::new(pointee.to_sctype(tags)?)),
            CType::Array { element, length } => {
                Sctype::Array(Box::new(element.to_sctype(tags)?), Some(*length))
            }
            CType::Named { name } => Sctype::Struct(tags(name)?),
        })
    }
}
