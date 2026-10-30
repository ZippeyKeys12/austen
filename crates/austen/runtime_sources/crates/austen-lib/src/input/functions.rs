use std::sync::Arc;
use std::time::Instant;
use std::{collections::BTreeMap, path::PathBuf};

use std::num::NonZeroUsize;

use crate::gen_dsl::passes::PassProfile;
use crate::gen_dsl::{
    BaseType, DefaultTypes, Entropy, Env, Evaluator, GenFailure, GenModule, GenTerm, Incoming,
    IndexTerm, RejectTally, SizeArgError, Sym, TestCaseSize, TranslateError, TreeLinearizer,
    bind_size_args, merge_timings, validate_size_args,
};
use crate::wire::AustenInput;

use crate::campaign::CampaignFunction;
#[allow(deprecated)]
use crate::input::{
    AttemptStats, DefaultSizeSelector, EscalationMemberSelector, FunctionSelectionError,
    FunctionSelector, FunctionSelectorKind, FunctionStats, GAVE_UP_ATTEMPTS, GenerateCx,
    GeneratePoll, Generator, GeneratorError, MemberOutcome, MemberSelectionError, MemberSelector,
    MemberStats, RETRIES_EXHAUSTED, Rejections, SizeOutcome, SizeSelectionError, SizeSelector,
    SizeSelectorKind,
};
use crate::internal::{
    AnyTreeEngine, EngineBuild, EngineOpenError, ProductionModules, TreeEngine, TreeEngineFactory,
    validate_generator_pair, validate_module_fork,
};

struct FunctionTree {
    post_defaults: DefaultTypes,
    target_id: u8,
    name: String,
    generator: Sym,
    custom_source: Option<PathBuf>,
    oarg: BaseType,
    root_args: Vec<Vec<RootArg>>,
    wire_at: usize,
    stats: FunctionStats,
}

enum RootArg {
    Incoming(IndexTerm),
    Size(BaseType),
    Heap(IndexTerm),
}

impl FunctionTree {
    fn root(&self, member: usize, size: TestCaseSize) -> GenTerm {
        let args = self.root_args[member]
            .iter()
            .map(|arg| match arg {
                RootArg::Incoming(term) | RootArg::Heap(term) => term.clone(),
                RootArg::Size(bt) => IndexTerm::num_lit(size.get().into(), bt)
                    .expect("opening checked that the test-case size inhabits this argument"),
            })
            .collect();
        GenTerm::call(self.generator.clone(), args, self.oarg.clone())
    }

    fn custom_tree(&self, reason: String) -> Option<GeneratorError> {
        self.custom_source
            .as_ref()
            .map(|file| GeneratorError::CustomTree {
                file: file.clone(),
                entry: self.generator.clone(),
                target: self.name.clone(),
                reason,
            })
    }
}

struct Member {
    name: &'static str,
    profile: PassProfile,
    engine: Box<dyn TreeEngine>,
    custom: BTreeMap<u8, Box<dyn TreeEngine>>,
}

impl Member {
    fn engine_for(&self, target: u8) -> &dyn TreeEngine {
        self.custom
            .get(&target)
            .map_or(self.engine.as_ref(), |engine| engine.as_ref())
    }

    fn engine_for_mut(&mut self, target: u8) -> &mut dyn TreeEngine {
        match self.custom.get_mut(&target) {
            Some(engine) => engine.as_mut(),
            None => self.engine.as_mut(),
        }
    }
}

pub struct TreeGenerator {
    name: &'static str,
    trust: crate::wire::Trust,
    functions: Vec<FunctionTree>,
    wire_module: Arc<GenModule>,
    incoming: Incoming,
    members: Vec<Member>,
    function_selector: Box<dyn FunctionSelector>,
    size_selector: Box<dyn SizeSelector>,
    member_selector: Box<dyn MemberSelector>,
    entropy: Entropy,
    rejected: u64,
    last_attempt: Option<AttemptStats>,
}

impl TreeGenerator {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        engine_kind: AnyTreeEngine,
        selector_kind: FunctionSelectorKind,
        functions: Vec<CampaignFunction>,
        wire_module: Arc<GenModule>,
        production_module: Arc<GenModule>,
        incoming: impl Into<Incoming>,
        max_attempts: u32,
        max_array_length: usize,
        entropy: Entropy,
    ) -> Result<Self, EngineOpenError> {
        Self::open_with_selectors(
            engine_kind,
            selector_kind.open(),
            Box::new(DefaultSizeSelector),
            functions,
            wire_module,
            production_module,
            incoming,
            max_attempts,
            max_array_length,
            None,
            entropy,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn open_with_max_test_case_size(
        engine_kind: AnyTreeEngine,
        selector_kind: FunctionSelectorKind,
        max_test_case_size: TestCaseSize,
        size_selector: SizeSelectorKind,
        functions: Vec<CampaignFunction>,
        wire_module: Arc<GenModule>,
        production_module: Arc<GenModule>,
        incoming: impl Into<Incoming>,
        max_attempts: u32,
        max_array_length: usize,
        generator_timeout: Option<std::time::Duration>,
        entropy: Entropy,
    ) -> Result<Self, EngineOpenError> {
        Self::open_with_selectors(
            engine_kind,
            selector_kind.open(),
            size_selector.open(max_test_case_size),
            functions,
            wire_module,
            production_module,
            incoming,
            max_attempts,
            max_array_length,
            generator_timeout,
            entropy,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn open_with_selector(
        engine_kind: AnyTreeEngine,
        selector: Box<dyn FunctionSelector>,
        functions: Vec<CampaignFunction>,
        wire_module: Arc<GenModule>,
        production_module: Arc<GenModule>,
        incoming: impl Into<Incoming>,
        max_attempts: u32,
        max_array_length: usize,
        entropy: Entropy,
    ) -> Result<Self, EngineOpenError> {
        Self::open_with_selectors(
            engine_kind,
            selector,
            Box::new(DefaultSizeSelector),
            functions,
            wire_module,
            production_module,
            incoming,
            max_attempts,
            max_array_length,
            None,
            entropy,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn open_with_selectors(
        engine_kind: AnyTreeEngine,
        function_selector: Box<dyn FunctionSelector>,
        size_selector: Box<dyn SizeSelector>,
        functions: Vec<CampaignFunction>,
        wire_module: Arc<GenModule>,
        production_module: Arc<GenModule>,
        incoming: impl Into<Incoming>,
        max_attempts: u32,
        max_array_length: usize,
        generator_timeout: Option<std::time::Duration>,
        entropy: Entropy,
    ) -> Result<Self, EngineOpenError> {
        Self::open_members(
            &[engine_kind],
            engine_kind.name(),
            function_selector,
            size_selector,
            Box::new(EscalationMemberSelector::default()),
            functions,
            wire_module,
            ProductionModules::single(engine_kind.passes(), production_module),
            incoming,
            max_attempts,
            max_array_length,
            generator_timeout,
            entropy,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn open_members(
        members: &[AnyTreeEngine],
        name: &'static str,
        function_selector: Box<dyn FunctionSelector>,
        size_selector: Box<dyn SizeSelector>,
        member_selector: Box<dyn MemberSelector>,
        functions: Vec<CampaignFunction>,
        wire_module: Arc<GenModule>,
        production_modules: ProductionModules,
        incoming: impl Into<Incoming>,
        max_attempts: u32,
        max_array_length: usize,
        generator_timeout: Option<std::time::Duration>,
        entropy: Entropy,
    ) -> Result<Self, EngineOpenError> {
        debug_assert!(
            !functions.is_empty(),
            "validate_functions rejects an empty dispatch table"
        );
        assert!(
            !members.is_empty(),
            "a tree generator needs at least one member engine"
        );

        wire_module.validate().map_err(EngineOpenError::Wire)?;
        for (_, production_module) in production_modules.iter() {
            production_module
                .validate()
                .map_err(EngineOpenError::Production)?;
            for function in &functions {
                validate_generator_pair(&function.generator, &wire_module, production_module)?;
            }
            validate_module_fork(&wire_module, production_module)?;
        }
        for (_, custom) in production_modules.custom_iter() {
            custom.validate(&wire_module)?;
        }
        let member_modules = members
            .iter()
            .map(|member| {
                let profile = member.passes();
                production_modules.for_profile(&profile).cloned().ok_or(
                    EngineOpenError::MissingProductionModule {
                        engine: member.name(),
                        profile,
                    },
                )
            })
            .collect::<Result<Vec<_>, _>>()?;

        let incoming = incoming.into();

        let opened = functions
            .into_iter()
            .map(|function| {
                let CampaignFunction {
                    post_defaults,
                    target_id,
                    name,
                    generator,
                } = function;
                let custom = production_modules.custom_for(target_id);
                let custom_interface = custom.map(|custom| {
                    custom
                        .module
                        .generator(&custom.entry)
                        .expect("validation resolved the imported entry")
                });
                let (wire_at, interface) = wire_module
                    .defs
                    .iter()
                    .enumerate()
                    .find(|(_, (name, _))| name == &generator)
                    .map(|(at, (_, def))| (at, def))
                    .expect("validation resolved the wire generator");
                if interface.size_arg.is_none() {
                    return Err(EngineOpenError::MissingTestCaseSize {
                        generator: generator.clone(),
                    });
                }
                if let Err(error) = validate_size_args(interface, &wire_module) {
                    let (SizeArgError::NotDeclared(argument)
                    | SizeArgError::CollidesWithGlobal(argument)
                    | SizeArgError::WrongType(argument)) = error;
                    return Err(EngineOpenError::InvalidTestCaseSizeArgument {
                        generator: generator.clone(),
                        argument,
                    });
                }
                let root_args = member_modules
                    .iter()
                    .map(|production_module| {
                        let production_interface = custom_interface.unwrap_or_else(|| {
                            production_module
                                .generator(&generator)
                                .expect("validation resolved the production generator")
                        });
                        production_interface
                            .iargs
                            .iter()
                            .map(|(param, bt)| {
                                let heap_bound = if production_interface.heap_base_arg.as_ref()
                                    == Some(param)
                                {
                                    Some(incoming.heap().base)
                                } else if production_interface.heap_size_arg.as_ref() == Some(param)
                                {
                                    Some(incoming.heap().size)
                                } else {
                                    None
                                };
                                if production_interface.size_args().any(|size| size == param) {
                                    Ok(RootArg::Size(bt.clone()))
                                } else if let Some(bound) = heap_bound {
                                    IndexTerm::num_lit(bound.into(), bt)
                                        .map(RootArg::Heap)
                                        .ok_or_else(|| EngineOpenError::UnboundFunctionArgument {
                                            generator: generator.clone(),
                                            argument: param.clone(),
                                        })
                                } else if wire_module.find_global(param).is_some() {
                                    Ok(RootArg::Incoming(IndexTerm::sym(param.clone(), bt.clone())))
                                } else {
                                    Err(EngineOpenError::UnboundFunctionArgument {
                                        generator: generator.clone(),
                                        argument: param.clone(),
                                    })
                                }
                            })
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let member_stats = if members.len() > 1 {
                    members
                        .iter()
                        .map(|member| MemberStats::new(member.name()))
                        .collect()
                } else {
                    Vec::new()
                };
                Ok(FunctionTree {
                    post_defaults,
                    target_id,
                    stats: FunctionStats {
                        members: member_stats,
                        ..FunctionStats::new(target_id, name.clone())
                    },
                    name,
                    generator: custom.map_or(generator, |custom| custom.entry.clone()),
                    custom_source: custom.map(|custom| custom.source.clone()),
                    oarg: custom_interface
                        .map_or_else(|| interface.oarg.clone(), |custom| custom.oarg.clone()),
                    root_args,
                    wire_at,
                })
            })
            .collect::<Result<Vec<_>, EngineOpenError>>()?;

        let members = members
            .iter()
            .map(|kind| {
                let engine = kind.open(EngineBuild {
                    production_modules: production_modules.clone(),
                    wire_module: Arc::clone(&wire_module),
                    incoming: incoming.clone(),
                    max_attempts,
                    max_array_length,
                    generator_timeout,
                    entropy: Entropy::seeded(0),
                    custom: None,
                })?;
                let custom = opened
                    .iter()
                    .filter_map(|function| {
                        production_modules
                            .custom_for(function.target_id)
                            .map(|custom| (function, custom))
                    })
                    .map(|(function, custom)| {
                        let wire_entry = wire_module.defs[function.wire_at].0.clone();
                        let producer = custom.open(
                            *kind,
                            EngineBuild {
                                production_modules: production_modules.clone(),
                                wire_module: Arc::clone(&wire_module),
                                incoming: incoming.clone(),
                                max_attempts,
                                max_array_length,
                                generator_timeout,
                                entropy: Entropy::seeded(0),
                                custom: None,
                            },
                            wire_entry,
                        )?;
                        Ok((function.target_id, producer))
                    })
                    .collect::<Result<BTreeMap<_, _>, EngineOpenError>>()?;
                Ok(Member {
                    name: kind.name(),
                    profile: kind.passes(),
                    engine,
                    custom,
                })
            })
            .collect::<Result<Vec<_>, EngineOpenError>>()?;

        Ok(Self {
            name,
            trust: production_modules.trust(),
            functions: opened,
            wire_module,
            incoming,
            members,
            function_selector,
            size_selector,
            member_selector,
            entropy,
            rejected: 0,
            last_attempt: None,
        })
    }
}

impl TreeGenerator {
    pub fn next_input(&mut self) -> Result<AustenInput, GeneratorError> {
        self.next_input_until(&|| false)
            .map(|input| input.expect("unbounded production cannot be interrupted"))
    }

    pub fn next_input_until(
        &mut self,
        stop: &dyn Fn() -> bool,
    ) -> Result<Option<AustenInput>, GeneratorError> {
        self.last_attempt = None;
        if stop() {
            return Ok(None);
        }
        let count = NonZeroUsize::new(self.functions.len())
            .expect("campaign validation rejects an empty function vector");
        let function_selector = &mut self.function_selector;
        let mut decisions = self.entropy.start_input_decisions();
        let selected = decisions
            .resolve(|rng| function_selector.select(count, rng) as u64)
            .map_err(|error| {
                debug_assert_eq!(error.decision, 0);
                FunctionSelectionError::Truncated {
                    available: error.available,
                }
            })?;
        let selected = usize::try_from(selected)
            .ok()
            .filter(|selected| *selected < self.functions.len())
            .ok_or(FunctionSelectionError::OutOfRange {
                index: selected,
                functions: self.functions.len(),
            })?;
        let size_selector = &mut self.size_selector;
        let size = decisions
            .resolve(|rng| size_selector.select(rng).get())
            .map_err(|error| {
                debug_assert_eq!(error.decision, 1);
                SizeSelectionError::Truncated {
                    available: error.available,
                }
            })?;
        let size = TestCaseSize::new(size).ok_or(SizeSelectionError::OutOfRange { size })?;
        let default_seed = decisions.resolve(|rng| rng.next_u64()).map_err(|error| {
            debug_assert_eq!(error.decision, 2);
            GeneratorError::DefaultSeedTruncated {
                available: error.available,
            }
        })?;
        let member_count = NonZeroUsize::new(self.members.len())
            .expect("open_members refuses an empty member list");
        let member_selector = &mut self.member_selector;
        let member = decisions
            .resolve(|rng| member_selector.select(selected, member_count, rng) as u64)
            .map_err(|error| {
                debug_assert_eq!(error.decision, 3);
                MemberSelectionError::Truncated {
                    available: error.available,
                }
            })?;
        let member = usize::try_from(member)
            .ok()
            .filter(|member| *member < self.members.len())
            .ok_or(MemberSelectionError::OutOfRange {
                index: member,
                members: self.members.len(),
            })?;
        let function = &self.functions[selected];
        let evaluator = Evaluator::sampled(
            &self.wire_module,
            size,
            default_seed,
            &function.post_defaults,
        )?;
        let root = function.root(member, size);

        let before = self.members[member]
            .engine_for(function.target_id)
            .rejections();
        let started = Instant::now();
        let result = self.members[member]
            .engine_for_mut(function.target_id)
            .produce_with_entropy_and_evaluator_until(&root, &mut self.entropy, &evaluator, stop)
            .map_err(|error| {
                if let GenFailure::InvalidChoiceTree { reason } = &error
                    && let Some(contextual) = function.custom_tree(reason.clone())
                {
                    return contextual;
                }
                GeneratorError::from(error)
            })
            .and_then(|tree| {
                tree.map(|tree| {
                    let wire = &self.wire_module.defs[function.wire_at].1;
                    let mut env = Env::new();
                    bind_size_args(&mut env, wire, size);
                    let tape = TreeLinearizer::new(&self.wire_module, &self.incoming)
                        .linearize_with_evaluator(&wire.body, env, &tree, &evaluator)
                        .map_err(|error| {
                            if let TranslateError::Default(error) = error {
                                return GeneratorError::from(GenFailure::Default(error));
                            }
                            function.custom_tree(error.to_string()).unwrap_or_else(|| {
                                panic!(
                                    "an accepted tree for function {:?} ({:?}) conforms to its \
                                     wire generator: {error}",
                                    function.name, function.generator
                                )
                            })
                        })?;
                    Ok(AustenInput::from_parts(
                        function.target_id,
                        size,
                        tape.into_bytes(),
                    ))
                })
                .transpose()
            });
        let elapsed = started.elapsed();
        let answered = &self.members[member];
        let delta = answered
            .engine_for(function.target_id)
            .rejections()
            .since(&before);
        let backtracks = delta.total();
        let target_id = function.target_id;
        let stats = &mut self.functions[selected].stats;
        stats.rejections.add(delta.nonzero());
        *stats.sizes.test_case_size.entry(size.get()).or_default() += 1;
        if let Some(member_stats) = stats.members.get_mut(member) {
            member_stats.rejections.add(delta.nonzero());
        }
        let outcome = match &result {
            Ok(Some(_)) => Some((SizeOutcome::Produced, MemberOutcome::Produced)),
            Err(GeneratorError::Generation(GenFailure::RetriesExhausted { .. })) => {
                Some((SizeOutcome::Discarded, MemberOutcome::Exhausted))
            }
            Ok(None) | Err(_) => None,
        };
        if let Some((size_outcome, member_outcome)) = outcome {
            self.size_selector.observe(size_outcome);
            self.member_selector
                .observe(selected, member, member_outcome);
        }
        let owned_bytes = match &result {
            Ok(Some(_)) => {
                stats.inputs += 1;
                let owned_bytes = answered.engine_for(target_id).last_owned_bytes();
                *stats.sizes.owned_bytes.entry(owned_bytes).or_default() += 1;
                if let Some(member_stats) = stats.members.get_mut(member) {
                    member_stats.inputs += 1;
                }
                owned_bytes
            }
            Err(GeneratorError::Generation(GenFailure::RetriesExhausted { .. })) => {
                stats.rejected += 1;
                self.rejected += 1;
                stats.rejections.add([(RETRIES_EXHAUSTED, 1)]);
                if let Some(member_stats) = stats.members.get_mut(member) {
                    member_stats.rejected += 1;
                    member_stats.rejections.add([(RETRIES_EXHAUSTED, 1)]);
                }
                0
            }
            Ok(None) | Err(_) => 0,
        };
        stats.gave_up = stats.inputs == 0 && stats.rejected >= GAVE_UP_ATTEMPTS;
        let head_profile = self.members[0].profile;
        for (site, count) in answered.engine_for(target_id).sites() {
            if answered.profile == head_profile {
                let entry = stats.sites.entry(site.clone()).or_default();
                entry.failed += count.failed;
                entry.passed += count.passed;
            }
            if let Some(member_stats) = stats.members.get_mut(member) {
                let entry = member_stats.sites.entry(site.clone()).or_default();
                entry.failed += count.failed;
                entry.passed += count.passed;
            }
        }
        for (index, member_stats) in stats.members.iter_mut().enumerate() {
            member_stats.retired = self.member_selector.retired(selected, index, member_count);
        }
        let mut timing = answered.engine_for(target_id).timings().clone();
        timing.entry("generate").or_default().observe(elapsed);
        merge_timings(&mut stats.timing, &timing);
        self.last_attempt = Some(AttemptStats {
            function_id: stats.id,
            engine: answered.name,
            backtracks,
            owned_bytes,
            timing,
        });
        result
    }

    pub fn inputs_left(&self) -> usize {
        self.entropy.inputs_left()
    }
}

impl Generator for TreeGenerator {
    fn name(&self) -> &'static str {
        self.name
    }

    fn poll_next(&mut self, cx: GenerateCx<'_>) -> Result<GeneratePoll, GeneratorError> {
        if cx.is_replay() && self.entropy.inputs_left() == 0 {
            return Ok(GeneratePoll::Exhausted);
        }
        match self.next_input_until(&|| cx.is_stopped()) {
            Ok(Some(input)) => Ok(GeneratePoll::Input(input)),
            Ok(None) => Ok(GeneratePoll::Interrupted),
            Err(GeneratorError::Generation(GenFailure::RetriesExhausted { .. })) => {
                Ok(GeneratePoll::Rejected(RETRIES_EXHAUSTED))
            }
            Err(error) => Err(error),
        }
    }

    fn trust(&self) -> crate::wire::Trust {
        self.trust
    }

    fn rejections(&self) -> Rejections {
        let mut tally = RejectTally::default();
        for member in &self.members {
            tally += member.engine.rejections();
            for engine in member.custom.values() {
                tally += engine.rejections();
            }
        }
        let mut sites = tally.nonzero();
        if self.rejected > 0 {
            sites.push((RETRIES_EXHAUSTED, self.rejected));
        }
        Rejections::from_pairs(sites)
    }

    fn replay_remaining(&self) -> Option<usize> {
        Some(self.entropy.inputs_left())
    }

    fn per_function(&self) -> Option<Vec<FunctionStats>> {
        Some(
            self.functions
                .iter()
                .map(|function| function.stats.clone())
                .collect(),
        )
    }

    fn last_attempt(&self) -> Option<&AttemptStats> {
        self.last_attempt.as_ref()
    }
}
