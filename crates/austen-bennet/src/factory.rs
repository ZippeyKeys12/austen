use crate::SeededBennet;

use austen_lib::internal::{EngineBuild, EngineOpenError, TreeEngine, TreeEngineFactory};

pub const NAME: &str = "bennet";

pub const DISPLAY_NAME: &str = "Bennet";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BennetFactory;

impl TreeEngineFactory for BennetFactory {
    fn name(&self) -> &'static str {
        NAME
    }

    fn display_name(&self) -> &'static str {
        DISPLAY_NAME
    }

    fn passes(&self) -> austen_lib::gen_dsl::passes::PassProfile {
        crate::PASSES
    }

    fn open_validated(&self, build: EngineBuild) -> Result<Box<dyn TreeEngine>, EngineOpenError> {
        let engine = SeededBennet::new(
            build.production_module(NAME, self.passes())?,
            build.incoming,
            build.max_attempts,
            build.entropy,
        )
        .map_err(EngineOpenError::Production)?
        .with_generator_timeout(build.generator_timeout);
        Ok(Box::new(engine))
    }
}
