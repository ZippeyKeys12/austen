use austen_lib::gen_dsl::choice::translate::scalar_value;
use austen_lib::gen_dsl::rand_like::lemire;
use austen_lib::gen_dsl::{
    BaseType, GenFailure, PrimitiveContext, PrimitiveDraw, PrimitiveGenerator, Sign, sized_scalar,
};

use crate::NULL_IN_EVERY;

#[derive(Debug, Default, Clone, Copy)]
pub struct BennetPrimitiveGenerator;

impl PrimitiveGenerator for BennetPrimitiveGenerator {
    fn arbitrary(
        &self,
        target: &BaseType,
        context: &mut PrimitiveContext<'_>,
    ) -> Result<PrimitiveDraw, GenFailure> {
        let raw = match sized_scalar(context.rng, context.size.get(), target) {
            Some(result) => result?,
            None => match target {
                BaseType::Loc(_) => {
                    if lemire::uniform_u64(context.rng, NULL_IN_EVERY) == 0 {
                        0
                    } else {
                        sized_scalar(
                            context.rng,
                            context.size.get(),
                            &BaseType::Bits(Sign::Unsigned, 64),
                        )
                        .expect("a 64-bit unsigned target is a scalar")?
                    }
                }
                _ => return Err(GenFailure::UnsupportedBaseType { bt: target.clone() }),
            },
        };
        let value = scalar_value(raw, target)
            .expect("Bennet primitive draws are representable at their target type");
        Ok(PrimitiveDraw::Value(value))
    }
}
