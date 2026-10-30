pub const HEAP_BYTES_ENV: &str = "AUSTEN_HEAP_BYTES";

pub use crate::gen_dsl::DEFAULT_HEAP_BYTES;

pub const HEAP_BASE_ENV: &str = "AUSTEN_HEAP_BASE";

pub const ALLOW_UNSTABLE_ADDRESSES_ENV: &str = "AUSTEN_ALLOW_UNSTABLE_ADDRESSES";

pub use crate::gen_dsl::DEFAULT_HEAP_BASE;

pub fn is_default_base(base: u64) -> bool {
    base == DEFAULT_HEAP_BASE
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeapBase {
    Hint(u64),
    Required(u64),
    Kernel,
}

impl Default for HeapBase {
    fn default() -> Self {
        HeapBase::Required(DEFAULT_HEAP_BASE)
    }
}

impl HeapBase {
    pub fn parse(value: &str) -> Result<HeapBase, HeapBaseConfigError> {
        let addr =
            super::radix::parse_u64(value).ok_or_else(|| HeapBaseConfigError::InvalidBase {
                given: value.to_owned(),
            })?;
        Ok(match addr {
            0 => HeapBase::Kernel,
            addr => HeapBase::Required(addr),
        })
    }

    fn configured(value: Option<&str>, allow_unstable: bool) -> Result<Self, HeapBaseConfigError> {
        match value {
            Some(value) => match Self::parse(value)? {
                HeapBase::Kernel if !allow_unstable => Err(HeapBaseConfigError::UnstableNotAllowed),
                placement => Ok(placement),
            },
            None if allow_unstable => Ok(HeapBase::Hint(DEFAULT_HEAP_BASE)),
            None => Ok(Self::default()),
        }
    }

    pub fn from_env() -> Result<HeapBase, HeapBaseConfigError> {
        let permission = std::env::var(ALLOW_UNSTABLE_ADDRESSES_ENV).ok();
        let allow_unstable = unstable_addresses_allowed(permission.as_deref())?;
        let base = std::env::var(HEAP_BASE_ENV).ok();
        Self::configured(base.as_deref(), allow_unstable)
    }
}

pub fn unstable_addresses_token(allow: bool) -> &'static str {
    if allow { "1" } else { "0" }
}

fn unstable_addresses_allowed(value: Option<&str>) -> Result<bool, HeapBaseConfigError> {
    match value {
        None | Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(given) => Err(HeapBaseConfigError::InvalidPermission {
            given: given.to_owned(),
        }),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HeapBaseConfigError {
    #[error(
        "bad {HEAP_BASE_ENV} `{given}`: expected 0 (kernel-chosen) or an address in decimal or 0x hex"
    )]
    InvalidBase { given: String },
    #[error("bad {ALLOW_UNSTABLE_ADDRESSES_ENV} `{given}`: expected 0 or 1")]
    InvalidPermission { given: String },
    #[error(
        "{HEAP_BASE_ENV}=0 requires --allow-unstable-addresses \
         (standalone harness: {ALLOW_UNSTABLE_ADDRESSES_ENV}=1); \
         choose a nonzero {HEAP_BASE_ENV} for fixed placement"
    )]
    UnstableNotAllowed,
}
