use std::num::NonZeroU64;
use std::sync::Arc;

use crate::gen_dsl::{Entropy, GenModule, Incoming, TestCaseSize};
use crate::wire::Trust;

use crate::campaign::{CampaignConfig, CampaignError, CampaignFunction};
use crate::internal::{AnyTreeEngine, DEFAULT_MAX_ATTEMPTS, ProductionModules, TreeEngineFactory};

pub use crate::input::{
    GenerateCx, GeneratePoll, Generator, GeneratorError, Rejections, StopReason, TreeGenerator,
};

pub struct GeneratorBuild {
    pub functions: Vec<CampaignFunction>,
    pub wire_module: GenModule,
    pub production_modules: ProductionModules,
    pub incoming: Incoming,
    pub max_array_length: usize,
    pub generator_timeout: Option<std::time::Duration>,
    pub chain_discard_limit: Option<NonZeroU64>,
    pub function_selector: crate::FunctionSelectorKind,
    pub size_selector: crate::SizeSelectorKind,
    pub max_test_case_size: TestCaseSize,
    pub entropy: Entropy,
}

pub trait GeneratorFactory {
    fn name(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn descriptor(&self) -> crate::engine::EngineDescriptor;
    fn profiles(&self) -> Vec<crate::gen_dsl::passes::PassProfile>;

    fn prepare(
        &self,
        cfg: &CampaignConfig,
        journal: &mut crate::coverage::Journal,
    ) -> Result<Box<dyn PreparedGenerator>, CampaignError>;
}

pub trait PreparedGenerator {
    fn name(&self) -> &'static str;
    fn trust(&self) -> Trust;
    fn open(self: Box<Self>, build: GeneratorBuild) -> Result<Box<dyn Generator>, CampaignError>;
}

#[derive(Clone, Copy)]
pub struct GeneratorFactoryHandle(&'static (dyn GeneratorFactory + Sync));

impl GeneratorFactoryHandle {
    pub const fn new(factory: &'static (dyn GeneratorFactory + Sync)) -> Self {
        Self(factory)
    }
}
impl std::fmt::Debug for GeneratorFactoryHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("GeneratorFactoryHandle")
            .field(&self.name())
            .finish()
    }
}
impl PartialEq for GeneratorFactoryHandle {
    fn eq(&self, other: &Self) -> bool {
        self.name() == other.name()
    }
}
impl Eq for GeneratorFactoryHandle {}
impl GeneratorFactory for GeneratorFactoryHandle {
    fn name(&self) -> &'static str {
        self.0.name()
    }
    fn display_name(&self) -> &'static str {
        self.0.display_name()
    }
    fn descriptor(&self) -> crate::engine::EngineDescriptor {
        self.0.descriptor()
    }
    fn profiles(&self) -> Vec<crate::gen_dsl::passes::PassProfile> {
        self.0.profiles()
    }
    fn prepare(
        &self,
        cfg: &CampaignConfig,
        journal: &mut crate::coverage::Journal,
    ) -> Result<Box<dyn PreparedGenerator>, CampaignError> {
        self.0.prepare(cfg, journal)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnyGenerator {
    Tree(AnyTreeEngine),
    Other(GeneratorFactoryHandle),
}

impl AnyGenerator {
    fn factory(&self) -> &dyn GeneratorFactory {
        match self {
            Self::Tree(factory) => factory,
            Self::Other(factory) => factory,
        }
    }
}
impl GeneratorFactory for AnyGenerator {
    fn name(&self) -> &'static str {
        self.factory().name()
    }
    fn display_name(&self) -> &'static str {
        self.factory().display_name()
    }
    fn descriptor(&self) -> crate::engine::EngineDescriptor {
        self.factory().descriptor()
    }
    fn profiles(&self) -> Vec<crate::gen_dsl::passes::PassProfile> {
        self.factory().profiles()
    }
    fn prepare(
        &self,
        cfg: &CampaignConfig,
        journal: &mut crate::coverage::Journal,
    ) -> Result<Box<dyn PreparedGenerator>, CampaignError> {
        self.factory().prepare(cfg, journal)
    }
}

pub(crate) struct PreparedTree {
    pub(crate) members: Vec<AnyTreeEngine>,
    pub(crate) name: &'static str,
    pub(crate) trust: Trust,
}

impl PreparedGenerator for PreparedTree {
    fn name(&self) -> &'static str {
        self.name
    }

    fn trust(&self) -> Trust {
        self.trust
    }

    fn open(self: Box<Self>, build: GeneratorBuild) -> Result<Box<dyn Generator>, CampaignError> {
        let generator = TreeGenerator::open_members(
            &self.members,
            self.name,
            build.function_selector.open(),
            build.size_selector.open(build.max_test_case_size),
            Box::new(crate::input::EscalationMemberSelector::with_discard_limit(
                build.chain_discard_limit,
            )),
            build.functions,
            Arc::new(build.wire_module),
            build.production_modules,
            build.incoming,
            DEFAULT_MAX_ATTEMPTS,
            build.max_array_length,
            build.generator_timeout,
            build.entropy,
        )?;
        Ok(Box::new(generator))
    }
}

impl GeneratorFactory for AnyTreeEngine {
    fn descriptor(&self) -> crate::engine::EngineDescriptor {
        crate::engine::TREE_ENGINE_DESCRIPTOR
    }

    fn name(&self) -> &'static str {
        TreeEngineFactory::name(self)
    }

    fn display_name(&self) -> &'static str {
        TreeEngineFactory::display_name(self)
    }

    fn profiles(&self) -> Vec<crate::gen_dsl::passes::PassProfile> {
        vec![TreeEngineFactory::passes(self)]
    }

    fn prepare(
        &self,
        cfg: &CampaignConfig,
        _journal: &mut crate::coverage::Journal,
    ) -> Result<Box<dyn PreparedGenerator>, CampaignError> {
        Ok(Box::new(PreparedTree {
            trust: cfg.production_modules.trust(),
            members: vec![*self],
            name: TreeEngineFactory::name(self),
        }))
    }
}
