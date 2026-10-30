use strum::{EnumCount as _, VariantArray as _};

use crate::gen_dsl::{GenDef, GenModule, GenModuleError, ScopeError, Sym, SymGen};

use super::SingletonSearch;
#[cfg(feature = "smt")]
use super::SmtPruningPass;
use super::prune_call_graph::prune_call_graph;
use super::{
    AnnotatePointeesPass, ApplyArrayMaxLengthPass, DestructArbitraryPass, DestructProductsPass,
    EachFusionPass, FlipIfsPass, ForEachGenerator, GenModulePass, GeneratorPass, HeapArgsPass,
    InlineGenPass, InlineMode, NormalizeScopesPass, PassOptions, PassProfile, PruneArgsPass,
    PruneReturnsPass, ReifyTreePass, ReorderPass, SimplifyGenPass, SinkArgsPass,
    SizeGeneratorsPass, SpecializeGuardedChoicesPass, SpecializeSingletonPass, TransformError,
    UnfoldPass,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedGenModules {
    pub wire: GenModule,
    pub production: GenModule,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StageError {
    #[error(transparent)]
    Module(#[from] GenModuleError),
    #[error(transparent)]
    Transform(#[from] TransformError),
    #[error("generator {name:?} is not well scoped: {error}")]
    NotWellScoped { name: Sym, error: ScopeError },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumCount, strum::VariantArray)]
enum Stage {
    Convert,
    DestructProducts,
    PruneReturns,
    PruneArgs,
    ReifyTree,
    SizeGenerators,
    DestructArbitrary,
    SinkArgs,
    Unfold,
    NormalizeScopes,
    SimplifyGen,
    InlineGen,
    SimplifyPostInline,
    EachFusion,
    ApplyArrayMaxLength,
    SimplifyPostFusion,
    FlipIfs,
    SimplifyPostFlip,
    SpecializeGuardedChoices,
    Reorder,
    SpecializeSingleton,
    SimplifyFinal,
    SpecializePostSweep,
    SimplifyPostSpecialize,
    NormalizeScopesFinal,
    AnnotatePointees,
    HeapArgs,
    SmtPruning,
}

impl Stage {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Convert => "convert",
            Self::DestructProducts => "destruct_products",
            Self::PruneReturns => "prune_returns",
            Self::PruneArgs => "prune_args",
            Self::ReifyTree => "reify_tree",
            Self::SizeGenerators => "size_generators",
            Self::DestructArbitrary => "destruct_arbitrary",
            Self::SinkArgs => "sink_args",
            Self::Unfold => "unfold",
            Self::NormalizeScopes => "normalize_scopes",
            Self::SimplifyGen => "simplify_gen",
            Self::InlineGen => "inline_gen",
            Self::SimplifyPostInline => "simplify_post_inline",
            Self::EachFusion => "each_fusion",
            Self::ApplyArrayMaxLength => "apply_array_max_length",
            Self::SimplifyPostFusion => "simplify_post_fusion",
            Self::FlipIfs => "flip_ifs",
            Self::SimplifyPostFlip => "simplify_post_flip",
            Self::SpecializeGuardedChoices => "specialize_guarded_choices",
            Self::Reorder => "reorder",
            Self::SpecializeSingleton => "specialize_singleton",
            Self::SimplifyFinal => "simplify_final",
            Self::SpecializePostSweep => "specialize_post_sweep",
            Self::SimplifyPostSpecialize => "simplify_post_specialize",
            Self::NormalizeScopesFinal => "normalize_scopes_final",
            Self::AnnotatePointees => "annotate_pointees",
            Self::HeapArgs => "heap_args",
            Self::SmtPruning => "smt_pruning",
        }
    }
}

pub const OBSERVED_STAGES: [&str; Stage::COUNT] = {
    let mut names = [""; Stage::COUNT];
    let mut i = 0;
    while i < Stage::VARIANTS.len() {
        names[i] = Stage::VARIANTS[i].as_str();
        i += 1;
    }
    names
};

fn observe(
    observer: &mut dyn FnMut(&'static str, &Sym, &GenDef),
    stage: Stage,
    module: &GenModule,
) {
    for (name, def) in &module.defs {
        observer(stage.as_str(), name, def);
    }
}

fn apply<P: GenModulePass>(
    module: GenModule,
    mut pass: P,
    syms: &mut SymGen,
) -> Result<GenModule, StageError> {
    Ok(pass.transform(module, syms)?)
}

fn apply_if<P: GenModulePass>(
    module: GenModule,
    enabled: bool,
    pass: P,
    syms: &mut SymGen,
) -> Result<GenModule, StageError> {
    if enabled {
        apply(module, pass, syms)
    } else {
        Ok(module)
    }
}

fn prune(module: &mut GenModule) {
    prune_call_graph(&mut module.defs);
}

fn apply_pruned<P: GenModulePass>(
    module: GenModule,
    pass: P,
    syms: &mut SymGen,
) -> Result<GenModule, StageError> {
    let mut module = apply(module, pass, syms)?;
    prune(&mut module);
    Ok(module)
}

fn apply_pruned_if<P: GenModulePass>(
    module: GenModule,
    enabled: bool,
    pass: P,
    syms: &mut SymGen,
) -> Result<GenModule, StageError> {
    if enabled {
        apply_pruned(module, pass, syms)
    } else {
        Ok(module)
    }
}

fn apply_each<P: GeneratorPass>(
    module: GenModule,
    mut pass: ForEachGenerator<P>,
    syms: &mut SymGen,
    stage: Stage,
    observer: &mut dyn FnMut(&'static str, &Sym, &GenDef),
) -> Result<GenModule, StageError> {
    Ok(pass.transform_observed(module, syms, &mut |name, def| {
        observer(stage.as_str(), name, def);
    })?)
}

fn apply_each_if<P: GeneratorPass>(
    module: GenModule,
    enabled: bool,
    pass: ForEachGenerator<P>,
    syms: &mut SymGen,
    stage: Stage,
    observer: &mut dyn FnMut(&'static str, &Sym, &GenDef),
) -> Result<GenModule, StageError> {
    if enabled {
        apply_each(module, pass, syms, stage, observer)
    } else {
        observe(observer, stage, &module);
        Ok(module)
    }
}

#[cfg(feature = "smt")]
fn apply_each_pruned_if<P: GeneratorPass>(
    module: GenModule,
    enabled: bool,
    mut pass: ForEachGenerator<P>,
    syms: &mut SymGen,
    stage: Stage,
    observer: &mut dyn FnMut(&'static str, &Sym, &GenDef),
) -> Result<GenModule, StageError> {
    let mut module = if enabled {
        pass.transform_observed(module, syms, &mut |_, _| {})?
    } else {
        module
    };
    if enabled {
        prune(&mut module);
    }
    observe(observer, stage, &module);
    Ok(module)
}

pub fn stage_module(
    mut module: GenModule,
    mut syms: SymGen,
    profile: PassProfile,
    options: PassOptions,
    observer: &mut dyn FnMut(&'static str, &Sym, &GenDef),
) -> Result<StagedGenModules, StageError> {
    module.validate()?;
    observe(observer, Stage::Convert, &module);

    tracing::debug!(
        pass = Stage::DestructProducts.as_str(),
        enabled = options.experimental_product_arg_destruction,
        "stage 1"
    );
    module = apply_if(
        module,
        options.experimental_product_arg_destruction,
        DestructProductsPass,
        &mut syms,
    )?;
    observe(observer, Stage::DestructProducts, &module);

    tracing::debug!(
        pass = Stage::PruneReturns.as_str(),
        enabled = options.experimental_return_pruning,
        "stage 1"
    );
    module = apply_pruned_if(
        module,
        options.experimental_return_pruning,
        PruneReturnsPass,
        &mut syms,
    )?;
    observe(observer, Stage::PruneReturns, &module);

    tracing::debug!(
        pass = Stage::PruneArgs.as_str(),
        enabled = options.experimental_arg_pruning,
        "stage 1"
    );
    module = apply_if(
        module,
        options.experimental_arg_pruning,
        PruneArgsPass,
        &mut syms,
    )?;
    observe(observer, Stage::PruneArgs, &module);

    tracing::debug!(pass = Stage::ReifyTree.as_str(), "choice-tree boundary");
    module = apply_each(
        module,
        ForEachGenerator(ReifyTreePass),
        &mut syms,
        Stage::ReifyTree,
        observer,
    )?;

    tracing::debug!(pass = Stage::SizeGenerators.as_str(), "stage 5");
    module = apply(module, SizeGeneratorsPass, &mut syms)?;
    observe(observer, Stage::SizeGenerators, &module);

    module.validate()?;
    let wire = module;
    let mut production = wire.clone();

    tracing::debug!(pass = Stage::DestructArbitrary.as_str(), "stage 1");
    production = apply_each(
        production,
        ForEachGenerator(DestructArbitraryPass),
        &mut syms,
        Stage::DestructArbitrary,
        observer,
    )?;

    tracing::debug!(
        pass = Stage::SinkArgs.as_str(),
        enabled = options.sink_args,
        "production"
    );
    production = apply_pruned_if(production, options.sink_args, SinkArgsPass, &mut syms)?;
    observe(observer, Stage::SinkArgs, &production);

    tracing::debug!(
        pass = Stage::Unfold.as_str(),
        max = options.max_unfolds,
        enabled = options.max_unfolds.is_some(),
        "stage 1"
    );
    production = apply_pruned_if(
        production,
        options.max_unfolds.is_some(),
        UnfoldPass {
            max: options.max_unfolds.unwrap_or(0),
        },
        &mut syms,
    )?;
    observe(observer, Stage::Unfold, &production);

    production = apply(production, NormalizeScopesPass, &mut syms)?;
    observe(observer, Stage::NormalizeScopes, &production);

    production = apply_pruned(
        production,
        SimplifyGenPass {
            passes: profile,
            options,
            stage: Stage::SimplifyGen.as_str(),
            push: false,
        },
        &mut syms,
    )?;
    observe(observer, Stage::SimplifyGen, &production);

    tracing::debug!(
        pass = Stage::InlineGen.as_str(),
        mode = ?options.inline,
        enabled = options.inline != InlineMode::Nothing,
        "stage 2"
    );
    production = apply_if(
        production,
        options.inline != InlineMode::Nothing,
        InlineGenPass {
            mode: options.inline,
        },
        &mut syms,
    )?;
    observe(observer, Stage::InlineGen, &production);

    production = apply_pruned_if(
        production,
        options.inline != InlineMode::Nothing,
        SimplifyGenPass {
            passes: profile,
            options,
            stage: Stage::SimplifyPostInline.as_str(),
            push: false,
        },
        &mut syms,
    )?;
    observe(observer, Stage::SimplifyPostInline, &production);

    tracing::debug!(
        pass = Stage::EachFusion.as_str(),
        elected = profile.each_fusion,
        "stage 2"
    );
    production = apply_each_if(
        production,
        profile.each_fusion,
        ForEachGenerator(EachFusionPass),
        &mut syms,
        Stage::EachFusion,
        observer,
    )?;

    tracing::debug!(
        pass = Stage::ApplyArrayMaxLength.as_str(),
        max = options.max_array_length,
        elected = profile.apply_array_max_length,
        "stage 2"
    );
    production = apply_each_if(
        production,
        profile.apply_array_max_length,
        ForEachGenerator(ApplyArrayMaxLengthPass {
            max_array_length: options.max_array_length,
        }),
        &mut syms,
        Stage::ApplyArrayMaxLength,
        observer,
    )?;

    production = apply_pruned(
        production,
        SimplifyGenPass {
            passes: profile,
            options,
            stage: Stage::SimplifyPostFusion.as_str(),
            push: false,
        },
        &mut syms,
    )?;
    observe(observer, Stage::SimplifyPostFusion, &production);

    tracing::debug!(pass = Stage::FlipIfs.as_str(), "stage 2");
    production = apply_each(
        production,
        ForEachGenerator(FlipIfsPass),
        &mut syms,
        Stage::FlipIfs,
        observer,
    )?;

    production = apply_pruned(
        production,
        SimplifyGenPass {
            passes: profile,
            options,
            stage: Stage::SimplifyPostFlip.as_str(),
            push: true,
        },
        &mut syms,
    )?;
    observe(observer, Stage::SimplifyPostFlip, &production);

    let guarded = profile.specialize_guarded_choices && options.specialize_guarded_choices;
    tracing::debug!(
        pass = Stage::SpecializeGuardedChoices.as_str(),
        elected = profile.specialize_guarded_choices,
        enabled = options.specialize_guarded_choices,
        "stage 2"
    );
    production = apply_each_if(
        production,
        guarded,
        ForEachGenerator(SpecializeGuardedChoicesPass),
        &mut syms,
        Stage::SpecializeGuardedChoices,
        observer,
    )?;

    tracing::debug!(pass = Stage::Reorder.as_str(), "stage 2");
    production = apply_each(
        production,
        ForEachGenerator(ReorderPass),
        &mut syms,
        Stage::Reorder,
        observer,
    )?;

    tracing::debug!(pass = Stage::SpecializeSingleton.as_str(), "stage 2");
    production = apply_each(
        production,
        ForEachGenerator(SpecializeSingletonPass {
            search: SingletonSearch::StopAtLet,
        }),
        &mut syms,
        Stage::SpecializeSingleton,
        observer,
    )?;

    production = apply_pruned(
        production,
        SimplifyGenPass {
            passes: profile,
            options,
            stage: Stage::SimplifyFinal.as_str(),
            push: false,
        },
        &mut syms,
    )?;
    observe(observer, Stage::SimplifyFinal, &production);

    tracing::debug!(pass = Stage::SpecializePostSweep.as_str(), "stage 2");
    production = apply_each(
        production,
        ForEachGenerator(SpecializeSingletonPass {
            search: SingletonSearch::StopAtLet,
        }),
        &mut syms,
        Stage::SpecializePostSweep,
        observer,
    )?;

    production = apply_pruned(
        production,
        SimplifyGenPass {
            passes: profile,
            options,
            stage: Stage::SimplifyPostSpecialize.as_str(),
            push: false,
        },
        &mut syms,
    )?;
    observe(observer, Stage::SimplifyPostSpecialize, &production);

    production = apply(production, NormalizeScopesPass, &mut syms)?;
    observe(observer, Stage::NormalizeScopesFinal, &production);

    production = apply(production, AnnotatePointeesPass, &mut syms)?;
    observe(observer, Stage::AnnotatePointees, &production);

    tracing::debug!(pass = Stage::HeapArgs.as_str(), "production");
    production = apply(production, HeapArgsPass, &mut syms)?;
    observe(observer, Stage::HeapArgs, &production);

    tracing::debug!(
        pass = Stage::SmtPruning.as_str(),
        enabled = options.smt_pruning,
        "stage 4"
    );
    #[cfg(feature = "smt")]
    {
        production = apply_each_pruned_if(
            production,
            options.smt_pruning,
            ForEachGenerator(SmtPruningPass),
            &mut syms,
            Stage::SmtPruning,
            observer,
        )?;
    }
    #[cfg(not(feature = "smt"))]
    {
        if options.smt_pruning {
            return Err(TransformError::SmtUnavailable.into());
        }
        observe(observer, Stage::SmtPruning, &production);
    }

    for (name, def) in &wire.defs {
        def.well_scoped()
            .map_err(|error| StageError::NotWellScoped {
                name: name.clone(),
                error,
            })?;
    }
    for (name, def) in &production.defs {
        def.well_scoped_reopenable()
            .map_err(|error| StageError::NotWellScoped {
                name: name.clone(),
                error,
            })?;
    }
    production.validate()?;

    Ok(StagedGenModules { wire, production })
}
