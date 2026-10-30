//! A zero-biased strategy built on Wickham's rejection interpreter.

use austen_lib::gen_dsl::choice::translate::scalar_value;
use austen_lib::gen_dsl::passes::PassProfile;
use austen_lib::gen_dsl::{
    BaseType, ChoiceTree, Entropy, EntropyBacked, GenFailure, GenTerm, PrimitiveContext,
    PrimitiveDraw, PrimitiveGenerator, ProduceCx, RejectTally, RejectingProducer, SiteTally,
    Timings, TreeProducer,
};
use austen_lib::{EngineBuild, EngineOpenError, TreeEngine, TreeEngineFactory};
use austen_wickham::{SeededWickham, WickhamPrimitiveGenerator};

/// Choose zero for half of plain integer draws; otherwise use Wickham's policy.
/// Constraints, recursion, pointers, retries and source defaults keep their
/// existing interpreter behavior.
struct ZeroBiasedScalars;

impl PrimitiveGenerator for ZeroBiasedScalars {
    fn arbitrary(
        &self,
        target: &BaseType,
        context: &mut PrimitiveContext<'_>,
    ) -> Result<PrimitiveDraw, GenFailure> {
        if matches!(target, BaseType::Bits(..))
            && context.rng.next_u64() & 1 == 0
            && let Some(value) = scalar_value(0, target)
        {
            return Ok(PrimitiveDraw::Value(value));
        }
        WickhamPrimitiveGenerator::default().arbitrary(target, context)
    }
}

/// The live producer. Forward the caller's context intact: cancellation and
/// per-input default choices must be shared across all retries.
pub struct ZeroBiased(SeededWickham);

impl TreeProducer for ZeroBiased {
    fn produce_in(
        &mut self,
        term: &GenTerm,
        cx: ProduceCx<'_, '_>,
    ) -> Result<Option<ChoiceTree>, GenFailure> {
        self.0.produce_in(term, cx)
    }
}

impl RejectingProducer for ZeroBiased {
    fn rejections(&self) -> RejectTally {
        self.0.rejections()
    }

    fn sites(&self) -> &SiteTally {
        self.0.sites()
    }

    fn timings(&self) -> &Timings {
        self.0.timings()
    }

    fn last_owned_bytes(&self) -> u64 {
        self.0.last_owned_bytes()
    }
}

impl EntropyBacked for ZeroBiased {
    fn entropy_mut(&mut self) -> &mut Entropy {
        self.0.entropy_mut()
    }
}

/// Registration describes how to open a producer once its module and incoming
/// memory map are available. The default `open` validates the module pair.
pub struct ZeroBiasedFactory;

impl TreeEngineFactory for ZeroBiasedFactory {
    fn name(&self) -> &'static str {
        "zero-biased"
    }

    fn display_name(&self) -> &'static str {
        "Zero-biased Wickham"
    }

    fn passes(&self) -> PassProfile {
        austen_wickham::PASSES
    }

    fn open_validated(&self, build: EngineBuild) -> Result<Box<dyn TreeEngine>, EngineOpenError> {
        let module = build.production_module(self.name(), self.passes())?;
        let producer =
            SeededWickham::new(module, build.incoming, build.max_attempts, build.entropy)
                .map_err(EngineOpenError::Production)?
                .with_generator_timeout(build.generator_timeout)
                .with_primitive_generator(ZeroBiasedScalars);
        Ok(Box::new(ZeroBiased(producer)))
    }
}
