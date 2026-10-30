use crate::SeededDarcy;

use austen_lib::internal::{EngineBuild, EngineOpenError, TreeEngine, TreeEngineFactory};

pub const NAME: &str = "darcy";

pub const DISPLAY_NAME: &str = "Darcy";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DarcyFactory;

impl TreeEngineFactory for DarcyFactory {
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
        let engine = SeededDarcy::new(
            build.production_module(NAME, self.passes())?,
            build.incoming,
            build.max_attempts,
            build.entropy,
        )
        .map_err(EngineOpenError::Production)?;
        let engine = match build.custom {
            Some(custom) => engine.with_validator_entry(build.wire_module, custom.wire_entry),
            None => engine.with_validator(build.wire_module),
        }
        .map_err(EngineOpenError::Wire)?
        .with_max_array_length(build.max_array_length)
        .with_generator_timeout(build.generator_timeout);
        Ok(Box::new(engine))
    }
}
