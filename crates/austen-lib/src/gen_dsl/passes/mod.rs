pub mod apply_array_max_length;
pub mod branch_pruning;
pub mod choice_effects;
pub mod destruct_arbitrary;
pub mod destruct_products;
pub mod each_fusion;
pub mod flip_ifs;
mod framework;
pub mod heap_args;
pub mod inline_gen;
pub mod inline_term;

pub mod lift;
pub mod member_indirection;
pub mod normalize_scopes;
pub mod partial_evaluation;
mod path_condition;
pub mod pointees;
pub mod prune_args;
pub mod prune_call_graph;
pub mod prune_returns;
pub mod push_pull;
pub mod reify_tree;
pub mod remove_unused;
pub mod reorder;
pub mod simplify_gen;
pub mod simplify_index_term;
pub mod sink_args;
pub mod size_generators;
#[cfg(feature = "smt")]
mod smt_pruning;
pub mod specialize_guarded_choices;
pub mod specialize_singleton;
mod stage;
mod tail;
pub mod term_bounds;
pub mod undrawable_defaults;
pub mod unfold;
pub mod unfold_maps;

pub use apply_array_max_length::{DEFAULT_MAX_ARRAY_LENGTH, apply_array_max_length};
pub use branch_pruning::{prune_branches, prune_branches_with_paths};
pub use choice_effects::{ChoiceEffects, EffectBits};
pub use destruct_arbitrary::destruct_arbitrary;
pub use destruct_products::destruct_products;
pub use each_fusion::each_fusion;
pub use flip_ifs::flip_ifs;
#[allow(unused_imports)]
pub(in crate::gen_dsl) use framework::GeneratorPass;
#[cfg(feature = "smt")]
pub(in crate::gen_dsl) use framework::SmtPruningPass;
pub(in crate::gen_dsl) use framework::{
    AnnotatePointeesPass, ApplyArrayMaxLengthPass, DestructArbitraryPass, DestructProductsPass,
    EachFusionPass, FlipIfsPass, ForEachGenerator, GenModulePass, HeapArgsPass, InlineGenPass,
    NormalizeScopesPass, PruneArgsPass, PruneReturnsPass, ReifyTreePass, ReorderPass,
    SimplifyGenPass, SinkArgsPass, SizeGeneratorsPass, SpecializeGuardedChoicesPass,
    SpecializeSingletonPass, UnfoldPass,
};
pub use heap_args::heap_args;
pub use inline_gen::{InlineMode, inline_gen};
pub use inline_term::inline_term;
pub use lift::{lift_constraints, lift_constraints_with_options};
pub use member_indirection::{MemberKind, member_indirection, replace_memberof_gt};
pub use normalize_scopes::normalize_scopes;
pub use partial_evaluation::partial_evaluation;
pub use pointees::annotate_pointees;
pub use prune_args::prune_args;
pub use prune_call_graph::prune_call_graph;
pub use prune_returns::prune_returns;
pub use push_pull::{MAX_PUSH_GROWTH, push_pull, push_pull_bounded};
pub use reify_tree::{reify_tree, scope_choices};
pub use remove_unused::remove_unused;
pub use reorder::reorder;
pub use simplify_gen::{SIMPLIFY_FUEL, simplify_gen, simplify_gen_with_push_budget};
pub use simplify_index_term::{GoodHandling, simplify_index_term};
pub use sink_args::sink_args;
pub use size_generators::size_generators;
#[cfg(feature = "smt")]
pub use smt_pruning::prune_smt;
pub use specialize_guarded_choices::{
    MAX_GUARDED_GROWTH, specialize_guarded_choices, specialize_guarded_choices_bounded,
};
pub use specialize_singleton::{SingletonSearch, specialize_singleton};
pub use stage::{OBSERVED_STAGES, StageError, StagedGenModules, stage_module};
pub use undrawable_defaults::undrawable_defaults_to_unknown;
pub use unfold::unfold;
pub use unfold_maps::unfold_maps;

use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind, ScopeName};
use crate::gen_dsl::lang::sctype::Sctype;
use crate::gen_dsl::lang::sym::Sym;

pub(in crate::gen_dsl) fn peel_scopes(mut gt: GenTerm) -> (Vec<ScopeName>, GenTerm) {
    let mut scopes = Vec::new();
    loop {
        let (term, bt) = gt.into_parts();
        match term {
            GenTermKind::Scope { name, body } => {
                scopes.push(name);
                gt = *body;
            }
            term => return (scopes, GenTerm::from_parts(term, bt)),
        }
    }
}

pub(in crate::gen_dsl) fn wrap_scopes(scopes: Vec<ScopeName>, gt: GenTerm) -> GenTerm {
    scopes
        .into_iter()
        .rev()
        .fold(gt, |body, name| GenTerm::scope(name, body))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PassProfile {
    pub lift_constraints: bool,
    pub each_fusion: bool,
    pub apply_array_max_length: bool,
    pub specialize_guarded_choices: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassOptions {
    pub path_pruning: bool,
    pub smt_pruning: bool,
    pub inline: InlineMode,
    pub max_unfolds: Option<usize>,
    pub experimental_product_arg_destruction: bool,
    pub experimental_return_pruning: bool,
    pub experimental_arg_pruning: bool,
    pub sink_args: bool,
    pub specialize_guarded_choices: bool,
    pub max_array_length: usize,
}

impl PassOptions {
    pub const NONE: PassOptions = PassOptions {
        path_pruning: false,
        smt_pruning: false,
        inline: InlineMode::Nothing,
        max_unfolds: None,
        experimental_product_arg_destruction: false,
        experimental_return_pruning: false,
        experimental_arg_pruning: false,
        sink_args: false,
        specialize_guarded_choices: false,
        max_array_length: DEFAULT_MAX_ARRAY_LENGTH,
    };

    pub fn production_only_requests(&self) -> [(&'static str, bool); 6] {
        [
            ("--path-pruning", self.path_pruning),
            ("--smt-pruning", self.smt_pruning),
            ("--inline", self.inline != InlineMode::Nothing),
            ("--max-unfolds", self.max_unfolds.is_some_and(|max| max > 0)),
            ("--sink-args", self.sink_args),
            (
                "--specialize-guarded-choices",
                self.specialize_guarded_choices,
            ),
        ]
    }
}

impl PassProfile {
    pub const NONE: PassProfile = PassProfile {
        lift_constraints: false,
        each_fusion: false,
        apply_array_max_length: false,
        specialize_guarded_choices: false,
    };

    pub const fn union(self, other: PassProfile) -> PassProfile {
        PassProfile {
            lift_constraints: self.lift_constraints || other.lift_constraints,
            each_fusion: self.each_fusion || other.each_fusion,
            apply_array_max_length: self.apply_array_max_length || other.apply_array_max_length,
            specialize_guarded_choices: self.specialize_guarded_choices
                || other.specialize_guarded_choices,
        }
    }

    pub const AXES: &'static [(&'static str, PassProfile)] = &[
        (
            "lift",
            PassProfile {
                lift_constraints: true,
                each_fusion: false,
                apply_array_max_length: false,
                specialize_guarded_choices: false,
            },
        ),
        (
            "fusion",
            PassProfile {
                lift_constraints: false,
                each_fusion: true,
                apply_array_max_length: false,
                specialize_guarded_choices: false,
            },
        ),
        (
            "array_max",
            PassProfile {
                lift_constraints: false,
                each_fusion: false,
                apply_array_max_length: true,
                specialize_guarded_choices: false,
            },
        ),
        (
            "guarded",
            PassProfile {
                lift_constraints: false,
                each_fusion: false,
                apply_array_max_length: false,
                specialize_guarded_choices: true,
            },
        ),
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransformError {
    #[error("unknown struct tag {tag:?}")]
    UnknownStruct { tag: Sym },
    #[error("cannot draw an array of unknown length")]
    UnsizedArray,
    #[error("array length {len} is not representable as an index")]
    ArrayLenOutOfRange { len: usize },
    #[error("no logical base type for C type {ct:?}")]
    UnsupportedSctype { ct: Sctype },
    #[error("unsupported index type {bt:?} in permission")]
    UnsupportedIndexType { bt: BaseType },
    #[error("spine dependencies cycle at {sym:?}")]
    CyclicSpineDependency { sym: Sym },
    #[error("unschedulable spine statements read unbound symbols {syms:?}")]
    UnschedulableStatements { syms: Vec<Sym> },
    #[error("call to unknown generator {fsym:?}")]
    UnknownCallee { fsym: Sym },
    #[error("SMT pruning requires the `smt` feature")]
    SmtUnavailable,
    #[error("inlining cycles at {fsym:?}")]
    InlineCycle { fsym: Sym },
    #[error("call to {fsym:?} passes {actual} arguments where {expected} are declared")]
    CallArityMismatch {
        fsym: Sym,
        expected: usize,
        actual: usize,
    },
    #[error("C-typed parameter {param:?} has a record or tuple logical type")]
    AggregateCTypedParam { param: Sym },
}
