use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use crate::gen_dsl::passes::PassProfile;

use crate::campaign::{CampaignConfig, CampaignError, CampaignReport, Engine};
use crate::engine::EngineDescriptor;
use crate::generator::{GeneratorFactory, PreparedGenerator, PreparedTree};
use crate::internal::{AnyTreeEngine, EngineOpenError, TreeEngineFactory};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chain {
    members: Vec<AnyTreeEngine>,
    name: &'static str,
    display_name: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChainError {
    #[error("an engine chain needs at least two members; {members} given")]
    TooFew { members: usize },
    #[error("engine {engine} appears more than once in the chain")]
    Duplicate { engine: &'static str },
    #[error(
        "unknown chain member `{name}`: a chain escalates tree engines only, one of {expected}"
    )]
    UnknownMember { name: String, expected: String },
}

impl Chain {
    pub const SEPARATOR: char = '/';

    pub fn new(members: Vec<AnyTreeEngine>) -> Result<Chain, ChainError> {
        if members.len() < 2 {
            return Err(ChainError::TooFew {
                members: members.len(),
            });
        }
        let mut seen = Vec::with_capacity(members.len());
        for member in &members {
            if seen.contains(member) {
                return Err(ChainError::Duplicate {
                    engine: TreeEngineFactory::name(member),
                });
            }
            seen.push(*member);
        }
        let name = intern_name(
            &members
                .iter()
                .map(TreeEngineFactory::name)
                .collect::<Vec<_>>()
                .join(&Self::SEPARATOR.to_string()),
        );
        let display_name = intern_name(
            &members
                .iter()
                .map(TreeEngineFactory::display_name)
                .collect::<Vec<_>>()
                .join(" / "),
        );
        Ok(Chain {
            members,
            name,
            display_name,
        })
    }

    pub fn resolve(
        name: &str,
        registry: &crate::engine::EngineRegistry,
    ) -> Result<Chain, ChainError> {
        let members = name
            .split(Self::SEPARATOR)
            .map(|member| {
                registry
                    .resolve_tree(member)
                    .ok_or_else(|| ChainError::UnknownMember {
                        name: member.to_string(),
                        expected: registry.tree_names().collect::<Vec<_>>().join(", "),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(members)
    }

    pub fn members(&self) -> &[AnyTreeEngine] {
        &self.members
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn display_name(&self) -> &'static str {
        self.display_name
    }

    pub fn profiles(&self) -> Vec<PassProfile> {
        let mut profiles = Vec::new();
        for member in &self.members {
            let profile = TreeEngineFactory::passes(member);
            if !profiles.contains(&profile) {
                profiles.push(profile);
            }
        }
        profiles
    }

    pub fn descriptor(&self) -> EngineDescriptor {
        crate::engine::TREE_ENGINE_DESCRIPTOR
    }

    pub fn selection_label(&self) -> String {
        format!("{} (--engine={})", self.display_name, self.name)
    }
}

impl std::fmt::Display for Chain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name)
    }
}

pub fn intern_name(name: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Default::default)
        .lock()
        .expect("an intern-table holder cannot panic mid-insert");
    if let Some(existing) = names.get(name) {
        return existing;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    names.insert(leaked);
    leaked
}

impl GeneratorFactory for Chain {
    fn descriptor(&self) -> crate::engine::EngineDescriptor {
        Chain::descriptor(self)
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn display_name(&self) -> &'static str {
        self.display_name
    }

    fn profiles(&self) -> Vec<PassProfile> {
        Chain::profiles(self)
    }

    fn prepare(
        &self,
        cfg: &CampaignConfig,
        _journal: &mut crate::coverage::Journal,
    ) -> Result<Box<dyn PreparedGenerator>, CampaignError> {
        for member in &self.members {
            let profile = TreeEngineFactory::passes(member);
            if cfg.production_modules.for_profile(&profile).is_none() {
                return Err(EngineOpenError::MissingProductionModule {
                    engine: TreeEngineFactory::name(member),
                    profile,
                }
                .into());
            }
        }
        Ok(Box::new(PreparedTree {
            trust: cfg.production_modules.trust(),
            members: self.members.clone(),
            name: self.name,
        }))
    }
}

impl Engine for Chain {
    fn name(&self) -> &'static str {
        self.name
    }

    fn display_name(&self) -> &'static str {
        self.display_name
    }

    fn profiles(&self) -> Vec<PassProfile> {
        Chain::profiles(self)
    }

    fn campaign(&self, cfg: CampaignConfig) -> Result<CampaignReport, CampaignError> {
        crate::internal::campaign_with_generator_factory(self, cfg)
    }
}
