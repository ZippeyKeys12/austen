use std::collections::BTreeMap;
use std::sync::Arc;

use crate::gen_dsl::passes::PassProfile;
use crate::gen_dsl::{
    Entropy, EntropyBacked, GenModule, GenModuleError, Incoming, RejectingProducer, Sym,
    TreeProducer,
};

#[cfg(feature = "driver")]
mod campaign;
#[cfg(feature = "driver")]
mod chain;
mod custom;
#[cfg(feature = "driver")]
mod spool;
#[cfg(feature = "driver")]
pub mod target;

#[cfg(feature = "driver")]
#[doc(hidden)]
pub use campaign::campaign_with_generator_factory;
#[cfg(feature = "driver")]
pub use chain::{Chain, ChainError, intern_name};
pub use custom::{CustomProducer, CustomSelection};

pub const DEFAULT_MAX_ATTEMPTS: u32 = 64;

#[derive(Debug, Clone, Default)]
pub struct ProductionModules {
    profiles: BTreeMap<PassProfile, Arc<GenModule>>,
    custom: BTreeMap<u8, CustomProducer>,
}

impl ProductionModules {
    pub fn single(profile: PassProfile, module: Arc<GenModule>) -> Self {
        Self {
            profiles: BTreeMap::from([(profile, module)]),
            custom: BTreeMap::new(),
        }
    }

    pub fn insert(
        &mut self,
        profile: PassProfile,
        module: Arc<GenModule>,
    ) -> Option<Arc<GenModule>> {
        self.profiles.insert(profile, module)
    }

    pub fn for_profile(&self, profile: &PassProfile) -> Option<&Arc<GenModule>> {
        self.profiles.get(profile)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&PassProfile, &Arc<GenModule>)> {
        self.profiles.iter()
    }

    pub fn insert_custom(
        &mut self,
        target_id: u8,
        producer: CustomProducer,
    ) -> Option<CustomProducer> {
        self.custom.insert(target_id, producer)
    }

    pub fn custom_for(&self, target_id: u8) -> Option<&CustomProducer> {
        self.custom.get(&target_id)
    }

    pub fn has_custom(&self) -> bool {
        !self.custom.is_empty()
    }

    #[cfg(feature = "driver")]
    pub fn trust(&self) -> crate::wire::Trust {
        if self.has_custom() {
            crate::wire::Trust::Checked
        } else {
            crate::wire::Trust::Producer
        }
    }

    pub fn custom_iter(&self) -> impl Iterator<Item = (&u8, &CustomProducer)> {
        self.custom.iter()
    }

    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }
}

impl FromIterator<(PassProfile, Arc<GenModule>)> for ProductionModules {
    fn from_iter<I: IntoIterator<Item = (PassProfile, Arc<GenModule>)>>(iter: I) -> Self {
        Self {
            profiles: iter.into_iter().collect(),
            custom: BTreeMap::new(),
        }
    }
}

pub struct EngineBuild {
    pub production_modules: ProductionModules,
    pub wire_module: Arc<GenModule>,
    pub incoming: Incoming,
    pub max_attempts: u32,
    pub max_array_length: usize,
    pub generator_timeout: Option<std::time::Duration>,
    pub entropy: Entropy,
    pub custom: Option<CustomSelection>,
}

impl EngineBuild {
    fn validate(&self) -> Result<(), EngineOpenError> {
        self.wire_module.validate().map_err(EngineOpenError::Wire)?;
        for (_, production_module) in self.production_modules.iter() {
            production_module
                .validate()
                .map_err(EngineOpenError::Production)?;
            validate_module_fork(&self.wire_module, production_module)?;
        }
        Ok(())
    }

    pub fn production_module(
        &self,
        engine: &'static str,
        profile: PassProfile,
    ) -> Result<Arc<GenModule>, EngineOpenError> {
        if let Some(custom) = &self.custom {
            return Ok(Arc::clone(&custom.module));
        }
        self.production_modules
            .for_profile(&profile)
            .cloned()
            .ok_or(EngineOpenError::MissingProductionModule { engine, profile })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EngineOpenError {
    #[error("custom generator {entry:?} from {file}: {reason}")]
    Custom {
        file: std::path::PathBuf,
        entry: Sym,
        reason: String,
    },
    #[error("invalid production generator module: {0}")]
    Production(#[source] GenModuleError),
    #[error("invalid wire generator module: {0}")]
    Wire(#[source] GenModuleError),
    #[error("wire and production definitions disagree for generator {generator:?}")]
    DefinitionMismatch { generator: Sym },
    #[error(
        "function generator {generator:?} has non-global input {argument:?}; \
         an Austen-driven tree campaign cannot construct its root call"
    )]
    UnboundFunctionArgument { generator: Sym, argument: Sym },
    #[error(
        "function generator {generator:?} has no test-case size argument; \
         Austen-driven tree campaigns require the sized generator form"
    )]
    MissingTestCaseSize { generator: Sym },
    #[error("function generator {generator:?} has invalid test-case size argument {argument:?}")]
    InvalidTestCaseSizeArgument { generator: Sym, argument: Sym },
    #[error(
        "wire and production generator modules disagree on definition order or membership: \
         production must be the wire table with unreachable auxiliary definitions dropped, in the \
         wire's order and holding every function generator, followed by appended auxiliaries"
    )]
    DefinitionTableMismatch,
    #[error("wire and production generator modules disagree on their declaration tables")]
    DeclarationMismatch,
    #[error(
        "engine {engine} has no production module staged under its pass profile {profile:?}; \
         the campaign staged the profiles of a different engine"
    )]
    MissingProductionModule {
        engine: &'static str,
        profile: PassProfile,
    },
}

pub(crate) fn validate_module_fork(
    wire: &GenModule,
    production: &GenModule,
) -> Result<(), EngineOpenError> {
    if wire.structs != production.structs
        || wire.functions != production.functions
        || wire.datatypes != production.datatypes
        || wire.globals != production.globals
    {
        return Err(EngineOpenError::DeclarationMismatch);
    }
    let mut wire_defs = wire.defs.iter().peekable();
    for (production_key, production_def) in &production.defs {
        let matched = loop {
            match wire_defs.peek().copied() {
                Some((wire_key, wire_def)) if same_key(wire_key, production_key) => {
                    break Some(wire_def);
                }
                Some((_, wire_def)) if !wire_def.spec => {
                    wire_defs.next();
                }
                _ => break None,
            }
        };
        match matched {
            Some(wire_def) => {
                wire_defs.next();
                if !wire_def.same_signature(production_def) {
                    return Err(EngineOpenError::DefinitionMismatch {
                        generator: production_key.clone(),
                    });
                }
            }
            None => {
                if wire_defs.any(|(_, wire_def)| wire_def.spec) {
                    return Err(EngineOpenError::DefinitionTableMismatch);
                }
                let mut appended = production
                    .defs
                    .iter()
                    .skip_while(|(key, _)| !same_key(key, production_key));
                if appended.any(|(key, def)| {
                    def.spec
                        || wire
                            .defs
                            .iter()
                            .any(|(wire_key, _)| same_key(wire_key, key))
                }) {
                    return Err(EngineOpenError::DefinitionTableMismatch);
                }
                return Ok(());
            }
        }
    }
    if wire_defs.any(|(_, wire_def)| wire_def.spec) {
        return Err(EngineOpenError::DefinitionTableMismatch);
    }
    Ok(())
}

fn same_key(a: &Sym, b: &Sym) -> bool {
    a.num == b.num && a.name == b.name
}

#[cfg(feature = "driver")]
pub(crate) fn validate_generator_pair(
    generator: &Sym,
    wire: &GenModule,
    production: &GenModule,
) -> Result<(), EngineOpenError> {
    production
        .function_generator(generator)
        .map_err(EngineOpenError::Production)?;
    wire.function_generator(generator)
        .map_err(EngineOpenError::Wire)?;
    Ok(())
}

pub trait TreeEngineFactory {
    fn name(&self) -> &'static str;

    fn display_name(&self) -> &'static str;

    fn passes(&self) -> PassProfile;

    fn open(&self, build: EngineBuild) -> Result<Box<dyn TreeEngine>, EngineOpenError> {
        build.validate()?;
        self.open_validated(build)
    }

    fn open_validated(&self, build: EngineBuild) -> Result<Box<dyn TreeEngine>, EngineOpenError>;
}

#[derive(Clone, Copy)]
pub struct AnyTreeEngine(&'static (dyn TreeEngineFactory + Sync));

impl AnyTreeEngine {
    pub const fn new(factory: &'static (dyn TreeEngineFactory + Sync)) -> Self {
        Self(factory)
    }
}

impl std::fmt::Debug for AnyTreeEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("AnyTreeEngine")
            .field(&TreeEngineFactory::name(self))
            .finish()
    }
}

impl PartialEq for AnyTreeEngine {
    fn eq(&self, other: &Self) -> bool {
        TreeEngineFactory::name(self) == TreeEngineFactory::name(other)
    }
}
impl Eq for AnyTreeEngine {}

impl TreeEngineFactory for AnyTreeEngine {
    fn name(&self) -> &'static str {
        self.0.name()
    }
    fn display_name(&self) -> &'static str {
        self.0.display_name()
    }
    fn passes(&self) -> PassProfile {
        self.0.passes()
    }
    fn open(&self, build: EngineBuild) -> Result<Box<dyn TreeEngine>, EngineOpenError> {
        self.0.open(build)
    }
    fn open_validated(&self, build: EngineBuild) -> Result<Box<dyn TreeEngine>, EngineOpenError> {
        self.0.open_validated(build)
    }
}

pub trait TreeEngine: TreeProducer + RejectingProducer + EntropyBacked {}

impl<T> TreeEngine for T where T: TreeProducer + RejectingProducer + EntropyBacked {}
