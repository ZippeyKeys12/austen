use super::*;
use crate::instrumentation::{CompileRequest, Coverage};
use std::path::{Path, PathBuf};
use std::process::Child;

#[derive(Clone, Copy)]
pub struct AnyFuzzer(&'static (dyn Fuzzer + Sync));

impl AnyFuzzer {
    pub const fn new(fuzzer: &'static (dyn Fuzzer + Sync)) -> Self {
        Self(fuzzer)
    }
    fn inner(&self) -> &dyn Fuzzer {
        self.0
    }
}
impl std::fmt::Debug for AnyFuzzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnyFuzzer")
            .field("name", &Fuzzer::name(self))
            .field("mode", &Fuzzer::mode(self))
            .finish()
    }
}
impl PartialEq for AnyFuzzer {
    fn eq(&self, other: &Self) -> bool {
        Fuzzer::name(self) == Fuzzer::name(other) && Fuzzer::mode(self) == Fuzzer::mode(other)
    }
}
impl Eq for AnyFuzzer {}

#[derive(Debug, Clone, Copy)]
pub enum AnyDelegatedEngine {
    Fuzzer(AnyFuzzer),
    Direct(DelegatedEngineHandle),
}

impl PartialEq for AnyDelegatedEngine {
    fn eq(&self, other: &Self) -> bool {
        self.name() == other.name() && self.mode() == other.mode()
    }
}
impl Eq for AnyDelegatedEngine {}

#[derive(Clone, Copy)]
pub struct DelegatedEngineHandle(&'static (dyn DelegatedEngine + Sync));
impl DelegatedEngineHandle {
    pub const fn new(engine: &'static (dyn DelegatedEngine + Sync)) -> Self {
        Self(engine)
    }
}
impl std::fmt::Debug for DelegatedEngineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DelegatedEngineHandle")
            .field("name", &self.0.name())
            .field("mode", &self.0.mode())
            .finish()
    }
}
impl PartialEq for DelegatedEngineHandle {
    fn eq(&self, other: &Self) -> bool {
        self.0.name() == other.0.name() && self.0.mode() == other.0.mode()
    }
}
impl Eq for DelegatedEngineHandle {}

impl Fuzzer for AnyFuzzer {
    fn name(&self) -> &'static str {
        self.inner().name()
    }

    fn mode(&self) -> Option<ExternalMode> {
        self.inner().mode()
    }

    fn display_name(&self) -> &'static str {
        self.inner().display_name()
    }

    fn slug(&self) -> &'static str {
        self.inner().slug()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner().capabilities()
    }

    fn coverage(&self) -> &'static [Coverage] {
        self.inner().coverage()
    }

    fn rejected_coverage(&self) -> &'static [Coverage] {
        self.inner().rejected_coverage()
    }

    fn persistent_hooks(&self) -> Option<PersistentHookSymbols> {
        self.inner().persistent_hooks()
    }

    fn driver_symbol(&self) -> Option<&'static str> {
        self.inner().driver_symbol()
    }

    fn preflight_harness(&self, harness: &Path) -> Result<(), ExternalError> {
        self.inner().preflight_harness(harness)
    }

    fn compile_request(&self) -> CompileRequest {
        self.inner().compile_request()
    }

    fn target_compiler(&self) -> Result<Option<TargetCompiler>, NotFound> {
        self.inner().target_compiler()
    }

    fn run_plan(&self, req: &RunRequest<'_>) -> RunPlan {
        self.inner().run_plan(req)
    }

    fn parse_stats(&self, text: &str) -> Stats {
        self.inner().parse_stats(text)
    }

    fn discovery(&self) -> Discovery {
        self.inner().discovery()
    }

    fn discover(&self) -> Result<PathBuf, NotFound> {
        self.inner().discover()
    }

    fn harness_link(&self) -> Result<Option<HarnessLink>, NotFound> {
        self.inner().harness_link()
    }
}

impl AnyDelegatedEngine {
    pub fn as_fuzzer(self) -> Option<AnyFuzzer> {
        match self {
            Self::Fuzzer(fuzzer) => Some(fuzzer),
            Self::Direct(_) => None,
        }
    }

    fn inner(&self) -> &dyn DelegatedEngine {
        match self {
            Self::Fuzzer(inner) => inner,
            Self::Direct(inner) => inner.0,
        }
    }
}

impl DelegatedEngine for AnyDelegatedEngine {
    fn name(&self) -> &'static str {
        self.inner().name()
    }

    fn mode(&self) -> Option<ExternalMode> {
        self.inner().mode()
    }

    fn display_name(&self) -> &'static str {
        self.inner().display_name()
    }

    fn slug(&self) -> &'static str {
        self.inner().slug()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner().capabilities()
    }

    fn compile_request(&self) -> CompileRequest {
        self.inner().compile_request()
    }

    fn target_compiler(&self) -> Result<Option<TargetCompiler>, NotFound> {
        self.inner().target_compiler()
    }

    fn coverage(&self) -> &'static [Coverage] {
        self.inner().coverage()
    }

    fn rejected_coverage(&self) -> &'static [Coverage] {
        self.inner().rejected_coverage()
    }

    fn harness_link(&self) -> Result<Option<HarnessLink>, NotFound> {
        self.inner().harness_link()
    }

    fn stage(&self, req: &ExternalRequest<'_>) -> Result<StagedCampaign, ExternalError> {
        self.inner().stage(req)
    }

    fn launch(
        &self,
        staged: &StagedCampaign,
        req: &ExternalRequest<'_>,
    ) -> Result<Child, ExternalError> {
        self.inner().launch(staged, req)
    }

    fn parse_stats(&self, text: &str) -> Stats {
        self.inner().parse_stats(text)
    }

    fn campaign(&self, req: &ExternalRequest<'_>) -> Result<ExternalOutcome, ExternalError> {
        self.inner().campaign(req)
    }
}

impl From<AnyFuzzer> for AnyDelegatedEngine {
    fn from(value: AnyFuzzer) -> Self {
        Self::Fuzzer(value)
    }
}
