use serde::{Deserialize, Serialize};

pub mod c;
pub mod convert;
pub mod eval;
pub mod logical;
pub mod post;
pub mod sanity;

pub use c::{
    CType, Field, FunctionLinkage, FunctionSpec, GeneratorSource, GlobalLinkage, GlobalSpec,
    INTERNAL_GLOBAL_PREFIX, TypeDef,
};
pub use convert::{
    Conversion, ConvertError, ConvertedFunction, MAX_FUNCTIONS, OBSERVED_STAGES, StagedConversion,
    compile_production, global_context, global_size, transform, transform_observed,
    transform_observed_with_options, transform_staged_observed_with_options,
    transform_with_options, validate_function_table, validate_global_table,
};
pub use logical::{
    ArgType, Clause, ClauseShapeError, ConstructorDef, DatatypeDef, Ensures, FunctionBody, Init,
    Lat, LogicalFunctionDef, PackingFt, PredicateDef, Request, RequestName, ResourceBinding,
};
pub use post::{
    OracleSupportDefect, Post, check_owned_request, check_predicate_call,
    check_quantified_permission, check_quantified_request,
};
pub use sanity::{
    AnalysisLimitation, BranchCoverage, DiagnosticCode, Evidence, FunctionSummary, IrLocation,
    LimitationCode, RelationKind, RelationSummary, SanityDiagnostic, SanityOptions, SanityPhase,
    SanityReport, Severity, Subject, SubjectKind, SubjectSummary, analyze_module,
};

pub use crate::gen_dsl::choice::translate::MAX_MAP_ITERATIONS;
pub use crate::gen_dsl::{
    BaseType, BinOp, Bindings, Const, DefaultChoice, DefaultChoices, DefaultTypes,
    DefaultTypesError, Id, IndexTerm, IntegerBaseType, IntegerType, LogicalConstraint, Pattern,
    PatternKind, PointerOrigin, Scalar, Sctype, Sign, Sym, SymMap, SymbolicMapBounds, Term, UnOp,
    Value, global_layout,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecModule {
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub next_sym: Option<u64>,
    #[serde(default)]
    pub types: Vec<TypeDef>,
    #[serde(default)]
    pub globals: Vec<GlobalSpec>,
    #[serde(default)]
    pub predicates: Vec<PredicateDef>,
    #[serde(default)]
    pub datatypes: Vec<DatatypeDef>,
    #[serde(default)]
    pub logical_functions: Vec<LogicalFunctionDef>,
    #[serde(default)]
    pub functions: Vec<FunctionSpec>,
}

#[derive(Debug, thiserror::Error)]
pub enum SpecError {
    #[error("failed to parse spec JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl SpecModule {
    pub fn from_json(s: &str) -> Result<Self, SpecError> {
        Ok(crate::gen_dsl::json::from_str(s)?)
    }

    pub fn to_json(&self) -> Result<String, SpecError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn function(&self, name: &str) -> Option<&FunctionSpec> {
        self.functions.iter().find(|f| f.name == name)
    }

    pub fn predicate(&self, name: &crate::gen_dsl::Sym) -> Option<&PredicateDef> {
        self.predicates.iter().find(|p| p.name == *name)
    }

    pub fn datatype(&self, name: &crate::gen_dsl::Sym) -> Option<&DatatypeDef> {
        self.datatypes.iter().find(|d| d.name == *name)
    }

    pub fn logical_function(&self, name: &crate::gen_dsl::Sym) -> Option<&LogicalFunctionDef> {
        self.logical_functions.iter().find(|f| f.name == *name)
    }
}
