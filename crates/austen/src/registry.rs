#[cfg(feature = "aflpp")]
use austen_aflpp::{AflCmin, AflPlusPlus, AflTmin};
#[cfg(feature = "bennet")]
use austen_bennet::BennetFactory;
#[cfg(feature = "darcy")]
use austen_darcy::DarcyFactory;
#[cfg(feature = "honggfuzz")]
use austen_honggfuzz::Honggfuzz;
use austen_lib::engine::EngineRegistry;
#[cfg(any(feature = "aflpp", feature = "honggfuzz", feature = "libfuzzer"))]
use austen_lib::external::{AnyDelegatedEngine, AnyFuzzer};
use austen_lib::generator::AnyGenerator;
#[cfg(feature = "radamsa")]
use austen_lib::generator::GeneratorFactoryHandle;
use austen_lib::shrink::{AnyShrinkTool, AnyShrinker, HarnessKind, ShrinkTool, SuiteToolHandle};
use austen_lib::{AnyEngine, AnyTreeEngine, Engine, Schedule};
#[cfg(feature = "libfuzzer")]
use austen_libfuzzer::{LibFuzzer, LibFuzzerMinimize};
#[cfg(feature = "radamsa")]
use austen_radamsa::RadamsaFactory;
#[cfg(feature = "wickham")]
use austen_wickham::WickhamFactory;

#[cfg(feature = "wickham")]
pub const WICKHAM: AnyTreeEngine = AnyTreeEngine::new(&WickhamFactory);
#[cfg(feature = "bennet")]
pub const BENNET: AnyTreeEngine = AnyTreeEngine::new(&BennetFactory);
#[cfg(feature = "darcy")]
pub const DARCY: AnyTreeEngine = AnyTreeEngine::new(&DarcyFactory);
#[cfg(feature = "aflpp")]
pub const AFLPP: AnyFuzzer = AnyFuzzer::new(&AflPlusPlus);
#[cfg(feature = "honggfuzz")]
pub const HONGGFUZZ_AUTO: AnyFuzzer = AnyFuzzer::new(&Honggfuzz::auto());
#[cfg(feature = "honggfuzz")]
pub const HONGGFUZZ_PERSISTENT: AnyFuzzer = AnyFuzzer::new(&Honggfuzz::persistent());
#[cfg(feature = "honggfuzz")]
pub const HONGGFUZZ_STDIN: AnyFuzzer = AnyFuzzer::new(&Honggfuzz::stdin());
#[cfg(feature = "libfuzzer")]
pub const LIBFUZZER_DIRECT: AnyFuzzer = AnyFuzzer::new(&LibFuzzer::with_fork(false));
#[cfg(feature = "libfuzzer")]
pub const LIBFUZZER_FORK: AnyFuzzer = AnyFuzzer::new(&LibFuzzer::with_fork(true));

const ENGINES: &[AnyEngine] = &[
    #[cfg(feature = "wickham")]
    AnyEngine::AustenDriven(AnyGenerator::Tree(WICKHAM)),
    #[cfg(feature = "bennet")]
    AnyEngine::AustenDriven(AnyGenerator::Tree(BENNET)),
    #[cfg(feature = "darcy")]
    AnyEngine::AustenDriven(AnyGenerator::Tree(DARCY)),
    #[cfg(feature = "radamsa")]
    AnyEngine::AustenDriven(AnyGenerator::Other(GeneratorFactoryHandle::new(
        &RadamsaFactory,
    ))),
    #[cfg(feature = "aflpp")]
    AnyEngine::Delegated(AnyDelegatedEngine::Fuzzer(AFLPP)),
    #[cfg(feature = "honggfuzz")]
    AnyEngine::Delegated(AnyDelegatedEngine::Fuzzer(HONGGFUZZ_AUTO)),
    #[cfg(feature = "libfuzzer")]
    AnyEngine::Delegated(AnyDelegatedEngine::Fuzzer(LIBFUZZER_DIRECT)),
];

static REGISTRY: EngineRegistry = EngineRegistry::new(ENGINES);

pub fn engines() -> &'static EngineRegistry {
    &REGISTRY
}

pub fn default_tree() -> Option<AnyTreeEngine> {
    ["bennet", "darcy", "wickham"]
        .into_iter()
        .find_map(|name| engines().resolve_tree(name))
}

pub fn default_engine() -> Option<AnyEngine> {
    default_tree().map(|tree| AnyEngine::AustenDriven(AnyGenerator::Tree(tree)))
}

pub const RECOMMENDED_INSTALL: &str = "cargo install austen --features bennet,darcy";

pub fn default_schedule() -> Option<Schedule> {
    if engines().resolve_tree("bennet").is_some() && engines().resolve_tree("darcy").is_some() {
        Some(Schedule::parse("bennet/darcy", engines()).expect("both default chain members exist"))
    } else {
        default_engine().map(Schedule::single)
    }
}

pub fn default_selection_label() -> String {
    default_schedule().map_or_else(
        || "no engine".to_owned(),
        |schedule| schedule.selection_label(),
    )
}

pub const DEFAULT_CHAIN_DISCARD_LIMIT: std::num::NonZeroU64 =
    std::num::NonZeroU64::new(10).unwrap();

pub const NEUTRAL_DESCRIPTOR: austen_lib::EngineDescriptor = austen_lib::EngineDescriptor {
    generic_seed: false,
    max_array_length: true,
    ..austen_lib::engine::RAW_GENERATOR_DESCRIPTOR
};

pub fn missing_engine_feature(name: &str) -> Option<&'static str> {
    let (feature, enabled) = match name {
        "bennet" => ("bennet", cfg!(feature = "bennet")),
        "darcy" => ("darcy", cfg!(feature = "darcy")),
        "wickham" => ("wickham", cfg!(feature = "wickham")),
        "radamsa" => ("radamsa", cfg!(feature = "radamsa")),
        "afl++" => ("aflpp", cfg!(feature = "aflpp")),
        "honggfuzz" => ("honggfuzz", cfg!(feature = "honggfuzz")),
        "libfuzzer" | "libfuzzer-fork" => ("libfuzzer", cfg!(feature = "libfuzzer")),
        _ => return None,
    };
    (!enabled).then_some(feature)
}

pub fn generator_names() -> impl Iterator<Item = &'static str> {
    engines().iter().filter_map(|engine| match engine {
        AnyEngine::AustenDriven(_) => Some(engine.name()),
        AnyEngine::Delegated(_) => None,
    })
}

#[cfg(feature = "aflpp")]
pub const AFL_TMIN: AnyShrinkTool = AnyShrinkTool::new(&AflTmin);
#[cfg(feature = "libfuzzer")]
pub const LIBFUZZER_MINIMIZE: AnyShrinkTool = AnyShrinkTool::new(&LibFuzzerMinimize);

pub fn shrink_tools() -> impl Iterator<Item = AnyShrinkTool> {
    const TOOLS: &[AnyShrinkTool] = &[
        #[cfg(feature = "aflpp")]
        AFL_TMIN,
        #[cfg(feature = "libfuzzer")]
        LIBFUZZER_MINIMIZE,
    ];
    TOOLS.iter().copied()
}

pub fn shrinker_names() -> impl Iterator<Item = &'static str> {
    shrink_tools().map(|tool| ShrinkTool::name(&tool))
}

pub fn resolve_shrinker(name: &str) -> Option<AnyShrinker> {
    shrink_tools()
        .find(|tool| ShrinkTool::name(tool) == name)
        .map(AnyShrinker::Tool)
}

pub fn shrinker_for_harness(kind: HarnessKind) -> Option<AnyShrinker> {
    match kind {
        HarnessKind::Forkserver => resolve_shrinker("afl-tmin"),
        HarnessKind::InProcess => resolve_shrinker("libfuzzer"),
    }
}

pub fn suite_minimizer() -> Option<SuiteToolHandle> {
    const TOOLS: &[SuiteToolHandle] = &[
        #[cfg(feature = "aflpp")]
        SuiteToolHandle::new(&AflCmin),
    ];
    TOOLS.first().copied()
}

pub fn shrinkers() -> impl Iterator<Item = AnyShrinker> {
    shrink_tools().map(AnyShrinker::Tool)
}
