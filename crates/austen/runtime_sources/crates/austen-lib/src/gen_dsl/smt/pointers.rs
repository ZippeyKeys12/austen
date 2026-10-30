#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PointerEncoding {
    Address,
    #[default]
    Provenance,
}

impl PointerEncoding {
    pub fn is_provenance(self) -> bool {
        self == PointerEncoding::Provenance
    }
}
