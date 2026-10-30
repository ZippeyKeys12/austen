use crate::{PreparedRadamsa, RadamsaFactory, RadamsaPrepare};
use austen_lib::gen_dsl::passes::PassProfile;
use austen_lib::wire::Trust;

use austen_lib::campaign::{CampaignConfig, CampaignError, GeneratorPhase};
use austen_lib::engine::LaunchCandidateGate;
use austen_lib::findings::ArtifactFolders;
use austen_lib::generator::{GeneratorBuild, GeneratorFactory, PreparedGenerator};
use austen_lib::input::Generator;

fn generator_failure(phase: GeneratorPhase, error: impl std::fmt::Display) -> CampaignError {
    CampaignError::Generator {
        generator: RadamsaFactory.name(),
        phase,
        message: error.to_string(),
    }
}

impl GeneratorFactory for RadamsaFactory {
    fn descriptor(&self) -> austen_lib::engine::EngineDescriptor {
        austen_lib::engine::RAW_GENERATOR_DESCRIPTOR
    }

    fn name(&self) -> &'static str {
        RadamsaFactory::name(self)
    }

    fn display_name(&self) -> &'static str {
        RadamsaFactory::display_name(self)
    }

    fn profiles(&self) -> Vec<PassProfile> {
        RadamsaFactory::profiles(self)
    }

    fn prepare(
        &self,
        cfg: &CampaignConfig,
        journal: &mut austen_lib::coverage::Journal,
    ) -> Result<Box<dyn PreparedGenerator>, CampaignError> {
        let folders = ArtifactFolders::new(&cfg.corpus, &cfg.crashes);
        let seed_sources = folders.launch_seed_sources();
        let protected_paths = cfg.output_paths(&folders);
        let function_ids = cfg
            .functions
            .iter()
            .map(|function| function.target_id)
            .collect::<Vec<_>>();
        let gate = LaunchCandidateGate::new(cfg.validator.as_ref(), &cfg.reporter, journal);
        let prepared = RadamsaFactory::prepare(
            self,
            RadamsaPrepare {
                work: &cfg.work,
                seed: cfg.seed,
                function_ids: &function_ids,
                seed_sources: &seed_sources,
                protected_paths: &protected_paths,
                reporter: &cfg.reporter,
                candidate_gate: &gate,
            },
        )
        .map_err(|error| generator_failure(GeneratorPhase::Prepare, error))?;
        Ok(Box::new(prepared))
    }
}

impl PreparedGenerator for PreparedRadamsa {
    fn name(&self) -> &'static str {
        PreparedRadamsa::name(self)
    }

    fn trust(&self) -> Trust {
        PreparedRadamsa::trust(self)
    }

    fn open(self: Box<Self>, _build: GeneratorBuild) -> Result<Box<dyn Generator>, CampaignError> {
        PreparedRadamsa::open(*self).map_err(|error| generator_failure(GeneratorPhase::Open, error))
    }
}
