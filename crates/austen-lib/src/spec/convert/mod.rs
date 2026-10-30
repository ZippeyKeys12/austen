use std::collections::{BTreeMap, BTreeSet};

pub use crate::gen_dsl::OBSERVED_STAGES;
use crate::gen_dsl::passes::{PassOptions, PassProfile, TransformError};
use crate::gen_dsl::{
    BaseType, FunctionTableError, GenDef, GenModule, GenModuleError, GlobalDef, Id, ScopeError,
    Sctype, Sign, StructDef, Sym, SymGen, SymMap, SymSet, well_formed,
};
use crate::spec::{CType, FunctionSpec, GlobalLinkage, GlobalSpec, SpecModule};

pub mod ctx;
pub mod def;
pub mod lat;
pub(in crate::spec) mod logical_functions;
pub mod post;
pub mod sym_gen;

use ctx::Builder;
use def::{
    predicate_global_dependencies, recursive_predicates, spec_arg_type, transform_pred,
    transform_spec,
};
use logical_functions::{close_logical_function_globals, needs_global_closure};
use post::transform_post;

use crate::spec::Post;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConvertError {
    #[error("a specification declares no functions")]
    NoFunctions,
    #[error("a specification declares {count} functions; at most {max} fit in the selector byte")]
    TooManyFunctions { count: usize, max: usize },
    #[error("function {name} is declared more than once")]
    DuplicateFunction { name: String },
    #[error("global address {address:?} is declared more than once")]
    DuplicateGlobalAddress { address: Sym },
    #[error("global link name {name} is declared more than once")]
    DuplicateGlobalName { name: String },
    #[error("internal global {name} with owner {owner} is declared more than once")]
    DuplicateInternalGlobal { name: String, owner: String },
    #[error("global {address:?} has an empty C name")]
    EmptyGlobalName { address: Sym },
    #[error("internal global {name} has an empty owner link symbol")]
    EmptyGlobalOwner { name: String },
    #[error("global {name} is const- or volatile-qualified and is unsupported")]
    ImmutableGlobal { name: String },
    #[error("unsupported type for global {name}: {ty:?}")]
    UnsupportedGlobalType { name: String, ty: Sctype },
    #[error("unsupported parameter type: {0:?}")]
    UnsupportedParam(CType),
    #[error("unknown predicate {name:?}")]
    UnknownPredicate { name: Sym },
    #[error("predicate {name:?} is uninterpreted, so it has no generator")]
    UninterpretedPredicate { name: Sym },
    #[error("predicate {name:?} has no clauses")]
    NoClauses { name: Sym },
    #[error("the last clause of predicate {name:?} is guarded")]
    GuardedLastClause { name: Sym },
    #[error("generator {name:?} is defined twice")]
    DuplicateGenerator { name: Sym },
    #[error("expected a map base type for an iterated resource, got {bt:?}")]
    NotAMapBaseType { bt: BaseType },
    #[error("clauses of {name:?} return different types: {then_:?} vs {else_:?}")]
    IteBranchMismatch {
        name: Sym,
        then_: BaseType,
        else_: BaseType,
    },
    #[error("duplicate argument-record member {id:?}")]
    DuplicateRecordMember { id: Id },
    #[error(transparent)]
    Transform(#[from] TransformError),
    #[error("invalid generator module: {error}")]
    InvalidGeneratorModule { error: GenModuleError },
    #[error("unsupported type for field {field} of {ty}")]
    UnsupportedField { ty: String, field: String },
    #[error("no function named {name}")]
    UnknownFunction { name: String },
    #[error("no function at target id {target_id}; the specification declares {count} functions")]
    UnknownTargetId { target_id: u8, count: usize },
    #[error("generator {name:?} is not well-scoped: {error}")]
    NotWellScoped { name: Sym, error: ScopeError },
    #[error("logical-function table is not well formed: {error}")]
    MalformedFunctionTable { error: FunctionTableError },
}

pub struct Cx<'a> {
    pub module: &'a SpecModule,
    pub global_iargs: Vec<(Sym, BaseType)>,
    pub predicate_globals: SymMap<SymSet>,
    pub recursive: SymSet,
    pub structs: Vec<(Sym, StructDef)>,
    pub syms: SymGen,
    pub builder: Builder,
}

impl Cx<'_> {
    pub fn request_predicate(&mut self, fsym: &Sym) -> Result<(), ConvertError> {
        if self.module.predicate(fsym).is_none() {
            return Err(ConvertError::UnknownPredicate { name: fsym.clone() });
        }
        self.builder.request(fsym);
        Ok(())
    }

    pub fn globals_in(&self, used: &SymSet) -> Vec<(Sym, BaseType)> {
        self.global_iargs
            .iter()
            .filter(|(sym, _)| used.contains(sym))
            .cloned()
            .collect()
    }

    pub fn predicate_global_iargs(&self, name: &Sym) -> Vec<(Sym, BaseType)> {
        self.predicate_globals
            .get(name)
            .map_or_else(Vec::new, |used| self.globals_in(used))
    }

    pub fn global_symbols(&self) -> SymSet {
        self.global_iargs
            .iter()
            .map(|(sym, _)| sym.clone())
            .collect()
    }
}

pub fn param_base_type(ct: &CType) -> Option<BaseType> {
    match ct {
        CType::Int { bits, signed } => Some(BaseType::Bits(
            if *signed {
                Sign::Signed
            } else {
                Sign::Unsigned
            },
            u16::from(*bits),
        )),
        CType::Bool => Some(BaseType::Bool),
        CType::Pointer { .. } => Some(BaseType::Loc(None)),
        CType::Array { .. } | CType::Named { .. } | CType::Void => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversion {
    pub wire: GenModule,
    pub production: GenModule,
    pub functions: Vec<ConvertedFunction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedConversion {
    pub conversion: Conversion,
    pub spec: SpecModule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertedFunction {
    pub target_id: u8,
    pub name: String,
    pub generator: Sym,
    pub post: crate::spec::Post,
    pub post_defaults: crate::spec::DefaultTypes,
}

pub const MAX_FUNCTIONS: usize = u8::MAX as usize + 1;

pub fn validate_function_table(module: &SpecModule) -> Result<(), ConvertError> {
    if module.functions.is_empty() {
        return Err(ConvertError::NoFunctions);
    }
    if module.functions.len() > MAX_FUNCTIONS {
        return Err(ConvertError::TooManyFunctions {
            count: module.functions.len(),
            max: MAX_FUNCTIONS,
        });
    }
    let mut function_names = BTreeSet::new();
    for function in &module.functions {
        if !function_names.insert(function.name.clone()) {
            return Err(ConvertError::DuplicateFunction {
                name: function.name.clone(),
            });
        }
    }
    Ok(())
}

pub fn validate_global_table(module: &SpecModule) -> Result<(), ConvertError> {
    let mut syms = sym_gen::seeded(module);
    let structs = struct_table(module, &mut syms)?;
    validate_global_table_in(module, &structs)
}

pub fn global_size(module: &SpecModule, global: &GlobalSpec) -> Result<u64, ConvertError> {
    let mut syms = sym_gen::seeded(module);
    let structs = struct_table(module, &mut syms)?;
    validate_global_in(global, &structs)
}

pub fn global_context(module: &SpecModule) -> Result<GenModule, ConvertError> {
    let mut syms = sym_gen::seeded(module);
    let structs = struct_table(module, &mut syms)?;
    validate_global_table_in(module, &structs)?;
    let globals = module
        .globals
        .iter()
        .map(|global| (global.address.clone(), global_def(global)))
        .collect();
    Ok(GenModule {
        structs,
        globals,
        ..GenModule::default()
    })
}

pub(in crate::spec) fn close_over_callees(
    mut dependencies: SymMap<SymSet>,
    callees: &SymMap<SymSet>,
    mut narrow: impl FnMut(&Sym, &mut SymSet),
) -> SymMap<SymSet> {
    loop {
        let previous = dependencies.clone();
        let mut changed = false;
        for (name, used) in &mut dependencies {
            let old_len = used.len();
            for callee in callees.get(name).into_iter().flatten() {
                if let Some(callee_globals) = previous.get(callee) {
                    used.extend(callee_globals.iter().cloned());
                }
            }
            narrow(name, used);
            changed |= used.len() != old_len;
        }
        if !changed {
            return dependencies;
        }
    }
}

fn global_def(global: &GlobalSpec) -> GlobalDef {
    GlobalDef {
        c_name: global.c_name.clone(),
        ty: global.ty.clone(),
        linkage: global.linkage.clone(),
        qualifiers: global.qualifiers,
    }
}

fn validate_global_table_in(
    module: &SpecModule,
    structs: &[(Sym, StructDef)],
) -> Result<(), ConvertError> {
    let mut addresses = BTreeSet::new();
    let mut link_names = BTreeSet::new();
    let mut internal_objects = BTreeSet::new();
    for global in &module.globals {
        validate_global_in(global, structs)?;
        if !addresses.insert(global.address.clone()) {
            return Err(ConvertError::DuplicateGlobalAddress {
                address: global.address.clone(),
            });
        }
        let link_name = global.link_name();
        if !link_names.insert(link_name.clone()) {
            return Err(ConvertError::DuplicateGlobalName { name: link_name });
        }
        if let GlobalLinkage::Internal { owner } = &global.linkage
            && !internal_objects.insert((owner.clone(), global.c_name.clone()))
        {
            return Err(ConvertError::DuplicateInternalGlobal {
                name: global.c_name.clone(),
                owner: owner.clone(),
            });
        }
    }
    Ok(())
}

fn validate_global_in(
    global: &GlobalSpec,
    structs: &[(Sym, StructDef)],
) -> Result<u64, ConvertError> {
    if global.c_name.is_empty() {
        return Err(ConvertError::EmptyGlobalName {
            address: global.address.clone(),
        });
    }
    if let GlobalLinkage::Internal { owner } = &global.linkage
        && owner.is_empty()
    {
        return Err(ConvertError::EmptyGlobalOwner {
            name: global.c_name.clone(),
        });
    }
    if global.qualifiers.const_ || global.qualifiers.volatile {
        return Err(ConvertError::ImmutableGlobal {
            name: global.c_name.clone(),
        });
    }
    match global.ty.byte_size_in(structs) {
        Some(size) if ordinary_object_type(&global.ty) && size > 0 => Ok(size),
        _ => Err(ConvertError::UnsupportedGlobalType {
            name: global.c_name.clone(),
            ty: global.ty.clone(),
        }),
    }
}

fn ordinary_object_type(ty: &Sctype) -> bool {
    match ty {
        Sctype::Integer(_) | Sctype::Pointer(_) | Sctype::Struct(_) => true,
        Sctype::Array(element, Some(_)) => ordinary_object_type(element),
        Sctype::Array(_, None) | Sctype::Void | Sctype::Function { .. } | Sctype::Byte => false,
    }
}

pub fn transform(module: &SpecModule, passes: PassProfile) -> Result<Conversion, ConvertError> {
    transform_with_options(module, passes, PassOptions::NONE)
}

pub fn transform_with_options(
    module: &SpecModule,
    passes: PassProfile,
    options: PassOptions,
) -> Result<Conversion, ConvertError> {
    transform_observed_with_options(module, passes, options, &mut |_, _, _| {})
}

pub fn transform_observed(
    module: &SpecModule,
    passes: PassProfile,
    observe: &mut dyn FnMut(&'static str, &Sym, &GenDef),
) -> Result<Conversion, ConvertError> {
    transform_observed_with_options(module, passes, PassOptions::NONE, observe)
}

pub fn transform_observed_with_options(
    module: &SpecModule,
    passes: PassProfile,
    options: PassOptions,
    observe: &mut dyn FnMut(&'static str, &Sym, &GenDef),
) -> Result<Conversion, ConvertError> {
    validate_function_table(module)?;

    let closed;
    let module = if needs_global_closure(module) {
        closed = {
            let mut owned = module.clone();
            close_logical_function_globals(&mut owned);
            owned
        };
        &closed
    } else {
        module
    };

    transform_prepared_observed_with_options(module, None, passes, options, observe)
}

pub fn transform_staged_observed_with_options(
    mut module: SpecModule,
    selected_target_id: Option<u8>,
    passes: PassProfile,
    options: PassOptions,
    observe: &mut dyn FnMut(&'static str, &Sym, &GenDef),
) -> Result<StagedConversion, ConvertError> {
    validate_function_table(&module)?;
    close_logical_function_globals(&mut module);

    let conversion = if let Some(target_id) = selected_target_id {
        let count = module.functions.len();
        let function = module
            .functions
            .get(usize::from(target_id))
            .cloned()
            .ok_or(ConvertError::UnknownTargetId { target_id, count })?;
        let functions = std::mem::replace(&mut module.functions, vec![function]);
        let converted = transform_prepared_observed_with_options(
            &module,
            Some(target_id),
            passes,
            options,
            observe,
        );
        module.functions = functions;
        converted?
    } else {
        transform_prepared_observed_with_options(&module, None, passes, options, observe)?
    };

    Ok(StagedConversion {
        conversion,
        spec: module,
    })
}

fn transform_prepared_observed_with_options(
    module: &SpecModule,
    selected_target_id: Option<u8>,
    passes: PassProfile,
    options: PassOptions,
    observe: &mut dyn FnMut(&'static str, &Sym, &GenDef),
) -> Result<Conversion, ConvertError> {
    let mut syms = sym_gen::seeded(module);
    let structs = struct_table(module, &mut syms)?;
    validate_global_table_in(module, &structs)?;
    let global_iargs: Vec<_> = module
        .globals
        .iter()
        .map(|global| (global.address.clone(), BaseType::loc_to(global.ty.clone())))
        .collect();
    let predicate_globals = predicate_global_dependencies(module);
    let globals: Vec<_> = module
        .globals
        .iter()
        .map(|global| (global.address.clone(), global_def(global)))
        .collect();
    let names: Vec<Sym> = module
        .functions
        .iter()
        .map(|func| syms.fresh(&func.name))
        .collect();

    let mut cx = Cx {
        module,
        global_iargs,
        predicate_globals,
        recursive: recursive_predicates(module),
        structs,
        syms,
        builder: Builder::new(),
    };
    let mut converted_functions = Vec::with_capacity(module.functions.len());

    for (target_id, (func, name)) in module.functions.iter().zip(names).enumerate() {
        let at = spec_arg_type(func)?;
        let post = transform_post(module, &at, &func.ensures);
        let (post_defaults, unsupported_default) =
            post::collect_post_defaults_with_unsupported(module, &func.ensures);
        let post = match (post, unsupported_default) {
            (Post::Check, Some(error)) => Post::Unsupported {
                reason: format!("ensures reaches a default the input cannot draw: {error}"),
            },
            (post, _) => post,
        };
        converted_functions.push(ConvertedFunction {
            target_id: selected_target_id.unwrap_or(target_id as u8),
            name: func.name.clone(),
            generator: name.clone(),
            post,
            post_defaults,
        });
        let def = transform_spec(&mut cx, name, &at)?;
        cx.builder.add(def)?;
    }
    while let Some(fsym) = cx.builder.next_request() {
        let pred = module
            .predicate(&fsym)
            .ok_or_else(|| ConvertError::UnknownPredicate { name: fsym.clone() })?;
        let def = transform_pred(&mut cx, pred)?;
        cx.builder.add(def)?;
    }

    let Cx {
        structs,
        syms,
        builder,
        ..
    } = cx;

    let functions: Vec<_> = module
        .logical_functions
        .iter()
        .map(|f| (f.name.clone(), f.clone()))
        .collect();
    let datatypes: Vec<_> = module
        .datatypes
        .iter()
        .map(|d| (d.name.clone(), d.clone()))
        .collect();
    well_formed(&functions).map_err(|error| ConvertError::MalformedFunctionTable { error })?;
    let source = GenModule {
        defs: builder
            .into_defs()
            .into_iter()
            .map(|(name, def)| {
                (
                    name,
                    crate::gen_dsl::passes::undrawable_defaults_to_unknown(def),
                )
            })
            .collect(),
        structs,
        functions,
        datatypes,
        globals,
    };
    let staged =
        crate::gen_dsl::stage_module(source, syms, passes, options, observe).map_err(|error| {
            match error {
                crate::gen_dsl::StageError::Module(error) => {
                    ConvertError::InvalidGeneratorModule { error }
                }
                crate::gen_dsl::StageError::Transform(error) => ConvertError::Transform(error),
                crate::gen_dsl::StageError::NotWellScoped { name, error } => {
                    ConvertError::NotWellScoped { name, error }
                }
            }
        })?;
    for function in &converted_functions {
        staged
            .wire
            .function_generator(&function.generator)
            .map_err(|error| ConvertError::InvalidGeneratorModule { error })?;
        staged
            .production
            .function_generator(&function.generator)
            .map_err(|error| ConvertError::InvalidGeneratorModule { error })?;
    }
    Ok(Conversion {
        wire: staged.wire,
        production: staged.production,
        functions: converted_functions,
    })
}

pub fn compile_production(spec: &FunctionSpec) -> Result<GenDef, ConvertError> {
    let module = SpecModule {
        functions: vec![spec.clone()],
        ..SpecModule::default()
    };
    let conversion = transform(&module, PassProfile::NONE)?;
    let (_, def) = conversion
        .production
        .defs
        .into_iter()
        .find(|(_, def)| def.spec)
        .expect("a converted module of one function has one specification generator");
    Ok(def)
}

fn struct_table(
    module: &SpecModule,
    syms: &mut SymGen,
) -> Result<Vec<(Sym, StructDef)>, ConvertError> {
    let mut tags: BTreeMap<String, Sym> = BTreeMap::new();
    for ty in &module.types {
        let tag = ty.tag.clone().unwrap_or_else(|| syms.fresh(&ty.name));
        tags.insert(ty.name.clone(), tag);
    }
    let lookup = |name: &str| tags.get(name).cloned();

    let mut table = Vec::with_capacity(module.types.len());
    for ty in &module.types {
        let mut members = Vec::with_capacity(ty.fields.len());
        for field in &ty.fields {
            let ct = field
                .ty
                .to_sctype(&lookup)
                .ok_or_else(|| ConvertError::UnsupportedField {
                    ty: ty.name.clone(),
                    field: field.name.clone(),
                })?;
            members.push((Id(field.name.clone()), ct));
        }
        table.push((tags[&ty.name].clone(), StructDef { members }));
    }
    Ok(table)
}
