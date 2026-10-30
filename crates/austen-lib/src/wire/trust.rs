use strum::{EnumIter, IntoEnumIterator};

pub const TRUST_ENV: &str = "AUSTEN_TRUST";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, EnumIter)]
pub enum Trust {
    #[default]
    Checked,
    Producer,
}

impl Trust {
    pub fn name(self) -> &'static str {
        match self {
            Trust::Checked => "checked",
            Trust::Producer => "producer",
        }
    }

    pub fn parse(name: &str) -> Result<Trust, UnknownTrust> {
        Self::iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| UnknownTrust {
                given: name.to_owned(),
            })
    }

    pub fn from_env() -> Result<Trust, UnknownTrust> {
        match std::env::var(TRUST_ENV) {
            Ok(name) => Trust::parse(&name),
            Err(_) => Ok(Trust::default()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "unknown {TRUST_ENV} `{given}`: expected one of {}",
    Trust::iter().map(Trust::name).collect::<Vec<_>>().join(", ")
)]
pub struct UnknownTrust {
    pub given: String,
}
