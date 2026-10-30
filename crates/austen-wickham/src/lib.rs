mod attempt;
mod primitive;

mod factory;
pub use factory::WickhamFactory;

use std::sync::Arc;

use attempt::walk::{Attempt, Stop, walk_root};
pub use primitive::WickhamPrimitiveGenerator;

use austen_lib::gen_dsl::MarkedHeap;
pub use austen_lib::gen_dsl::Region;
use austen_lib::gen_dsl::{
    ChoiceTree, Entropy, EntropyBacked, GenFailure, GenModule, GenModuleError, GenTerm, Incoming,
    PrimitiveGenerator, ProduceCx, RejectingProducer, Rng, Scope, TestCaseSize, TreeProducer,
};
pub use austen_lib::gen_dsl::{RejectReason, RejectTally};

pub use austen_lib::gen_dsl::rand_like::DEFAULT_MAX_SIZE;

pub use austen_lib::internal::DEFAULT_MAX_ATTEMPTS;

pub const PASSES: austen_lib::gen_dsl::passes::PassProfile =
    austen_lib::gen_dsl::passes::PassProfile {
        lift_constraints: false,
        each_fusion: true,
        apply_array_max_length: false,
        specialize_guarded_choices: true,
    };

pub const POINTEE_SIZE: u64 = austen_lib::gen_dsl::DEFAULT_POINTEE_BYTES;

pub struct Wickham<R: Rng> {
    module: Arc<GenModule>,
    incoming: Incoming,
    max_size: u64,
    generator_timeout: Option<std::time::Duration>,
    max_attempts: u32,
    primitive: Box<dyn PrimitiveGenerator>,
    rng: R,
    rejects: RejectTally,
    site_index: austen_lib::gen_dsl::SiteIndex,
    sites: austen_lib::gen_dsl::SiteTally,
    last_owned_bytes: u64,
}

impl<R: Rng> Wickham<R> {
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
            max_size: DEFAULT_MAX_SIZE,
            generator_timeout: None,
            max_attempts,
            primitive: Box::new(WickhamPrimitiveGenerator::default()),
            rng,
            rejects: RejectTally::default(),
            site_index,
            sites: austen_lib::gen_dsl::SiteTally::new(),
            last_owned_bytes: 0,
        })
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
    pub fn with_pointee_bytes(mut self, bytes: u64) -> Self {
        self.primitive = Box::new(WickhamPrimitiveGenerator::new(bytes));
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

    pub fn module(&self) -> &GenModule {
        &self.module
    }
}

pub type SeededWickham = Wickham<Entropy>;

impl SeededWickham {
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

impl<R: Rng> TreeProducer for Wickham<R> {
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
        let size = root_size
            .or(evaluator.size())
            .unwrap_or_else(|| TestCaseSize::sample_uniform(&mut self.rng, self.max_size));
        let deadline = self
            .generator_timeout
            .map(|d| std::time::Instant::now() + d);
        for attempt in 0..self.max_attempts {
            if stop() {
                return Ok(None);
            }
            if attempt > 0 && deadline.is_some_and(|at| std::time::Instant::now() >= at) {
                break;
            }
            let mut att = Attempt {
                rng: &mut self.rng,
                heap: MarkedHeap::with_base(self.incoming.heap().base),
                incoming: &self.incoming,
                writes: Vec::new(),
                initial_size: size,
                primitive: self.primitive.as_ref(),
                deadline,
                ctx: &self.module,
                site_index: &self.site_index,
                sites: &mut self.sites,
                evaluator: &evaluator,
            };
            let mut root = Scope::new();
            let result = walk_root(&mut att, term, &mut root);
            if let Some(error) = evaluator.default_error() {
                return Err(error.into());
            }
            match result {
                Ok(_) => {
                    self.last_owned_bytes = att.writes.iter().map(|region| region.size).sum();
                    let mut tree = ChoiceTree::new(root);
                    tree.defaults = evaluator.default_choices();
                    return Ok(Some(tree));
                }
                Err(Stop::Reject(reason)) => {
                    self.rejects.bump(reason);
                    continue;
                }
                Err(Stop::Hard(failure)) => return Err(failure),
            }
        }
        Err(GenFailure::RetriesExhausted {
            attempts: self.max_attempts,
        })
    }
}

impl RejectingProducer for SeededWickham {
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

impl EntropyBacked for SeededWickham {
    fn entropy_mut(&mut self) -> &mut Entropy {
        &mut self.rng
    }
}
