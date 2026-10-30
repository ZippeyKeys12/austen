use super::AustenInput;
use libafl::inputs::{HasTargetBytes, Input};
use libafl_bolts::ownedref::OwnedSlice;

impl Input for AustenInput {}

impl HasTargetBytes for AustenInput {
    fn target_bytes(&self) -> OwnedSlice<'_, u8> {
        OwnedSlice::from(self.as_bytes())
    }
}
