mod factory;
pub use factory::BennetFactory;

use std::sync::Arc;

use austen_lib::gen_dsl::{
    ChoiceTree, Entropy, EntropyBacked, GenFailure, GenModule, GenModuleError, GenTerm, Incoming,
    MarkedHeap, PrimitiveGenerator, ProduceCx, RejectingProducer, Rewind, Rng, TestCaseSize,
    TreeProducer,
};

mod interp;
mod primitive;

pub use primitive::BennetPrimitiveGenerator;

pub use austen_lib::gen_dsl::Region;
pub use austen_lib::gen_dsl::{RejectReason, RejectTally};

use interp::{Attempt, Stop};

pub use austen_lib::gen_dsl::rand_like::DEFAULT_MAX_SIZE;

pub use austen_lib::internal::DEFAULT_MAX_ATTEMPTS;

pub const PASSES: austen_lib::gen_dsl::passes::PassProfile =
    austen_lib::gen_dsl::passes::PassProfile {
        lift_constraints: true,
        each_fusion: true,
        apply_array_max_length: true,
        specialize_guarded_choices: true,
    };

pub const DEFAULT_MAX_BACKTRACKS: u32 = 25;

pub const NULL_IN_EVERY: u64 = 5;

pub const SEARCH_STEPS: u64 = 1 << 22;

pub struct Bennet<R: Rng + Rewind> {
    module: Arc<GenModule>,
    incoming: Incoming,
    max_attempts: u32,
    max_size: u64,
    max_backtracks: u32,
    generator_timeout: Option<std::time::Duration>,
    primitive: Box<dyn PrimitiveGenerator>,
    rng: R,
    rejects: RejectTally,
    site_index: austen_lib::gen_dsl::SiteIndex,
    sites: austen_lib::gen_dsl::SiteTally,
    last_owned_bytes: u64,
}

impl<R: Rng + Rewind> Bennet<R> {
    pub fn new(
        module: Arc<GenModule>,
        incoming: impl Into<Incoming>,
        max_attempts: u32,
        rng: R,
    ) -> Result<Self, GenModuleError> {
        module.validate()?;
        let incoming = incoming.into();
        incoming.heap().debug_check_published();
        let site_index = austen_lib::gen_dsl::SiteIndex::new(&module);
        Ok(Self {
            module,
            incoming,
            max_attempts,
            max_size: DEFAULT_MAX_SIZE,
            generator_timeout: None,
            max_backtracks: DEFAULT_MAX_BACKTRACKS,
            primitive: Box::new(BennetPrimitiveGenerator),
            rng,
            rejects: RejectTally::default(),
            site_index,
            sites: austen_lib::gen_dsl::SiteTally::new(),
            last_owned_bytes: 0,
        })
    }

    pub fn module(&self) -> &GenModule {
        &self.module
    }

    pub fn rejections(&self) -> RejectTally {
        self.rejects
    }

    #[must_use]
    pub fn with_max_size(mut self, max_size: u64) -> Self {
        self.max_size = max_size.clamp(1, austen_lib::gen_dsl::rand_like::MAX_SIZE_LIMIT);
        self
    }

    #[must_use]
    pub fn with_generator_timeout(mut self, timeout: Option<std::time::Duration>) -> Self {
        self.generator_timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_primitive_generator(
        mut self,
        primitive: impl PrimitiveGenerator + 'static,
    ) -> Self {
        self.primitive = Box::new(primitive);
        self
    }

    #[must_use]
    pub fn with_max_backtracks(mut self, max_backtracks: u32) -> Self {
        self.max_backtracks = max_backtracks;
        self
    }
}

pub type SeededBennet = Bennet<Entropy>;

impl SeededBennet {
    pub fn seeded(
        module: Arc<GenModule>,
        incoming: impl Into<Incoming>,
        max_attempts: u32,
        seed: u64,
    ) -> Result<Self, GenModuleError> {
        Self::new(module, incoming, max_attempts, Entropy::seeded(seed))
    }

    pub fn entropy(&self) -> &Entropy {
        &self.rng
    }
}

impl<R: Rng + Rewind> TreeProducer for Bennet<R> {
    fn produce_in(
        &mut self,
        term: &GenTerm,
        cx: ProduceCx<'_, '_>,
    ) -> Result<Option<ChoiceTree>, GenFailure> {
        let ProduceCx { evaluator, stop } = cx;
        self.last_owned_bytes = 0;
        self.sites.clear();
        if stop() {
            return Ok(None);
        }
        if self.max_attempts == 0 {
            return Err(GenFailure::RetriesExhausted { attempts: 0 });
        }
        let module = Arc::clone(&self.module);
        let (root_size, evaluator) = austen_lib::gen_dsl::request_evaluator(
            term,
            &module,
            &self.incoming,
            evaluator,
            &mut self.rng,
            self.max_size,
        )?;
        let deadline = self
            .generator_timeout
            .map(|d| std::time::Instant::now() + d);
        let capacity = interp::ownership_capacity(&self.incoming);
        for attempt in 0..self.max_attempts {
            if stop() {
                return Ok(None);
            }
            if attempt > 0 && deadline.is_some_and(|at| std::time::Instant::now() >= at) {
                break;
            }
            let size = root_size
                .or_else(|| evaluator.size())
                .unwrap_or_else(|| TestCaseSize::sample_uniform(&mut self.rng, self.max_size));
            let bounds = self.incoming.heap();
            let mut att = Attempt {
                rng: &mut self.rng,
                heap: MarkedHeap::with_base(bounds.base),
                bounds,
                incoming: &self.incoming,
                capacity,
                writes: Vec::new(),
                initial_size: size,
                primitive: self.primitive.as_ref(),
                max_backtracks: self.max_backtracks,
                deadline,
                steps: SEARCH_STEPS,
                ctx: &self.module,
                evaluator: &evaluator,
                site_index: &self.site_index,
                sites: &mut self.sites,
                tally: &mut self.rejects,
                current_term: None,
                stop,
            };
            let result = interp::run(&mut att, term);
            if let Some(error) = evaluator.default_error() {
                return Err(error.into());
            }
            match result {
                Ok(root) => {
                    self.last_owned_bytes =
                        att.heap.regions().iter().map(|region| region.size).sum();
                    let mut tree = ChoiceTree::new(root);
                    tree.defaults = evaluator.default_choices();
                    return Ok(Some(tree));
                }
                Err(Stop::Fail(_)) => continue,
                Err(Stop::Hard(failure)) => return Err(failure),
                Err(Stop::Interrupted) => return Ok(None),
            }
        }
        Err(GenFailure::RetriesExhausted {
            attempts: self.max_attempts,
        })
    }
}

impl RejectingProducer for SeededBennet {
    fn rejections(&self) -> RejectTally {
        self.rejects
    }

    fn sites(&self) -> &austen_lib::gen_dsl::SiteTally {
        &self.sites
    }

    fn last_owned_bytes(&self) -> u64 {
        self.last_owned_bytes
    }
}

impl EntropyBacked for SeededBennet {
    fn entropy_mut(&mut self) -> &mut Entropy {
        &mut self.rng
    }
}
