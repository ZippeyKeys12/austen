use std::fmt;
use std::num::NonZeroU64;

use crate::gen_dsl::passes::PassProfile;

use crate::campaign::{CampaignConfig, CampaignError, CampaignReport, Engine};
use crate::engine::{
    AnyEngine, CoveragePolicy, EngineDescriptor, FunctionSelectionOwner, LoopOwner,
};
use crate::internal::{Chain, ChainError, TreeEngineFactory};
use crate::{AnyDelegatedEngine, AnyGenerator};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    stages: Vec<Stage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    pub engine: StageEngine,
    pub budget: StageBudget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageEngine {
    Single(AnyEngine),
    Chain(Chain),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageBudget {
    Seconds(NonZeroU64),
    Unstated,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
    #[error(
        "stage {stage}: empty engine name (the grammar is engine[/engine...][:seconds][,stage...])"
    )]
    Empty { stage: usize },
    #[error("stage {stage}: unknown engine `{name}`: expected one of {expected}")]
    UnknownEngine {
        stage: usize,
        name: String,
        expected: String,
    },
    #[error(
        "engine `libfuzzer-fork` was replaced by a mode option: use --engine=libfuzzer --libfuzzer-mode=fork"
    )]
    LibFuzzerFork,
    #[error("stage {stage}: budget `{text}` is not a whole number of seconds")]
    BadBudget { stage: usize, text: String },
    #[error(
        "stage {stage}: a stage budget must be positive; omit `:seconds` on the last stage to run \
         it until interrupted"
    )]
    ZeroBudget { stage: usize },
    #[error("stage {stage} ({engine}) is not the last stage, so it needs a `:seconds` budget")]
    MissingBudget { stage: usize, engine: String },
    #[error(
        "stage {stage}: `{name}` cannot be a chain member; a chain escalates tree engines only \
         ({expected})"
    )]
    NonTreeChainMember {
        stage: usize,
        name: String,
        expected: String,
    },
    #[error("stage {stage}: {source}")]
    Chain {
        stage: usize,
        #[source]
        source: ChainError,
    },
    #[error(
        "stages {first} and {second} both delegate to a tool; a schedule runs at most one \
         delegated engine (afl++, honggfuzz, libfuzzer)"
    )]
    SecondDelegated { first: usize, second: usize },
}

impl Schedule {
    pub const SEPARATOR: char = ',';
    pub const BUDGET: char = ':';

    pub fn single(engine: AnyEngine) -> Schedule {
        Schedule {
            stages: vec![Stage {
                engine: StageEngine::Single(engine),
                budget: StageBudget::Unstated,
            }],
        }
    }

    pub fn parse(text: &str, registry: &crate::EngineRegistry) -> Result<Schedule, ScheduleError> {
        let mut stages = Vec::new();
        let mut delegated: Option<usize> = None;
        for (index, stage_text) in text.split(Self::SEPARATOR).enumerate() {
            let (engine_text, budget_text) = match stage_text.split_once(Self::BUDGET) {
                Some((engine, budget)) => (engine, Some(budget)),
                None => (stage_text, None),
            };
            let budget = match budget_text {
                None => StageBudget::Unstated,
                Some(text) => match text.parse::<u64>() {
                    Err(_) => {
                        return Err(ScheduleError::BadBudget {
                            stage: index,
                            text: text.to_string(),
                        });
                    }
                    Ok(0) => return Err(ScheduleError::ZeroBudget { stage: index }),
                    Ok(seconds) => StageBudget::Seconds(
                        NonZeroU64::new(seconds).expect("zero was refused above"),
                    ),
                },
            };
            let names: Vec<&str> = engine_text.split(Chain::SEPARATOR).collect();
            let engine = if let [name] = names.as_slice() {
                StageEngine::Single(resolve(index, name, registry)?)
            } else {
                let members = names
                    .iter()
                    .map(|name| match resolve(index, name, registry)? {
                        AnyEngine::AustenDriven(AnyGenerator::Tree(engine)) => Ok(engine),
                        _ => Err(ScheduleError::NonTreeChainMember {
                            stage: index,
                            name: name.to_string(),
                            expected: registry.tree_names().collect::<Vec<_>>().join(", "),
                        }),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                StageEngine::Chain(Chain::new(members).map_err(|source| ScheduleError::Chain {
                    stage: index,
                    source,
                })?)
            };
            if engine.loop_owner() == LoopOwner::DelegatedTool {
                if let Some(first) = delegated {
                    return Err(ScheduleError::SecondDelegated {
                        first,
                        second: index,
                    });
                }
                delegated = Some(index);
            }
            stages.push(Stage { engine, budget });
        }
        let last = stages.len() - 1;
        for (index, stage) in stages.iter().enumerate() {
            if index < last && stage.budget == StageBudget::Unstated {
                return Err(ScheduleError::MissingBudget {
                    stage: index,
                    engine: stage.engine.name().to_string(),
                });
            }
        }
        Ok(Schedule { stages })
    }

    pub fn stages(&self) -> &[Stage] {
        &self.stages
    }

    pub fn len(&self) -> usize {
        self.stages.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn is_single(&self) -> bool {
        self.stages.len() == 1
    }

    pub fn single_chain(&self) -> Option<&Chain> {
        match &self.stages[..] {
            [
                Stage {
                    engine: StageEngine::Chain(chain),
                    ..
                },
            ] => Some(chain),
            _ => None,
        }
    }

    pub fn stage_mut(&mut self, index: usize) -> &mut Stage {
        &mut self.stages[index]
    }

    pub fn delegated_stage(&self) -> Option<(usize, AnyDelegatedEngine)> {
        self.stages
            .iter()
            .enumerate()
            .find_map(|(index, stage)| match stage.engine {
                StageEngine::Single(AnyEngine::Delegated(engine)) => Some((index, engine)),
                _ => None,
            })
    }

    pub fn first_austen_driven(&self) -> Option<AnyEngine> {
        self.stages
            .iter()
            .find(|stage| stage.engine.loop_owner() == LoopOwner::Austen)
            .map(|stage| stage.engine.representative())
    }

    pub fn profiles(&self) -> Vec<PassProfile> {
        self.labelled_profiles()
            .into_iter()
            .map(|(profile, _)| profile)
            .collect()
    }

    pub fn labelled_profiles(&self) -> Vec<(PassProfile, String)> {
        let mut labelled: Vec<(PassProfile, Vec<&'static str>)> = Vec::new();
        let mut note = |profile: PassProfile, name: &'static str| match labelled
            .iter_mut()
            .find(|(known, _)| *known == profile)
        {
            Some((_, names)) => {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            None => labelled.push((profile, vec![name])),
        };
        for stage in &self.stages {
            match &stage.engine {
                StageEngine::Single(engine) => {
                    for profile in engine.profiles() {
                        note(profile, engine.name());
                    }
                }
                StageEngine::Chain(chain) => {
                    for member in chain.members() {
                        note(member.passes(), member.name());
                    }
                }
            }
        }
        labelled
            .into_iter()
            .map(|(profile, names)| (profile, names.join("+")))
            .collect()
    }

    pub fn canonical_engine(&self) -> String {
        self.stages
            .iter()
            .map(|stage| stage.engine.name())
            .collect::<Vec<_>>()
            .join(&Self::SEPARATOR.to_string())
    }

    pub fn selection_label(&self) -> String {
        format!(
            "{} (--engine={})",
            self.stages
                .iter()
                .map(|stage| stage.engine.display_name())
                .collect::<Vec<_>>()
                .join(", then "),
            self
        )
    }

    pub fn descriptor(&self) -> EngineDescriptor {
        let stages: Vec<EngineDescriptor> = self
            .stages
            .iter()
            .map(|stage| stage.engine.descriptor())
            .collect();
        EngineDescriptor {
            loop_owner: if stages
                .iter()
                .any(|d| d.loop_owner == LoopOwner::DelegatedTool)
            {
                LoopOwner::DelegatedTool
            } else {
                LoopOwner::Austen
            },
            generic_seed: stages.iter().any(|d| d.generic_seed),
            entropy_tape: self.is_single() && stages[0].entropy_tape,
            function_selection: if stages
                .iter()
                .any(|d| d.function_selection == FunctionSelectionOwner::Austen)
            {
                FunctionSelectionOwner::Austen
            } else if stages
                .iter()
                .any(|d| d.function_selection == FunctionSelectionOwner::DelegatedTool)
            {
                FunctionSelectionOwner::DelegatedTool
            } else {
                FunctionSelectionOwner::Generator
            },
            producer_options: stages.iter().any(|d| d.producer_options),
            max_array_length: stages.iter().any(|d| d.max_array_length),
            coverage: CoveragePolicy {
                observed_by_austen: stages.iter().any(|d| d.coverage.observed_by_austen),
                can_run_blind: stages.iter().all(|d| d.coverage.can_run_blind),
            },
        }
    }
}

impl fmt::Display for Schedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, stage) in self.stages.iter().enumerate() {
            if index > 0 {
                f.write_str(&Self::SEPARATOR.to_string())?;
            }
            f.write_str(stage.engine.name())?;
            if let StageBudget::Seconds(seconds) = stage.budget {
                write!(f, "{}{seconds}", Self::BUDGET)?;
            }
        }
        Ok(())
    }
}

fn resolve(
    stage: usize,
    name: &str,
    registry: &crate::EngineRegistry,
) -> Result<AnyEngine, ScheduleError> {
    if name.is_empty() {
        return Err(ScheduleError::Empty { stage });
    }
    if name == "libfuzzer-fork" {
        return Err(ScheduleError::LibFuzzerFork);
    }
    registry
        .resolve(name)
        .ok_or_else(|| ScheduleError::UnknownEngine {
            stage,
            name: name.to_string(),
            expected: registry.names().collect::<Vec<_>>().join(", "),
        })
}

impl StageEngine {
    pub fn name(&self) -> &'static str {
        match self {
            StageEngine::Single(engine) => engine.name(),
            StageEngine::Chain(chain) => chain.name(),
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            StageEngine::Single(engine) => engine.display_name(),
            StageEngine::Chain(chain) => chain.display_name(),
        }
    }

    pub fn representative(&self) -> AnyEngine {
        match self {
            StageEngine::Single(engine) => *engine,
            StageEngine::Chain(chain) => {
                AnyEngine::AustenDriven(AnyGenerator::Tree(chain.members()[0]))
            }
        }
    }

    pub fn descriptor(&self) -> EngineDescriptor {
        match self {
            StageEngine::Single(engine) => engine.descriptor(),
            StageEngine::Chain(chain) => chain.descriptor(),
        }
    }

    pub fn loop_owner(&self) -> LoopOwner {
        self.descriptor().loop_owner
    }

    pub fn profiles(&self) -> Vec<PassProfile> {
        match self {
            StageEngine::Single(engine) => engine.profiles(),
            StageEngine::Chain(chain) => chain.profiles(),
        }
    }

    pub fn campaign(&self, cfg: CampaignConfig) -> Result<CampaignReport, CampaignError> {
        match self {
            StageEngine::Single(engine) => engine.campaign(cfg),
            StageEngine::Chain(chain) => chain.campaign(cfg),
        }
    }
}
