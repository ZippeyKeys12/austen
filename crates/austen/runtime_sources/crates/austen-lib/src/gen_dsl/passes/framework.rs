use crate::gen_dsl::{
    GenDeclarations, GenDef, GenModule, GenTerm, IndexTerm, SiteGen, Sym, SymGen,
};

use super::{
    ChoiceEffects, InlineMode, MAX_PUSH_GROWTH, PassOptions, PassProfile, SingletonSearch,
    TransformError, annotate_pointees, apply_array_max_length, destruct_arbitrary,
    destruct_products, each_fusion, flip_ifs, heap_args, inline_gen, normalize_scopes, prune_args,
    prune_returns, reify_tree, reorder, simplify_gen_with_push_budget, sink_args, size_generators,
    specialize_guarded_choices, specialize_singleton, unfold,
};

pub(in crate::gen_dsl) trait GeneratorPass {
    fn transform(
        &mut self,
        def: &GenDef,
        body: GenTerm,
        declarations: GenDeclarations<'_>,
        syms: &mut SymGen,
    ) -> Result<GenTerm, TransformError>;
}

pub(in crate::gen_dsl) trait GenModulePass {
    fn transform(
        &mut self,
        module: GenModule,
        syms: &mut SymGen,
    ) -> Result<GenModule, TransformError>;
}

#[derive(Debug, Clone, Copy)]
pub(in crate::gen_dsl) struct ForEachGenerator<P>(pub(in crate::gen_dsl) P);

impl<P: GeneratorPass> ForEachGenerator<P> {
    pub(in crate::gen_dsl) fn transform_observed(
        &mut self,
        module: GenModule,
        syms: &mut SymGen,
        observe: &mut dyn FnMut(&Sym, &GenDef),
    ) -> Result<GenModule, TransformError> {
        debug_assert!(module.validate().is_ok(), "passes require a valid module");
        let mut module = module;
        let defs = std::mem::take(&mut module.defs);
        let declarations = module.declarations();
        let mut transformed = Vec::with_capacity(defs.len());
        for (key, mut def) in defs {
            let body = std::mem::replace(&mut def.body, placeholder_body());
            def.body = self.0.transform(&def, body, declarations, syms)?;
            debug_assert_eq!(key, def.name);
            observe(&key, &def);
            transformed.push((key, def));
        }
        module.defs = transformed;
        Ok(module)
    }
}

fn placeholder_body() -> GenTerm {
    GenTerm::return_(IndexTerm::unit())
}

#[derive(Debug, Clone, Copy, Default)]
pub(in crate::gen_dsl) struct ReifyTreePass;

impl GeneratorPass for ReifyTreePass {
    fn transform(
        &mut self,
        _def: &GenDef,
        body: GenTerm,
        _declarations: GenDeclarations<'_>,
        syms: &mut SymGen,
    ) -> Result<GenTerm, TransformError> {
        Ok(reify_tree(body, syms, &mut SiteGen::new()))
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(in crate::gen_dsl) struct DestructArbitraryPass;

impl GeneratorPass for DestructArbitraryPass {
    fn transform(
        &mut self,
        _def: &GenDef,
        body: GenTerm,
        declarations: GenDeclarations<'_>,
        syms: &mut SymGen,
    ) -> Result<GenTerm, TransformError> {
        destruct_arbitrary(body, declarations.structs(), syms)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(in crate::gen_dsl) struct EachFusionPass;

impl GeneratorPass for EachFusionPass {
    fn transform(
        &mut self,
        def: &GenDef,
        body: GenTerm,
        _declarations: GenDeclarations<'_>,
        syms: &mut SymGen,
    ) -> Result<GenTerm, TransformError> {
        each_fusion(&def.iarg_syms(), body, syms)
    }
}

#[derive(Debug, Clone, Copy)]
pub(in crate::gen_dsl) struct ApplyArrayMaxLengthPass {
    pub(in crate::gen_dsl) max_array_length: usize,
}

impl GeneratorPass for ApplyArrayMaxLengthPass {
    fn transform(
        &mut self,
        _def: &GenDef,
        body: GenTerm,
        _declarations: GenDeclarations<'_>,
        _syms: &mut SymGen,
    ) -> Result<GenTerm, TransformError> {
        apply_array_max_length(body, self.max_array_length)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(in crate::gen_dsl) struct FlipIfsPass;

impl GeneratorPass for FlipIfsPass {
    fn transform(
        &mut self,
        def: &GenDef,
        body: GenTerm,
        _declarations: GenDeclarations<'_>,
        syms: &mut SymGen,
    ) -> Result<GenTerm, TransformError> {
        Ok(flip_ifs(&def.iarg_syms(), body, syms))
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(in crate::gen_dsl) struct ReorderPass;

impl GeneratorPass for ReorderPass {
    fn transform(
        &mut self,
        def: &GenDef,
        body: GenTerm,
        _declarations: GenDeclarations<'_>,
        _syms: &mut SymGen,
    ) -> Result<GenTerm, TransformError> {
        reorder(&def.iarg_syms(), body)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(in crate::gen_dsl) struct SpecializeGuardedChoicesPass;

impl GeneratorPass for SpecializeGuardedChoicesPass {
    fn transform(
        &mut self,
        def: &GenDef,
        body: GenTerm,
        _declarations: GenDeclarations<'_>,
        _syms: &mut SymGen,
    ) -> Result<GenTerm, TransformError> {
        Ok(specialize_guarded_choices(&def.iarg_syms(), body))
    }
}

#[derive(Debug, Clone, Copy)]
pub(in crate::gen_dsl) struct SpecializeSingletonPass {
    pub(in crate::gen_dsl) search: SingletonSearch,
}

impl GeneratorPass for SpecializeSingletonPass {
    fn transform(
        &mut self,
        def: &GenDef,
        body: GenTerm,
        _declarations: GenDeclarations<'_>,
        _syms: &mut SymGen,
    ) -> Result<GenTerm, TransformError> {
        Ok(specialize_singleton(&def.iarg_syms(), body, self.search))
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(in crate::gen_dsl) struct DestructProductsPass;

impl GenModulePass for DestructProductsPass {
    fn transform(
        &mut self,
        mut module: GenModule,
        syms: &mut SymGen,
    ) -> Result<GenModule, TransformError> {
        module.defs = destruct_products(&module.defs, &module.structs, syms)?;
        Ok(module)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(in crate::gen_dsl) struct PruneReturnsPass;

impl GenModulePass for PruneReturnsPass {
    fn transform(
        &mut self,
        mut module: GenModule,
        syms: &mut SymGen,
    ) -> Result<GenModule, TransformError> {
        module.defs = prune_returns(&module.defs, &module.structs, syms)?;
        Ok(module)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(in crate::gen_dsl) struct PruneArgsPass;

impl GenModulePass for PruneArgsPass {
    fn transform(
        &mut self,
        mut module: GenModule,
        _syms: &mut SymGen,
    ) -> Result<GenModule, TransformError> {
        prune_args(&mut module.defs);
        Ok(module)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(in crate::gen_dsl) struct SizeGeneratorsPass;

impl GenModulePass for SizeGeneratorsPass {
    fn transform(
        &mut self,
        mut module: GenModule,
        syms: &mut SymGen,
    ) -> Result<GenModule, TransformError> {
        size_generators(&mut module.defs, syms);
        Ok(module)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(in crate::gen_dsl) struct HeapArgsPass;

impl GenModulePass for HeapArgsPass {
    fn transform(
        &mut self,
        mut module: GenModule,
        syms: &mut SymGen,
    ) -> Result<GenModule, TransformError> {
        heap_args(&mut module.defs, syms);
        Ok(module)
    }
}

#[derive(Debug, Clone, Copy)]
pub(in crate::gen_dsl) struct UnfoldPass {
    pub(in crate::gen_dsl) max: usize,
}

impl GenModulePass for UnfoldPass {
    fn transform(
        &mut self,
        mut module: GenModule,
        syms: &mut SymGen,
    ) -> Result<GenModule, TransformError> {
        unfold(&mut module.defs, self.max, syms);
        Ok(module)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(in crate::gen_dsl) struct SinkArgsPass;

impl GenModulePass for SinkArgsPass {
    fn transform(
        &mut self,
        mut module: GenModule,
        syms: &mut SymGen,
    ) -> Result<GenModule, TransformError> {
        sink_args(&mut module.defs, syms)?;
        Ok(module)
    }
}

#[derive(Debug, Clone, Copy)]
pub(in crate::gen_dsl) struct InlineGenPass {
    pub(in crate::gen_dsl) mode: InlineMode,
}

impl GenModulePass for InlineGenPass {
    fn transform(
        &mut self,
        mut module: GenModule,
        syms: &mut SymGen,
    ) -> Result<GenModule, TransformError> {
        inline_gen(&mut module.defs, self.mode, syms)?;
        Ok(module)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(in crate::gen_dsl) struct NormalizeScopesPass;

impl GenModulePass for NormalizeScopesPass {
    fn transform(
        &mut self,
        mut module: GenModule,
        syms: &mut SymGen,
    ) -> Result<GenModule, TransformError> {
        normalize_scopes(&mut module.defs, syms);
        Ok(module)
    }
}

#[derive(Debug, Clone, Copy)]
pub(in crate::gen_dsl) struct SimplifyGenPass {
    pub(in crate::gen_dsl) passes: PassProfile,
    pub(in crate::gen_dsl) options: PassOptions,
    pub(in crate::gen_dsl) stage: &'static str,
    pub(in crate::gen_dsl) push: bool,
}

impl GenModulePass for SimplifyGenPass {
    fn transform(
        &mut self,
        mut module: GenModule,
        syms: &mut SymGen,
    ) -> Result<GenModule, TransformError> {
        let effects = ChoiceEffects::analyze(&module.defs);
        let mut defs = std::mem::take(&mut module.defs);
        for (_, def) in &mut defs {
            let name = def.name.clone();
            let body = std::mem::replace(&mut def.body, GenTerm::return_(IndexTerm::unit()));
            let mut push_budget = if self.push { MAX_PUSH_GROWTH } else { 0 };
            def.body = simplify_gen_with_push_budget(
                body,
                &module,
                syms,
                self.passes,
                self.options,
                &effects,
                &mut push_budget,
                &mut |pass, iteration, changed| {
                    if changed {
                        tracing::debug!(generator = ?name, which = self.stage, pass, iteration, "simplification rewrote");
                    } else {
                        tracing::trace!(generator = ?name, which = self.stage, pass, iteration, "pass was identity");
                    }
                },
            );
        }
        module.defs = defs;
        Ok(module)
    }
}

#[cfg(feature = "smt")]
#[derive(Debug, Clone, Copy, Default)]
pub(in crate::gen_dsl) struct SmtPruningPass;

#[cfg(feature = "smt")]
impl GeneratorPass for SmtPruningPass {
    fn transform(
        &mut self,
        def: &GenDef,
        body: GenTerm,
        _declarations: GenDeclarations<'_>,
        _syms: &mut SymGen,
    ) -> Result<GenTerm, TransformError> {
        Ok(super::prune_smt(&def.iargs, body))
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(in crate::gen_dsl) struct AnnotatePointeesPass;

impl GenModulePass for AnnotatePointeesPass {
    fn transform(
        &mut self,
        mut module: GenModule,
        _syms: &mut SymGen,
    ) -> Result<GenModule, TransformError> {
        annotate_pointees(&mut module.defs, &module.structs);
        Ok(module)
    }
}
