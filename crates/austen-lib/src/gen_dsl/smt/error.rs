use crate::gen_dsl::{BaseType, Id, Sctype, Sym};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SmtError {
    #[error("no checked SMT semantics for {construct} at type {bt:?}")]
    UnsupportedCheckedSemantics {
        construct: &'static str,
        bt: BaseType,
    },

    #[error("checked SMT translation refuses recursive helper {name:?}")]
    RecursiveCheckedFunction { name: Sym },

    #[error("no SMT sort for base type {0:?}")]
    UnsupportedBaseType(BaseType),

    #[error("cannot translate {term} at type {bt:?}")]
    UnsupportedTerm { term: &'static str, bt: BaseType },

    #[error("operator {op} has no SMT reading at type {bt:?}")]
    UnsupportedOperator { op: &'static str, bt: BaseType },

    #[error("cannot cast {from:?} to {to:?}")]
    UnsupportedCast { from: BaseType, to: BaseType },

    #[error(
        "{what} needs pointer provenance; this translator reads pointers as flat addresses \
         (open it with PointerEncoding::Provenance)"
    )]
    NeedsProvenance { what: &'static str },

    #[error("exponent must be a non-negative literal, not {found}")]
    UnsupportedSymbolicExponent { found: &'static str },

    #[error("symbol {sym:?} is not bound")]
    UnboundSymbol { sym: Sym },

    #[error("expected type {expected:?}, got {actual:?}")]
    TypeMismatch {
        expected: BaseType,
        actual: BaseType,
    },

    #[error("bitvector width must be non-zero")]
    ZeroWidthBitvector,

    #[error("no LP64 size for C type {0:?}")]
    UnsizedCtype(Sctype),

    #[error("declaration of {0:?} contains itself")]
    CyclicDeclaration(BaseType),

    #[error("no definition for struct {tag:?}")]
    UnknownStruct { tag: Sym },

    #[error("struct {tag:?} has no member {member:?}")]
    UnknownStructMember { tag: Sym, member: Id },

    #[error("record type has no member {member:?}")]
    UnknownRecordMember { member: Id },

    #[error("tuple of {arity} has no component {index}")]
    TupleIndexOutOfRange { index: usize, arity: usize },

    #[error("no declaration for datatype {tag:?} (no datatype table entry)")]
    UnknownDatatype { tag: Sym },

    #[error("datatype {tag:?} has no constructors and cannot be represented by Z3")]
    EmptyDatatype { tag: Sym },

    #[error("datatype {tag:?} has no finite inhabitant and cannot be represented by Z3")]
    NonWellFoundedDatatype { tag: Sym },

    #[error("datatype {tag:?} is declared more than once")]
    DuplicateDatatype { tag: Sym },

    #[error("datatype {tag:?} has no constructor {constructor:?}")]
    UnknownConstructor { tag: Sym, constructor: Sym },

    #[error("constructor {constructor:?} is declared more than once")]
    DuplicateConstructor { constructor: Sym },

    #[error("constructor {constructor:?} of datatype {tag:?} has no member {member:?}")]
    UnknownConstructorMember {
        tag: Sym,
        constructor: Sym,
        member: Id,
    },

    #[error(
        "datatype {tag:?} declares member {member:?} more than once (again in constructor {constructor:?})"
    )]
    DuplicateConstructorMember {
        tag: Sym,
        constructor: Sym,
        member: Id,
    },

    #[error("constructor {constructor:?} of datatype {tag:?} is missing member {member:?}")]
    MissingConstructorMember {
        tag: Sym,
        constructor: Sym,
        member: Id,
    },

    #[error(
        "constructor {constructor:?} of datatype {tag:?} takes {expected} member(s), got {actual}"
    )]
    ConstructorArity {
        tag: Sym,
        constructor: Sym,
        expected: usize,
        actual: usize,
    },

    #[error("no declaration for logical function {name:?}")]
    UnknownLogicalFunction { name: Sym },

    #[error("logical function {name:?} is uninterpreted (declared with no body)")]
    UninterpretedFunction { name: Sym },

    #[error("logical function {name:?} takes {expected} argument(s), applied to {actual}")]
    FunctionArity {
        name: Sym,
        expected: usize,
        actual: usize,
    },

    #[error("{what} expands to {count} instances, over the limit of {limit}")]
    ExpansionLimit {
        what: &'static str,
        count: u128,
        limit: u128,
    },

    #[error("Z3 rejected the {kind} numeral {value}")]
    BadNumeral { kind: &'static str, value: String },

    #[error("cannot decode a model value of type {bt:?}: {why}")]
    ModelDecode { bt: BaseType, why: String },
}

impl SmtError {
    pub(in crate::gen_dsl) fn unsupported(term: &'static str, bt: &BaseType) -> Self {
        SmtError::UnsupportedTerm {
            term,
            bt: bt.clone(),
        }
    }
}

impl From<crate::gen_dsl::ValueCheckError> for SmtError {
    fn from(e: crate::gen_dsl::ValueCheckError) -> Self {
        use crate::gen_dsl::ValueCheckError as V;
        match e {
            V::UnsizedCtype(ct) => SmtError::UnsizedCtype(ct),
            V::UnknownStruct { tag } => SmtError::UnknownStruct { tag },
            V::TypeMismatch { expected, actual } => SmtError::TypeMismatch { expected, actual },
            V::UnsupportedBaseType(bt) => SmtError::UnsupportedBaseType(bt),
            V::TooDeep { limit } => SmtError::ExpansionLimit {
                what: "value check nesting",
                count: limit as u128 + 1,
                limit: limit as u128,
            },
        }
    }
}
