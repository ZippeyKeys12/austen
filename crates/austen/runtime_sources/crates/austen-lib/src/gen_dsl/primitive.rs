use crate::gen_dsl::choice::GenFailure;
use crate::gen_dsl::lang::{BaseType, GenDeclarations, Value};
use crate::gen_dsl::rand_like::{Rng, TestCaseSize, sized_bits_with_extrema_skew, uniform_in};
use crate::gen_dsl::{MarkedHeap, Region, RejectReason};

pub struct PrimitiveContext<'a> {
    pub rng: &'a mut dyn Rng,
    pub size: TestCaseSize,
    pub heap: &'a mut MarkedHeap,
    pub heap_bounds: Region,
    pub declarations: GenDeclarations<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrimitiveDraw {
    Value(Value),
    Rejected(RejectReason),
}

pub trait PrimitiveGenerator: Send + Sync {
    fn arbitrary(
        &self,
        target: &BaseType,
        context: &mut PrimitiveContext<'_>,
    ) -> Result<PrimitiveDraw, GenFailure>;
}

pub fn sized_scalar(
    rng: &mut dyn Rng,
    size: u64,
    target: &BaseType,
) -> Option<Result<i128, GenFailure>> {
    match target {
        BaseType::Bool => Some(Ok(uniform_in(rng, 0, 1))),
        BaseType::Bits(sign, width) => Some(
            sized_bits_with_extrema_skew(rng, size, *sign, *width)
                .ok_or(GenFailure::UnrepresentableWidth { bits: *width }),
        ),
        _ => None,
    }
}

impl<P: PrimitiveGenerator + ?Sized> PrimitiveGenerator for Box<P> {
    fn arbitrary(
        &self,
        target: &BaseType,
        context: &mut PrimitiveContext<'_>,
    ) -> Result<PrimitiveDraw, GenFailure> {
        (**self).arbitrary(target, context)
    }
}

impl<P: PrimitiveGenerator + ?Sized> PrimitiveGenerator for std::sync::Arc<P> {
    fn arbitrary(
        &self,
        target: &BaseType,
        context: &mut PrimitiveContext<'_>,
    ) -> Result<PrimitiveDraw, GenFailure> {
        (**self).arbitrary(target, context)
    }
}
