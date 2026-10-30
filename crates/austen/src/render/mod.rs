mod human;
mod jsonl;
mod style;

use std::io::IsTerminal;
use std::sync::Arc;

use austen_lib::observe::{Reporter, RunObserver, Tee};

pub use human::HumanRenderer;
pub use jsonl::JsonlRenderer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum When {
    #[default]
    Auto,
    Always,
    Never,
}

impl std::str::FromStr for When {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "auto" => Ok(When::Auto),
            "always" => Ok(When::Always),
            "never" => Ok(When::Never),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Verbosity {
    pub quiet: bool,
    pub level: u8,
}

pub const TRACE_INPUTS_LEVEL: u8 = 3;

impl Verbosity {
    pub fn filter(self) -> String {
        if self.quiet {
            return "off".to_string();
        }
        match self.level {
            0 => "warn".to_string(),
            1 => "warn,austen=debug,austen::input=off".to_string(),
            2 => "warn,austen=trace,austen::input=off".to_string(),
            _ => "warn,austen=trace".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Output {
    pub progress: When,
    pub color: When,
    pub verbosity: Verbosity,
    pub events: Option<std::path::PathBuf>,
}

impl Output {
    pub fn dynamic(&self) -> bool {
        match self.progress {
            When::Always => true,
            When::Never => false,
            When::Auto => std::io::stderr().is_terminal(),
        }
    }

    pub fn reporter(&self) -> std::io::Result<Reporter> {
        let mut sinks: Vec<Arc<dyn RunObserver>> = Vec::new();
        if !self.verbosity.quiet {
            sinks.push(Arc::new(HumanRenderer::new(self)));
        }
        if let Some(dest) = &self.events {
            sinks.push(Arc::new(JsonlRenderer::open(dest)?));
        }
        Ok(match sinks.len() {
            0 => Reporter::silent(),
            1 => Reporter::new(sinks.pop().expect("one sink")),
            _ => Reporter::new(Arc::new(Tee(sinks))),
        })
    }
}
