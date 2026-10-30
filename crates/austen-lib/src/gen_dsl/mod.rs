pub mod choice;
mod dsl;
pub mod fnv;
mod incoming;
pub mod json;
pub mod lang;
mod marked_heap;
pub mod passes;
mod pointee;
pub mod primitive;
pub mod rand_like;
pub mod region;
pub mod reject;
#[cfg(feature = "smt")]
pub mod smt;
pub mod stats;

pub use choice::draws;
pub use choice::{
    AsgnTarget, AsgnWrite, AttachError, Blind, Bv, Choice, ChoiceTape, ChoiceTree, Draw,
    DrawSource, DrawTape, EntropyBacked, GenFailure, Label, MapCandidates, MapDomainError,
    ParseOutput, ProduceCx, ReconstructError, RejectingProducer, Scope, ScopeError, ScopeId,
    ScopeKey, ScopeMergeError, SiteGen, SizeArgError, SymbolicMapBounds, TapeParser,
    TranslateError, TreeLinearizer, TreeProducer, asgn_target, bind_size_args, check_asgn_granted,
    check_asgn_incoming, check_asgn_target_incoming, choice_of_value, evaluated_write_choice,
    map_candidate_range, map_domain, map_permission_holds, request_evaluator, root_test_case_size,
    symbolic_map_bounds, validate_size_args, write_choice_of,
};
pub use dsl::{DEFAULT_DSL_WIDTH, DslFormatter, sym_text};
pub use incoming::{
    GLOBAL_LAYOUT_PREFIX, GlobalLayoutError, GlobalObject, Grant, Incoming, IncomingError,
    PublishedGlobal, global_layout, global_layout_tables, incoming_from_published,
};
pub use lang::eval::{
    DefaultChoice, DefaultChoices, DefaultError, DefaultTypes, DefaultTypesError,
};
pub use lang::sym_scan;
pub use lang::{
    BaseType, BinOp, Bindings, CARRIER_BITS, ChooseBoolError, Const, ConstructorDef, DatatypeDef,
    Env, EnvMark, Evaluator, FunctionBody, FunctionTableError, GenCallError, GenDeclarations,
    GenDef, GenModule, GenModuleError, GenTerm, GenTermKind, GlobalDef, GlobalLinkage, Id,
    IndexTerm, IntegerBaseType, IntegerType, LogicalConstraint, LogicalFunctionDef, MemberTypes,
    Pattern, PatternKind, PointerOrigin, Qualifiers, Scalar, ScopeName, Sctype, Sign, StructDef,
    StructLayout, Substitution, Sym, SymGen, SymMap, SymSet, Term, UnOp, Value, ValueCheckError,
    array_value, bits_range, bits_residue, divisible, eval, eval_in, free_vars_bts_list,
    gen_term_watermark, good, good_pointer_alignment, in_z_range, normalise_to_range_big, reduce,
    representable, sym_watermark, uintptr_bt, unfoldable_defs, value_bytes, value_check,
    value_from_bytes, well_formed,
};
pub use marked_heap::{AllocError, HeapMark, MarkedHeap};
pub use passes::{
    GoodHandling, OBSERVED_STAGES, StageError, StagedGenModules, TransformError, annotate_pointees,
    destruct_arbitrary, lift_constraints, partial_evaluation, reify_tree, reorder, scope_choices,
    simplify_index_term, stage_module, unfold_maps,
};
pub use pointee::{DEFAULT_POINTEE_BYTES, pointee_bytes};
pub use primitive::{PrimitiveContext, PrimitiveDraw, PrimitiveGenerator, sized_scalar};
pub use rand_like::{
    Entropy, EntropyMark, InputDecisions, RecordingError, RecordingLog, Rewind, Rng, SplitMix64,
    TestCaseSize, TestCaseSizeError, TruncatedDecision, recording,
};
pub use region::{Congruence, Region};
pub use reject::{RejectReason, RejectTally};
pub use stats::{SiteCount, SiteId, SiteIndex, SiteTally, TimingStats, Timings, merge_timings};

pub const DEFAULT_HEAP_BYTES: u64 = 1 << 20;

pub const DEFAULT_HEAP_BASE: u64 = if cfg!(target_os = "macos") {
    0x2000_0000_0000
} else {
    0x4000_0000
};
