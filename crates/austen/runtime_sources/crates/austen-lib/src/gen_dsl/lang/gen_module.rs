use serde::{Deserialize, Serialize};

use crate::gen_dsl::lang::datatype::{ConstructorDef, DatatypeDef};
use crate::gen_dsl::lang::gen_def::GenDef;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::logical_function::LogicalFunctionDef;
use crate::gen_dsl::lang::sctype::{Qualifiers, Sctype};
use crate::gen_dsl::lang::struct_def::StructDef;
use crate::gen_dsl::lang::sym::{Sym, SymMap, SymSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GlobalLinkage {
    External,
    Internal { owner: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalDef {
    pub c_name: String,
    #[serde(rename = "type")]
    pub ty: Sctype,
    pub linkage: GlobalLinkage,
    #[serde(default)]
    pub qualifiers: Qualifiers,
}

#[derive(Debug, Clone, Copy)]
pub struct GenDeclarations<'a> {
    structs: &'a [(Sym, StructDef)],
    functions: &'a [(Sym, LogicalFunctionDef)],
    datatypes: &'a [(Sym, DatatypeDef)],
    globals: &'a [(Sym, GlobalDef)],
}

impl GenDeclarations<'static> {
    pub const fn empty() -> Self {
        Self {
            structs: &[],
            functions: &[],
            datatypes: &[],
            globals: &[],
        }
    }
}

impl<'a> GenDeclarations<'a> {
    pub fn structs(self) -> &'a [(Sym, StructDef)] {
        self.structs
    }
    pub fn functions(self) -> &'a [(Sym, LogicalFunctionDef)] {
        self.functions
    }
    pub fn datatypes(self) -> &'a [(Sym, DatatypeDef)] {
        self.datatypes
    }
    pub fn globals(self) -> &'a [(Sym, GlobalDef)] {
        self.globals
    }

    pub fn find_struct(self, tag: &Sym) -> Option<&'a StructDef> {
        self.structs.iter().find(|(t, _)| t == tag).map(|(_, d)| d)
    }

    pub fn find_function(self, name: &Sym) -> Option<&'a LogicalFunctionDef> {
        self.functions
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, f)| f)
    }

    pub fn find_constructor(self, name: &Sym) -> Option<&'a ConstructorDef> {
        self.datatypes.iter().find_map(|(_, d)| d.constructor(name))
    }

    pub fn find_global(self, name: &Sym) -> Option<&'a GlobalDef> {
        self.globals
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, global)| global)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "UncheckedGenModule")]
pub struct GenModule {
    pub defs: Vec<(Sym, GenDef)>,
    pub structs: Vec<(Sym, StructDef)>,
    pub functions: Vec<(Sym, LogicalFunctionDef)>,
    pub datatypes: Vec<(Sym, DatatypeDef)>,
    pub globals: Vec<(Sym, GlobalDef)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedGenModule {
    defs: Vec<(Sym, GenDef)>,
    structs: Vec<(Sym, StructDef)>,
    functions: Vec<(Sym, LogicalFunctionDef)>,
    datatypes: Vec<(Sym, DatatypeDef)>,
    globals: Vec<(Sym, GlobalDef)>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GenModuleError {
    #[error("duplicate generator definition {name:?}")]
    DuplicateGenerator { name: Sym },
    #[error("generator table key {key:?} disagrees with definition name {definition:?}")]
    MismatchedGeneratorName { key: Sym, definition: Sym },
    #[error("unknown generator {name:?}")]
    UnknownGenerator { name: Sym },
    #[error("generator {name:?} is not a function generator")]
    NotFunctionGenerator { name: Sym },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GenCallError {
    #[error("unknown generator {name:?}")]
    UnknownGenerator { name: Sym },
    #[error("generator {name:?} expects {expected} arguments, but the call supplies {actual}")]
    Arity {
        name: Sym,
        expected: usize,
        actual: usize,
    },
}

impl TryFrom<UncheckedGenModule> for GenModule {
    type Error = GenModuleError;

    fn try_from(module: UncheckedGenModule) -> Result<Self, Self::Error> {
        let module = Self {
            defs: module.defs,
            structs: module.structs,
            functions: module.functions,
            datatypes: module.datatypes,
            globals: module.globals,
        };
        module.validate()?;
        Ok(module)
    }
}

impl GenModule {
    pub fn declarations(&self) -> GenDeclarations<'_> {
        GenDeclarations {
            structs: &self.structs,
            functions: &self.functions,
            datatypes: &self.datatypes,
            globals: &self.globals,
        }
    }

    pub fn of_defs(defs: Vec<(Sym, GenDef)>) -> Self {
        GenModule {
            defs,
            structs: Vec::new(),
            functions: Vec::new(),
            datatypes: Vec::new(),
            globals: Vec::new(),
        }
    }

    pub fn generator(&self, name: &Sym) -> Option<&GenDef> {
        self.defs.iter().find(|(n, _)| n == name).map(|(_, d)| d)
    }

    pub fn wire_root(&self, root: &GenTerm) -> GenTerm {
        let GenTermKind::Call { fsym, args } = &root.term else {
            return root.clone();
        };
        let Some(def) = self.generator(fsym) else {
            return root.clone();
        };
        if def.heap_args().next().is_none() || args.len() != def.iargs.len() {
            return root.clone();
        }
        let kept = def
            .iargs
            .iter()
            .zip(args)
            .filter(|((param, _), _)| !def.heap_args().any(|heap| heap == param))
            .map(|(_, arg)| arg.clone())
            .collect();
        GenTerm::call(fsym.clone(), kept, root.bt.clone())
    }

    pub fn function_generators(&self) -> impl Iterator<Item = (&Sym, &GenDef)> {
        self.defs
            .iter()
            .filter_map(|(name, def)| def.spec.then_some((name, def)))
    }

    pub fn function_generator(&self, name: &Sym) -> Result<&GenDef, GenModuleError> {
        let (_, def) = self
            .defs
            .iter()
            .find(|(candidate, _)| candidate == name)
            .ok_or_else(|| GenModuleError::UnknownGenerator { name: name.clone() })?;
        if !def.spec {
            return Err(GenModuleError::NotFunctionGenerator { name: name.clone() });
        }
        Ok(def)
    }

    pub fn resolve_call(&self, name: &Sym, actual: usize) -> Result<&GenDef, GenCallError> {
        let def = self
            .generator(name)
            .ok_or_else(|| GenCallError::UnknownGenerator { name: name.clone() })?;
        if def.iargs.len() != actual {
            return Err(GenCallError::Arity {
                name: name.clone(),
                expected: def.iargs.len(),
                actual,
            });
        }
        Ok(def)
    }

    pub fn validate(&self) -> Result<(), GenModuleError> {
        let mut seen = SymSet::new();
        for (key, def) in &self.defs {
            if key.num != def.name.num || key.name != def.name.name {
                return Err(GenModuleError::MismatchedGeneratorName {
                    key: key.clone(),
                    definition: def.name.clone(),
                });
            }
            if !seen.insert(key.clone()) {
                return Err(GenModuleError::DuplicateGenerator { name: key.clone() });
            }
        }
        Ok(())
    }

    pub fn find_struct(&self, tag: &Sym) -> Option<&StructDef> {
        self.declarations().find_struct(tag)
    }

    pub fn find_function(&self, name: &Sym) -> Option<&LogicalFunctionDef> {
        self.declarations().find_function(name)
    }

    pub fn find_constructor(&self, name: &Sym) -> Option<&ConstructorDef> {
        self.declarations().find_constructor(name)
    }

    pub fn find_global(&self, name: &Sym) -> Option<&GlobalDef> {
        self.declarations().find_global(name)
    }

    pub fn call_graph(&self) -> SymMap<SymSet> {
        call_graph_of(&self.defs)
    }
}

pub(in crate::gen_dsl) fn call_graph_of(defs: &[(Sym, GenDef)]) -> SymMap<SymSet> {
    defs.iter()
        .map(|(name, def)| (name.clone(), calls_of(&def.body)))
        .collect()
}

pub(in crate::gen_dsl) fn transitive_closure(graph: &SymMap<SymSet>) -> SymMap<SymSet> {
    graph
        .iter()
        .map(|(v, direct)| {
            let mut seen = SymSet::new();
            let mut stack: Vec<Sym> = direct.iter().cloned().collect();
            while let Some(s) = stack.pop() {
                if seen.insert(s.clone())
                    && let Some(next) = graph.get(&s)
                {
                    stack.extend(next.iter().cloned());
                }
            }
            (v.clone(), seen)
        })
        .collect()
}

fn calls_of(gt: &GenTerm) -> SymSet {
    let mut out = SymSet::new();
    collect_calls(gt, &mut out);
    out
}

fn collect_calls(gt: &GenTerm, out: &mut SymSet) {
    match &gt.term {
        GenTermKind::Arbitrary | GenTermKind::Return(_) => {}
        GenTermKind::Call { fsym, .. } => {
            out.insert(fsym.clone());
        }
        GenTermKind::Asgn { rest, .. }
        | GenTermKind::WriteChoice { rest, .. }
        | GenTermKind::Assert { rest, .. }
        | GenTermKind::Map { body: rest, .. }
        | GenTermKind::Scope { body: rest, .. } => collect_calls(rest, out),
        GenTermKind::LetStar { value, rest, .. } => {
            collect_calls(value, out);
            collect_calls(rest, out);
        }
        GenTermKind::Ite { then_, else_, .. } => {
            collect_calls(then_, out);
            collect_calls(else_, out);
        }
    }
}
