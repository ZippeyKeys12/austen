use austen_lib::gen_dsl::choice::translate::scalar_value;
use austen_lib::gen_dsl::rand_like::lemire;
use austen_lib::gen_dsl::{
    BaseType, GenFailure, PrimitiveContext, PrimitiveDraw, PrimitiveGenerator, RejectReason,
    sized_scalar,
};

use crate::POINTEE_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WickhamPrimitiveGenerator {
    pointee_bytes: u64,
}

impl WickhamPrimitiveGenerator {
    pub const fn new(pointee_bytes: u64) -> Self {
        Self {
            pointee_bytes: if pointee_bytes == 0 { 1 } else { pointee_bytes },
        }
    }

    pub const fn pointee_bytes(self) -> u64 {
        self.pointee_bytes
    }
}

impl Default for WickhamPrimitiveGenerator {
    fn default() -> Self {
        Self::new(POINTEE_SIZE)
    }
}

impl PrimitiveGenerator for WickhamPrimitiveGenerator {
    fn arbitrary(
        &self,
        target: &BaseType,
        context: &mut PrimitiveContext<'_>,
    ) -> Result<PrimitiveDraw, GenFailure> {
        let drawn = match sized_scalar(context.rng, context.size.get(), target) {
            Some(result) => result?,
            None => match target {
                BaseType::Loc(pointee) => {
                    let bytes = austen_lib::gen_dsl::pointee_bytes(
                        pointee.as_deref(),
                        context.declarations.structs(),
                        self.pointee_bytes,
                    )?;

                    if lemire::uniform_u64(context.rng, 2) == 1 {
                        0
                    } else {
                        let want_existing = lemire::uniform_u64(context.rng, 2) == 1;
                        if want_existing && !context.heap.regions().is_empty() {
                            let len = context.heap.regions().len() as u64;
                            let at = lemire::uniform_u64(context.rng, len) as usize;
                            i128::from(context.heap.regions()[at].base)
                        } else if bytes == 0 {
                            i128::from(context.heap_bounds.base)
                        } else {
                            let Some(region) =
                                context.heap.try_alloc(bytes, context.heap_bounds.size)
                            else {
                                return Ok(PrimitiveDraw::Rejected(RejectReason::PointerCapacity));
                            };
                            i128::from(region.base)
                        }
                    }
                }
                _ => return Err(GenFailure::UnsupportedBaseType { bt: target.clone() }),
            },
        };

        let value = scalar_value(drawn, target)
            .expect("Wickham primitive draws are representable at their target type");
        Ok(PrimitiveDraw::Value(value))
    }
}
