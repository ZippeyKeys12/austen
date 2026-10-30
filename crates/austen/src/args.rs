use std::collections::BTreeSet;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::time::Duration;

use austen_lib::driver::{
    DEFAULT_MAX_GENERATOR_SIZE, InlineMode, MAX_MAP_ITERATIONS, PassOptions, TestCaseSize,
};
use austen_lib::shrink::AnyShrinker;
use austen_lib::spec::sanity::DiagnosticCode;
use austen_lib::wire::AustenInput;
use austen_lib::{
    EngineDescriptor, FunctionSelectionOwner, FunctionSelectorKind, Schedule, ScheduleError,
    SizeSelectorKind, Stage,
};

#[cfg(any(feature = "honggfuzz", feature = "libfuzzer"))]
use austen_lib::{AnyDelegatedEngine, AnyEngine, AnyFuzzer};

use crate::render::TRACE_INPUTS_LEVEL;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Object(PathBuf),
    Archive(PathBuf),
    CSource(PathBuf),
}

impl Target {
    pub fn classify(path: PathBuf) -> Result<Self, Usage> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("o") => Ok(Target::Object(path)),
            Some("a") => Ok(Target::Archive(path)),
            Some("c") => Ok(Target::CSource(path)),
            _ => Err(Usage::new(format!(
                "{}: expected a target ending in .o, .a, or .c",
                path.display()
            ))),
        }
    }

    pub fn path(&self) -> &PathBuf {
        match self {
            Target::Object(p) | Target::Archive(p) | Target::CSource(p) => p,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    Test(Box<TestArgs>),
    Sanity(SanityArgs),
    Help,
    Version,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum SanityFormat {
    #[default]
    Text,
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanityArgs {
    pub spec: PathBuf,
    pub format: SanityFormat,
    pub function: Option<String>,
    pub pedantic: bool,
    pub suppress: BTreeSet<DiagnosticCode>,
    pub fail_on_warning: bool,
}

fn clock_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestMode {
    Campaign {
        duration_secs: Option<u64>,
        seed: u64,
        record: Option<PathBuf>,
    },
    Replay {
        tape: PathBuf,
        record: Option<PathBuf>,
    },
    SingleInput {
        source: InputSource,
    },
    Build,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputSource {
    Inline(AustenInput),
    Stdin,
}

impl TestMode {
    pub fn duration_secs(&self) -> Option<u64> {
        match self {
            TestMode::Campaign { duration_secs, .. } => *duration_secs,
            TestMode::Build => None,
            TestMode::Replay { .. } | TestMode::SingleInput { .. } => None,
        }
    }

    pub fn draws_entropy(&self) -> bool {
        !matches!(self, TestMode::SingleInput { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShrinkChoice {
    Default,
    Named(AnyShrinker),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum CoverageMode {
    #[value(name = "edge")]
    Edge,
    #[value(name = "bb")]
    BasicBlock,
    #[value(name = "func")]
    Function,
    #[value(name = "trace-pc-guard")]
    TracePcGuard,
    #[value(name = "inline-8bit-counters")]
    Inline8BitCounters,
    #[value(name = "trace-cmp")]
    TraceCmp,
    #[value(name = "trace-div")]
    TraceDiv,
    #[value(name = "trace-gep")]
    TraceGep,
    #[value(name = "indirect-calls")]
    IndirectCalls,
    #[value(name = "stack-depth")]
    StackDepth,
    #[value(name = "trace-pc")]
    TracePc,
    #[value(name = "function-trace")]
    FunctionTrace,
    #[value(name = "function-trace-after-inlining")]
    FunctionTraceAfterInlining,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum HonggfuzzLaunchMode {
    Auto,
    Persistent,
    Stdin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum FunctionSelectorMode {
    #[value(name = "random")]
    Random,
    #[value(name = "round-robin")]
    RoundRobin,
}

impl FunctionSelectorMode {
    const fn kind(self) -> FunctionSelectorKind {
        match self {
            FunctionSelectorMode::Random => FunctionSelectorKind::Random,
            FunctionSelectorMode::RoundRobin => FunctionSelectorKind::RoundRobin,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum SizingStrategyMode {
    #[value(name = "uniform")]
    Uniform,
    #[value(name = "quickcheck")]
    QuickCheck,
    #[value(name = "constant")]
    Constant,
}

impl SizingStrategyMode {
    const fn kind(self) -> SizeSelectorKind {
        match self {
            SizingStrategyMode::Uniform => SizeSelectorKind::Uniform,
            SizingStrategyMode::QuickCheck => SizeSelectorKind::QuickCheck,
            SizingStrategyMode::Constant => SizeSelectorKind::Constant,
        }
    }
}

#[cfg(feature = "honggfuzz")]
impl HonggfuzzLaunchMode {
    const fn adapter(self) -> AnyFuzzer {
        match self {
            HonggfuzzLaunchMode::Auto => crate::registry::HONGGFUZZ_AUTO,
            HonggfuzzLaunchMode::Persistent => crate::registry::HONGGFUZZ_PERSISTENT,
            HonggfuzzLaunchMode::Stdin => crate::registry::HONGGFUZZ_STDIN,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum LibFuzzerLaunchMode {
    Direct,
    Fork,
}

#[cfg(feature = "libfuzzer")]
impl LibFuzzerLaunchMode {
    const fn adapter(self) -> AnyFuzzer {
        match self {
            LibFuzzerLaunchMode::Direct => crate::registry::LIBFUZZER_DIRECT,
            LibFuzzerLaunchMode::Fork => crate::registry::LIBFUZZER_FORK,
        }
    }
}

impl CoverageMode {
    pub const fn token(self) -> &'static str {
        match self {
            CoverageMode::Edge => "edge",
            CoverageMode::BasicBlock => "bb",
            CoverageMode::Function => "func",
            CoverageMode::TracePcGuard => "trace-pc-guard",
            CoverageMode::Inline8BitCounters => "inline-8bit-counters",
            CoverageMode::TraceCmp => "trace-cmp",
            CoverageMode::TraceDiv => "trace-div",
            CoverageMode::TraceGep => "trace-gep",
            CoverageMode::IndirectCalls => "indirect-calls",
            CoverageMode::StackDepth => "stack-depth",
            CoverageMode::TracePc => "trace-pc",
            CoverageMode::FunctionTrace => "function-trace",
            CoverageMode::FunctionTraceAfterInlining => "function-trace-after-inlining",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestArgs {
    pub spec: PathBuf,
    pub target: Option<Target>,
    pub harness: Option<PathBuf>,
    pub harness_manifest: Option<PathBuf>,
    pub cflags: Vec<String>,
    pub coverage: Vec<CoverageMode>,
    pub no_coverage: bool,
    pub engine: Option<Schedule>,
    selected: Option<Schedule>,
    pub function_selector: FunctionSelectorKind,
    pub size_selector: SizeSelectorKind,
    pub max_generator_size: TestCaseSize,
    pub generator_timeout: Option<Duration>,
    pub chain_discard_limit: Option<NonZeroU64>,
    pub corpus_snapshot_interval: Option<Duration>,
    pub mode: TestMode,
    pub function: Option<String>,
    pub ignore_custom_generators: bool,
    pub crashes: Option<PathBuf>,
    pub shrink: Option<ShrinkChoice>,
    pub minimize_suite: bool,
    pub work: Option<PathBuf>,
    pub dsl_log_dir: Option<PathBuf>,
    pub pass_options: PassOptions,
    pub check_heap_ownership: bool,
    pub allow_unstable_addresses: bool,
    pub report: Option<PathBuf>,
    pub stats: Vec<StatBlock>,
    pub tyche: Option<PathBuf>,
    pub exit_fast: bool,
    pub strict_duration: bool,
    pub fail_on_crash: bool,
    pub clean: bool,
    pub output: crate::render::Output,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum StatBlock {
    Backtracks,
    Satisfaction,
    Sizes,
    Discards,
    Timing,
    All,
}

impl TestArgs {
    pub fn schedule(&self) -> Option<&Schedule> {
        self.selected.as_ref()
    }

    pub fn stages(&self) -> &[Stage] {
        self.selected.as_ref().map_or(&[], Schedule::stages)
    }

    pub fn is_single_stage(&self) -> bool {
        self.selected.as_ref().is_none_or(Schedule::is_single)
    }

    pub fn canonical_engine(&self) -> String {
        self.selected
            .as_ref()
            .map(Schedule::canonical_engine)
            .unwrap_or_default()
    }

    pub fn descriptor(&self) -> EngineDescriptor {
        self.selected
            .as_ref()
            .map_or(crate::registry::NEUTRAL_DESCRIPTOR, Schedule::descriptor)
    }

    pub fn reported_function_selector(&self) -> Option<FunctionSelectorKind> {
        (self.descriptor().function_selection == FunctionSelectionOwner::Austen
            && matches!(self.mode, TestMode::Campaign { .. } | TestMode::Build))
        .then_some(self.function_selector)
    }

    pub fn reported_size_selector(&self) -> Option<SizeSelectorKind> {
        (self.descriptor().producer_options
            && matches!(self.mode, TestMode::Campaign { .. } | TestMode::Build))
        .then_some(self.size_selector)
    }

    pub fn reported_max_generator_size(&self) -> Option<TestCaseSize> {
        (self.descriptor().producer_options
            && matches!(self.mode, TestMode::Campaign { .. } | TestMode::Build))
        .then_some(self.max_generator_size)
    }

    pub fn reported_generator_timeout(&self) -> GeneratorTimeout {
        if !(self.descriptor().producer_options
            && matches!(self.mode, TestMode::Campaign { .. } | TestMode::Build))
        {
            return GeneratorTimeout::Inapplicable;
        }
        match self.generator_timeout {
            Some(after) => GeneratorTimeout::After(after),
            None => GeneratorTimeout::Disabled,
        }
    }

    pub fn reported_chain_discard_limit(&self) -> Option<NonZeroU64> {
        self.chain_discard_limit
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratorTimeout {
    Inapplicable,
    Disabled,
    After(Duration),
}

impl GeneratorTimeout {
    pub fn applied(self) -> Option<Duration> {
        match self {
            Self::After(after) => Some(after),
            Self::Inapplicable | Self::Disabled => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct Usage {
    pub message: String,
}

impl Usage {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn from_message(message: impl Into<String>) -> Self {
        Self::new(message)
    }
}

pub const USAGE: &str = "\
usage:
  austen test [OPTIONS] <spec.json> [<target.o | target.a | target.c>]
  austen sanity [OPTIONS] <spec.json>
  austen help | --help | --version

sanity options:
  --format=<format>  text (default) or json
  --function=<name>  analyze only this specified function       [all functions]
  --pedantic         include opt-in diagnostics such as unused inputs
  --suppress=<code>  suppress one diagnostic code; repeat for more codes
  --fail-on-warning  exit 1 when unsuppressed warnings are found

test options:
  --harness=<path>   run an already-built harness
  --harness-manifest=<path>
                     use build metadata for --harness, including its
                     evaluation harness and effective instrumentation
  --cflag=<arg>      append one target-compiler argument after austen's
                     defaults; repeat for more arguments. Coverage settings
                     are checked against the selected engine. Applied to
                     campaign, shrink and common-evaluation rebuilds. Requires
                     a .c target
  --coverage=<mode>  add coverage to the primary engine build; repeat for more.
                     Shapes: edge, bb, func. Feedback: trace-pc-guard,
                     inline-8bit-counters, trace-cmp, trace-div, trace-gep,
                     indirect-calls, stack-depth, trace-pc, function-trace,
                     function-trace-after-inlining. Requires a .c target;
                     not inherited by shrink or common-evaluation builds.
                     Omit it to request the minimum: trace-pc-guard for Austen-driven,
                     AFL++ and honggfuzz; inline counters for libFuzzer.
                     Default Austen-driven runs fall back to a plain build only
                     when their coverage clang is unavailable
  --no-coverage      skip common-evaluation coverage building and replay;
                     retain the selected engine's feedback instrumentation.
                     Not with --minimize-suite
  --engine=<schedule>
                     what drives the run: an enabled engine, a chain of
                     tree engines joined by / (each escalates to
                     the next once it has given up on a function), or stages
                     joined by , each with :seconds (the last may omit it and
                     run until interrupted), run in order with every stage's
                     retained inputs seeding a later delegated stage. At most
                     one delegated stage. Availability depends on this build's
                     Cargo features; see the enabled engines below
  --honggfuzz-mode=<mode>
                     honggfuzz transport: auto (default), persistent, or
                     stdin. Persistent is strict -P with no fallback; stdin
                     starts one process per input. Requires
                     --engine=honggfuzz when supplied
  --libfuzzer-mode=<mode>
                     libFuzzer loop: direct (default) or fork. Fork runs under
                     its supervisor, so a crash costs one worker rather than
                     ending the campaign. Requires --engine=libfuzzer when
                     supplied
  --duration=<time>  how long to keep testing, for any engine: seconds, or
                     a span like 90s, 2m, 1h. Omit it — or pass 0, which
                     every fuzzer already reads that way — to keep testing
                     until you stop it (Ctrl-C), which is what a plain
                     `austen test` does. To build without testing at all,
                     see --build-only. Not with a multi-stage --engine,
                     whose stages state their own budgets.
  --corpus-snapshot-interval=<time>
                     coverage checkpoint grid, and live-corpus polling for
                     delegated engines: seconds, or a span like 30s, 2m. By
                     default uses 1% of --duration, clamped to 1s..15m;
                     unbounded runs use 15m
  --build-only       build, link and stage everything; test nothing
  --seed=<N>         Austen-driven generator seed         [current time, printed]
  --input=<bytes>    run one exact wire input instead of generating. Use
                     0x followed by an even number of hexadecimal digits, or
                     - to read raw binary bytes from stdin. Excludes function
                     and engine selection, campaign-only coverage, budgets,
                     seeds, recording and replay.
  --record=<path>    write the entropy each tested input consumed, one
                     base64 line per input, appended as the run goes
  --replay=<path>    take entropy from a recording instead of a seeded
                     stream, and re-run exactly the inputs it holds. Cut one
                     line out (behind the header) to run just that input.
                     Only a --record file is accepted, never a raw harness
                     input. Excludes --seed and --duration.
  --function=<name>  test only this specified function       [all functions]
  --ignore-custom-generators
                     use specification-derived generators instead of any
                     custom generator files named by the specification
  --function-selector=<mode>
                     choose active functions randomly (default) or in
                     round-robin source order; tree campaigns and builds
  --sizing-strategy=<mode>
                     how a tree campaign picks each input's size: uniform
                     (default), quickcheck (QuickCheck's ramp, small first),
                     or constant; tree campaigns and builds only
  --max-generator-size=<N>
                     select each tree-generated test-case size uniformly from
                     1..=N; tree campaigns and builds only                 [100]
  --generator-timeout=<ms>
                     discard one input's generation if it runs longer than
                     this and start another; 0 disables it; tree campaigns
                     only                                               [1000]
  --chain-discard-limit=<N>
                     for a single-stage tree-engine chain, retire each
                     non-last member per function after N cumulative
                     exhausted generation requests, even if it produced
                     inputs; N must be positive. The implicit bennet/darcy
                     chain defaults to 10; explicitly selected chains keep
                     the 100-exhaustion, zero-input policy without this flag
  --inline=<mode>    tree-generator call inlining: nothing (default),
                     nonrec, semirec, or everything
  --max-unfolds=<N>  unfold tree-generator calls to depth N; omitted
                     disables it, and N = 0 is the identity
  --path-pruning     prune inconsistent tree-generator paths using normalized
                     path conditions; disabled by default
  --smt-pruning      prune inconsistent tree-generator paths using scalar SMT
                     reasoning; disabled by default
  --max-array-length=<N>
                     bound the length of tree-generated arrays — CN's
                     --max-array-length, read by tree engines            [50]
  --experimental-product-arg-destruction
                     flatten aggregate generator arguments
  --experimental-return-pruning
                     specialize generators for the return values callers use
  --experimental-arg-pruning
                     remove unused generator arguments
  --sink-args        sink a draw passed to one tree-generator call into a
                     specialized callee, with its local constraints;
                     disabled by default
  --specialize-guarded-choices
                     decide a tree-generator Boolean selector from the entry
                     guards of its alternatives, computed before the draw;
                     wickham and bennet interpret it, darcy does not; disabled
                     by default
  --shrink[=<name>]  after the campaign, reduce every eligible crashing input
                     filed to the smallest byte string that still kills
                     the target, verified by re-running it. Attempts every
                     eligible input with no cap. Excludes --input and
                     --build-only, which run no campaign.
                     Bare, the minimizer that drives the harness this run
                     already builds; named, that one — building it a
                     harness of its own when the campaign's will not do.
  --minimize-suite   reduce each classified outcome separately with AFL's
                     corpus minimizer. With --shrink: minimize, shrink the
                     surviving crashes, then re-minimize crash suites from
                     originals plus reductions. Independent of --shrink.
                     Excludes --input and --build-only, which produce no
                     campaign suite, and --harness, which supplies no common
                     target recipe. Also excludes --no-coverage.
  --crashes=<dir>    crash outcome root                         [./crashes]
  --work=<dir>       run directory              [<cache>/austen/runs/<key>]
  --dsl-log-dir=<dir>
                     write CN-style gen-dsl snapshots for every conversion pass
  --check-heap-ownership
                     require a certified allocator boundary; fail on unowned
                     postcondition takes or unclaimed heap bytes;
                     default: no allocator rewriting or leak checking
  --allow-unstable-addresses
                     allow a dynamic heap if the default address is unavailable;
                     saved absolute-pointer inputs may not replay across runs.
                     Explicit nonzero AUSTEN_HEAP_BASE remains mandatory
  --report=<path>    write a JSON summary, including validation and coverage
  --stats=<list>     print CN statistics: backtracks,satisfaction,sizes,
                     discards,timing,all; comma-separated, default off
  --tyche=<path>     write per-attempt Tyche JSON lines; Austen-driven runs,
                     --input and --replay; excludes --build-only
  --exit-fast        stop a single-stage Austen-driven live campaign after its
                     first target kill; validation and reporting still run
  --strict-duration  make an explicit duration a strict pre-dispatch boundary;
                     a generated input that completes late is not tested or kept
  --fail-on-crash    exit 1 when crashes are found (default: exit 0)
  --clean            wipe the run directory before building
  --progress=<when>  auto | always | never — dynamic progress on a
                     terminal, line-oriented otherwise               [auto]
  --color=<when>     auto | always | never (honours NO_COLOR)        [auto]
  --events=<path>    stream the run as JSON lines; `-` is stdout
  -q, --quiet        only print the summary
  -v, -vv, -vvv      build commands and tool lifecycle; then protocol
                     diagnostics; then a line per input (small runs only)
      --trace-inputs the same as -vvv
";

pub fn usage() -> String {
    let names = crate::registry::engines().names().collect::<Vec<_>>();
    let enabled = if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    };
    let default = crate::registry::default_schedule()
        .map(|schedule| {
            let name = schedule.canonical_engine();
            if schedule.single_chain().is_some() {
                format!(
                    "{name} (discard limit {})",
                    crate::registry::DEFAULT_CHAIN_DISCARD_LIMIT
                )
            } else {
                name
            }
        })
        .unwrap_or_else(|| "none; generation requires --engine".to_owned());
    format!("{USAGE}\nenabled engines: {enabled}\ndefault engine: {default}\n")
}

#[derive(Debug, clap::Parser)]
#[command(
    name = "austen sanity",
    bin_name = "austen sanity",
    disable_help_flag = true,
    disable_version_flag = true
)]
struct RawSanity {
    spec: PathBuf,
    #[arg(long, value_name = "format")]
    format: Option<SanityFormat>,
    #[arg(long, value_name = "name")]
    function: Option<String>,
    #[arg(long)]
    pedantic: bool,
    #[arg(
        long,
        value_name = "code",
        action = clap::ArgAction::Append,
        value_parser = clap::builder::PossibleValuesParser::new(
            DiagnosticCode::ALL.map(DiagnosticCode::as_str),
        ),
    )]
    suppress: Vec<String>,
    #[arg(long)]
    fail_on_warning: bool,
}

impl TryFrom<RawSanity> for SanityArgs {
    type Error = Usage;

    fn try_from(raw: RawSanity) -> Result<Self, Self::Error> {
        let suppress = raw
            .suppress
            .iter()
            .filter_map(|token| token.parse::<DiagnosticCode>().ok())
            .collect();
        Ok(Self {
            spec: raw.spec,
            format: raw.format.unwrap_or_default(),
            function: raw.function,
            pedantic: raw.pedantic,
            suppress,
            fail_on_warning: raw.fail_on_warning,
        })
    }
}

#[derive(Debug, clap::Parser)]
#[command(
    name = "austen test",
    bin_name = "austen test",
    disable_help_flag = true,
    disable_version_flag = true
)]
struct RawTest {
    spec: PathBuf,
    target: Option<PathBuf>,
    #[arg(long, value_name = "path")]
    harness: Option<PathBuf>,
    #[arg(long, value_name = "path", require_equals = true)]
    harness_manifest: Option<PathBuf>,
    #[arg(
        long,
        value_name = "arg",
        action = clap::ArgAction::Append,
        require_equals = true,
        allow_hyphen_values = true
    )]
    cflag: Vec<String>,
    #[arg(
        long,
        value_name = "mode",
        action = clap::ArgAction::Append,
        require_equals = true
    )]
    coverage: Vec<CoverageMode>,
    #[arg(long)]
    no_coverage: bool,
    #[arg(long, value_name = "name")]
    engine: Option<String>,
    #[arg(long, value_name = "mode", require_equals = true)]
    honggfuzz_mode: Option<HonggfuzzLaunchMode>,
    #[arg(long, value_name = "mode", require_equals = true)]
    libfuzzer_mode: Option<LibFuzzerLaunchMode>,
    #[arg(long, value_name = "time")]
    duration: Option<String>,
    #[arg(long, value_name = "time")]
    corpus_snapshot_interval: Option<String>,
    #[arg(long)]
    build_only: bool,
    #[arg(long, value_name = "N")]
    seed: Option<u64>,
    #[arg(long, value_name = "bytes", allow_hyphen_values = true)]
    input: Option<String>,
    #[arg(long, value_name = "path")]
    record: Option<PathBuf>,
    #[arg(long, value_name = "path")]
    replay: Option<PathBuf>,
    #[arg(long, value_name = "name")]
    function: Option<String>,
    #[arg(long)]
    ignore_custom_generators: bool,
    #[arg(long, value_name = "mode", require_equals = true)]
    function_selector: Option<FunctionSelectorMode>,
    #[arg(long, value_name = "mode", require_equals = true)]
    sizing_strategy: Option<SizingStrategyMode>,
    #[arg(long, value_name = "N")]
    max_generator_size: Option<String>,
    #[arg(long, value_name = "ms")]
    generator_timeout: Option<String>,
    #[arg(long, value_name = "N")]
    chain_discard_limit: Option<String>,
    #[arg(long, value_name = "mode")]
    inline: Option<String>,
    #[arg(long, value_name = "N")]
    max_unfolds: Option<String>,
    #[arg(long)]
    path_pruning: bool,
    #[arg(long)]
    smt_pruning: bool,
    #[arg(long, value_name = "N")]
    max_array_length: Option<String>,
    #[arg(long)]
    experimental_product_arg_destruction: bool,
    #[arg(long)]
    experimental_return_pruning: bool,
    #[arg(long)]
    experimental_arg_pruning: bool,
    #[arg(long)]
    sink_args: bool,
    #[arg(long)]
    specialize_guarded_choices: bool,
    #[arg(long, value_name = "dir")]
    crashes: Option<PathBuf>,
    #[arg(long, num_args = 0..=1, require_equals = true, value_name = "name")]
    shrink: Option<Option<String>>,
    #[arg(long)]
    minimize_suite: bool,
    #[arg(long, value_name = "dir")]
    work: Option<PathBuf>,
    #[arg(long, value_name = "dir")]
    dsl_log_dir: Option<PathBuf>,
    #[arg(long)]
    check_heap_ownership: bool,
    #[arg(long)]
    allow_unstable_addresses: bool,
    #[arg(long, value_name = "path")]
    report: Option<PathBuf>,
    #[arg(
        long,
        value_name = "list",
        value_delimiter = ',',
        require_equals = true
    )]
    stats: Vec<StatBlock>,
    #[arg(long, value_name = "path")]
    tyche: Option<PathBuf>,
    #[arg(long)]
    exit_fast: bool,
    #[arg(long)]
    strict_duration: bool,
    #[arg(long)]
    fail_on_crash: bool,
    #[arg(long)]
    clean: bool,
    #[arg(long = "count", value_name = "N")]
    count: Option<Option<String>>,
    #[arg(short = 'n', value_name = "N")]
    count_short: Option<Option<String>>,
    #[arg(long, value_name = "when")]
    progress: Option<String>,
    #[arg(long, alias = "colour", value_name = "when")]
    color: Option<String>,
    #[arg(long, value_name = "path")]
    events: Option<PathBuf>,
    #[arg(short = 'q', long)]
    quiet: bool,
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count)]
    verbose: u8,
    #[arg(long)]
    trace_inputs: bool,
}

pub fn parse(argv: &[String]) -> Result<Cmd, Usage> {
    match argv.first().map(String::as_str) {
        Some("test") => TestArgs::try_from(sub::<RawTest>("austen test", &argv[1..])?)
            .map(|args| Cmd::Test(Box::new(args))),
        Some("sanity") => {
            SanityArgs::try_from(sub::<RawSanity>("austen sanity", &argv[1..])?).map(Cmd::Sanity)
        }
        Some("help" | "--help" | "-h") | None => Ok(Cmd::Help),
        Some("--version" | "-V") => Ok(Cmd::Version),
        Some(other) => Err(Usage::new(format!("unknown command `{other}`"))),
    }
}

fn sub<R: clap::Parser>(bin: &'static str, rest: &[String]) -> Result<R, Usage> {
    R::try_parse_from(std::iter::once(bin).chain(rest.iter().map(String::as_str)))
        .map_err(from_clap)
}

fn from_clap(mut err: clap::Error) -> Usage {
    err.remove(clap::error::ContextKind::Usage);
    let text = err.to_string();
    Usage::new(text.strip_prefix("error: ").unwrap_or(&text).trim_end())
}

fn when(key: &str, value: Option<&str>) -> Result<crate::render::When, Usage> {
    let Some(value) = value else {
        return Ok(crate::render::When::default());
    };
    value.parse().map_err(|()| {
        Usage::new(format!(
            "{key}: expected auto, always, or never, got `{value}`"
        ))
    })
}

fn parse_inline(value: Option<&str>) -> Result<InlineMode, Usage> {
    let Some(value) = value else {
        return Ok(InlineMode::Nothing);
    };
    InlineMode::from_token(value).ok_or_else(|| {
        let names: Vec<&str> = InlineMode::SPELLINGS
            .iter()
            .map(|(name, _)| *name)
            .collect();
        let (last, rest) = names.split_last().expect("SPELLINGS is non-empty");
        Usage::new(format!(
            "--inline: expected {}, or {last}, got `{value}`",
            rest.join(", ")
        ))
    })
}

fn parse_max_unfolds(value: Option<&str>) -> Result<Option<usize>, Usage> {
    let Some(value) = value else {
        return Ok(None);
    };
    let parsed = value
        .parse::<i64>()
        .map_err(|_| Usage::new(format!("--max-unfolds: expected an integer, got `{value}`")))?;
    if parsed < 0 {
        return Err(Usage::new(format!(
            "--max-unfolds: negative values are not supported, got `{value}`"
        )));
    }
    usize::try_from(parsed).map(Some).map_err(|_| {
        Usage::new(format!(
            "--max-unfolds={value} is too large for this platform"
        ))
    })
}

fn parse_max_array_length(value: Option<&str>) -> Result<usize, Usage> {
    let Some(value) = value else {
        return Ok(PassOptions::NONE.max_array_length);
    };
    let parsed = value.parse::<usize>().map_err(|_| {
        Usage::new(format!(
            "--max-array-length: expected a positive integer, got `{value}`"
        ))
    })?;
    if parsed == 0 {
        return Err(Usage::new(
            "--max-array-length=0 is not a spelling of 'unbounded': the bound \
             is also the solver engine's candidate-pool size, so write a \
             larger N instead"
                .to_string(),
        ));
    }
    if parsed > MAX_MAP_ITERATIONS as usize {
        return Err(Usage::new(format!(
            "--max-array-length={parsed} exceeds the wire map limit of {MAX_MAP_ITERATIONS}"
        )));
    }
    Ok(parsed)
}

fn parse_max_generator_size(value: Option<&str>) -> Result<TestCaseSize, Usage> {
    let Some(value) = value else {
        return Ok(DEFAULT_MAX_GENERATOR_SIZE);
    };
    let parsed = value.parse::<u64>().map_err(|_| {
        Usage::new(format!(
            "--max-generator-size: expected an integer in 1..=4294967296, got `{value}`"
        ))
    })?;
    TestCaseSize::new(parsed).ok_or_else(|| {
        Usage::new(format!(
            "--max-generator-size={parsed} is outside the supported range 1..=4294967296"
        ))
    })
}

fn parse_generator_timeout(value: Option<&str>) -> Result<Option<Duration>, Usage> {
    let Some(value) = value else {
        return Ok(Some(austen_lib::driver::DEFAULT_GENERATOR_TIMEOUT));
    };
    let ms = value.parse::<u64>().map_err(|_| {
        Usage::new(format!(
            "--generator-timeout: expected a whole number of milliseconds, got `{value}`"
        ))
    })?;
    Ok((ms != 0).then(|| Duration::from_millis(ms)))
}

fn parse_chain_discard_limit(value: Option<&str>) -> Result<Option<NonZeroU64>, Usage> {
    let Some(value) = value else {
        return Ok(None);
    };
    let parsed = value.parse::<u64>().map_err(|_| {
        Usage::new(format!(
            "--chain-discard-limit: expected a positive integer, got `{value}`"
        ))
    })?;
    NonZeroU64::new(parsed).map(Some).ok_or_else(|| {
        Usage::new(
            "--chain-discard-limit=0 is invalid: write a positive number of exhausted requests",
        )
    })
}

fn exclusive(subject: &str, why: &str, flags: &[(&str, bool)]) -> Result<(), Usage> {
    for (flag, given) in flags {
        if *given {
            return Err(Usage::new(format!(
                "{subject} and {flag} are mutually exclusive: {why}"
            )));
        }
    }
    Ok(())
}

fn parse_input_source(text: &str) -> Result<InputSource, Usage> {
    if text == "-" {
        return Ok(InputSource::Stdin);
    }
    let Some(payload) = text.strip_prefix("0x") else {
        return Err(Usage::new(
            "--input expects exact bytes as 0x<even-length hex> or '-' for raw stdin",
        ));
    };
    if payload.len() % 2 != 0 {
        return Err(Usage::new(
            "--input hex payload has an odd number of digits",
        ));
    }

    fn nibble(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    let mut bytes = Vec::with_capacity(payload.len() / 2);
    let (pairs, remainder) = payload.as_bytes().as_chunks::<2>();
    debug_assert!(remainder.is_empty(), "odd lengths were rejected above");
    for (pair_index, pair) in pairs.iter().enumerate() {
        let offset = pair_index * 2;
        let high = nibble(pair[0]).ok_or_else(|| {
            Usage::new(format!(
                "--input hex payload has invalid digit {:?} at offset {offset}",
                char::from(pair[0])
            ))
        })?;
        let low = nibble(pair[1]).ok_or_else(|| {
            Usage::new(format!(
                "--input hex payload has invalid digit {:?} at offset {}",
                char::from(pair[1]),
                offset + 1
            ))
        })?;
        bytes.push((high << 4) | low);
    }
    Ok(InputSource::Inline(AustenInput::from_bytes(bytes)))
}

fn parse_duration(v: &str) -> Result<u64, Usage> {
    if let Ok(secs) = v.parse::<u64>() {
        return Ok(secs);
    }
    let span = humantime::parse_duration(v).map_err(|_| {
        Usage::new(format!(
            "--duration: not a duration: {v} (write seconds, or a span like 90s, 2m)"
        ))
    })?;
    if span.as_secs() == 0 && !span.is_zero() {
        return Err(Usage::new(format!(
            "--duration={v} rounds down to zero seconds, and zero means 'until \
             interrupted' — write a whole number of seconds, or omit --duration \
             to say that on purpose",
        )));
    }
    Ok(span.as_secs())
}

fn parse_corpus_snapshot_interval(v: &str) -> Result<Duration, Usage> {
    let span = if let Ok(secs) = v.parse::<u64>() {
        Duration::from_secs(secs)
    } else {
        humantime::parse_duration(v).map_err(|_| {
            Usage::new(format!(
                "--corpus-snapshot-interval: not a duration: {v} \
                 (write seconds, or a span like 30s, 2m)"
            ))
        })?
    };
    if span < Duration::from_secs(1) {
        return Err(Usage::new(format!(
            "--corpus-snapshot-interval={v} must be at least one second"
        )));
    }
    if span.as_millis() > u128::from(u64::MAX) {
        return Err(Usage::new(format!(
            "--corpus-snapshot-interval={v} is too large to record exactly"
        )));
    }
    if span.subsec_nanos() % 1_000_000 != 0 {
        return Err(Usage::new(format!(
            "--corpus-snapshot-interval={v} must be a whole number of milliseconds \
             so the report records it exactly"
        )));
    }
    Ok(span)
}

fn require_c_target(flag: &str, target: &Option<Target>) -> Result<(), Usage> {
    match target {
        Some(Target::CSource(_)) => Ok(()),
        Some(t) => Err(Usage::new(format!(
            "{flag} requires a .c target: {} is already compiled and austen does not recompile .o or .a targets",
            t.path().display()
        ))),
        None => Err(Usage::new(format!(
            "{flag} cannot be used with --harness: --harness skips the target compile"
        ))),
    }
}

#[cfg(any(feature = "honggfuzz", feature = "libfuzzer"))]
fn select_delegated_mode(
    schedule: &mut Option<Schedule>,
    flag: &str,
    required: &str,
    purpose: &str,
    adapter: AnyFuzzer,
) -> Result<(), Usage> {
    let Some(selected) = schedule else {
        return Err(Usage::new(format!(
            "{flag} requires --engine={required}: {purpose}; without --engine this command selects {}",
            crate::registry::default_selection_label()
        )));
    };
    let stage = selected
        .delegated_stage()
        .filter(|(_, engine)| austen_lib::DelegatedEngine::name(engine) == required)
        .map(|(index, _)| index)
        .ok_or_else(|| {
            Usage::new(format!(
                "{flag} requires --engine={required}: {purpose}; this command selected {}",
                selected.selection_label()
            ))
        })?;
    selected.stage_mut(stage).engine =
        austen_lib::StageEngine::Single(AnyEngine::Delegated(AnyDelegatedEngine::Fuzzer(adapter)));
    Ok(())
}

impl TryFrom<RawTest> for TestArgs {
    type Error = Usage;

    fn try_from(raw: RawTest) -> Result<Self, Usage> {
        if raw.smt_pruning && !cfg!(feature = "smt") {
            return Err(Usage::new("--smt-pruning requires the `smt` Cargo feature"));
        }
        let shrink_name = raw.shrink.as_ref().and_then(|name| name.as_deref());
        for (flag, requested, feature, enabled) in [
            (
                "--honggfuzz-mode",
                raw.honggfuzz_mode.is_some(),
                "honggfuzz",
                cfg!(feature = "honggfuzz"),
            ),
            (
                "--libfuzzer-mode",
                raw.libfuzzer_mode.is_some(),
                "libfuzzer",
                cfg!(feature = "libfuzzer"),
            ),
            (
                "--minimize-suite",
                raw.minimize_suite,
                "aflpp",
                cfg!(feature = "aflpp"),
            ),
            (
                "--shrink=afl-tmin",
                shrink_name == Some("afl-tmin"),
                "aflpp",
                cfg!(feature = "aflpp"),
            ),
            (
                "--shrink=libfuzzer",
                shrink_name == Some("libfuzzer"),
                "libfuzzer",
                cfg!(feature = "libfuzzer"),
            ),
        ] {
            if requested && !enabled {
                return Err(Usage::new(format!(
                    "{flag} requires the `{feature}` Cargo feature"
                )));
            }
        }
        if matches!(raw.shrink, Some(None)) && !cfg!(any(feature = "aflpp", feature = "libfuzzer"))
        {
            return Err(Usage::new(
                "--shrink requires the `aflpp` or `libfuzzer` Cargo feature",
            ));
        }
        for (key, given) in [("--count", &raw.count), ("-n", &raw.count_short)] {
            if given.is_some() {
                return Err(Usage::new(format!(
                    "{key} was removed: every engine is budgeted in time now — \
                     use --duration=<secs>"
                )));
            }
        }

        let duration = raw.duration.as_deref().map(parse_duration).transpose()?;
        let corpus_snapshot_interval = raw
            .corpus_snapshot_interval
            .as_deref()
            .map(parse_corpus_snapshot_interval)
            .transpose()?;
        let max_generator_size = parse_max_generator_size(raw.max_generator_size.as_deref())?;
        let generator_timeout = parse_generator_timeout(raw.generator_timeout.as_deref())?;
        let chain_discard_limit = parse_chain_discard_limit(raw.chain_discard_limit.as_deref())?;
        let pass_options = PassOptions {
            inline: parse_inline(raw.inline.as_deref())?,
            max_unfolds: parse_max_unfolds(raw.max_unfolds.as_deref())?,
            path_pruning: raw.path_pruning,
            smt_pruning: raw.smt_pruning,
            experimental_product_arg_destruction: raw.experimental_product_arg_destruction,
            experimental_return_pruning: raw.experimental_return_pruning,
            experimental_arg_pruning: raw.experimental_arg_pruning,
            sink_args: raw.sink_args,
            specialize_guarded_choices: raw.specialize_guarded_choices,
            max_array_length: parse_max_array_length(raw.max_array_length.as_deref())?,
        };
        let production_only = pass_options.production_only_requests();
        let target = raw.target.map(Target::classify).transpose()?;

        if raw.harness_manifest.is_some() && raw.harness.is_none() {
            return Err(Usage::new(
                "--harness-manifest requires --harness=<path>: build metadata describes an already-built harness",
            ));
        }

        match (&raw.harness, &target) {
            (Some(_), Some(t)) => {
                return Err(Usage::new(format!(
                    "--harness and a target ({}) are mutually exclusive: --harness skips the build",
                    t.path().display()
                )));
            }
            (None, None) => {
                return Err(Usage::new(
                    "a target (.o, .a, or .c) is required, unless --harness names a built harness",
                ));
            }
            _ => {}
        }

        if raw.cflag.iter().any(String::is_empty) {
            return Err(Usage::new(
                "--cflag needs one non-empty compiler argument after `=`",
            ));
        }
        if raw.cflag.iter().any(|flag| flag == "--") {
            return Err(Usage::new(
                "--cflag=-- cannot terminate compiler options: austen must append its mandatory flags, source, and output path",
            ));
        }
        if let Some(flag) = raw.cflag.iter().find(|flag| {
            flag.starts_with('@')
                || ["--config", "--config-user-dir", "--config-system-dir"].contains(&flag.as_str())
                || ["--config=", "--config-user-dir=", "--config-system-dir="]
                    .iter()
                    .any(|prefix| flag.starts_with(prefix))
        }) {
            return Err(Usage::new(format!(
                "--cflag={flag} loads hidden compiler arguments, which austen cannot validate or layer before its mandatory flags; pass those arguments as separate --cflag=<arg> options"
            )));
        }
        if let (_, Some(flag)) = crate::interpreted_cflag_words(&raw.cflag) {
            return Err(Usage::new(format!(
                "--cflag={flag} needs a following compiler operand; pass that operand as another --cflag=<arg> so it cannot consume austen's mandatory flags"
            )));
        }
        if !raw.cflag.is_empty() {
            require_c_target("--cflag", &target)?;
        }

        if !raw.coverage.is_empty() {
            require_c_target("--coverage", &target)?;
        }

        if raw.check_heap_ownership {
            exclusive(
                "--check-heap-ownership",
                "allocator certification and rewriting happen while preparing the target, which --harness skips",
                &[("--harness", raw.harness.is_some())],
            )?;
        }

        let input = raw.input.as_deref().map(parse_input_source).transpose()?;

        if input.is_some() {
            let mut flags = vec![
                ("--duration", duration.is_some()),
                ("--seed", raw.seed.is_some()),
                ("--record", raw.record.is_some()),
                ("--replay", raw.replay.is_some()),
                ("--coverage", !raw.coverage.is_empty()),
                ("--engine", raw.engine.is_some()),
                ("--function", raw.function.is_some()),
                ("--ignore-custom-generators", raw.ignore_custom_generators),
                ("--function-selector", raw.function_selector.is_some()),
                ("--sizing-strategy", raw.sizing_strategy.is_some()),
                ("--max-generator-size", raw.max_generator_size.is_some()),
                ("--generator-timeout", raw.generator_timeout.is_some()),
                ("--chain-discard-limit", raw.chain_discard_limit.is_some()),
                ("--exit-fast", raw.exit_fast),
                ("--strict-duration", raw.strict_duration),
            ];
            flags.extend(production_only);
            exclusive(
                "--input",
                "an exact supplied input runs exactly once",
                &flags,
            )?;
        }

        if raw.build_only {
            exclusive(
                "--build-only",
                "--build-only builds and stages everything and tests nothing",
                &[
                    ("--duration", duration.is_some()),
                    ("--seed", raw.seed.is_some()),
                    ("--input", input.is_some()),
                    ("--record", raw.record.is_some()),
                    ("--replay", raw.replay.is_some()),
                    ("--tyche", raw.tyche.is_some()),
                    ("--exit-fast", raw.exit_fast),
                    ("--strict-duration", raw.strict_duration),
                ],
            )?;
        }

        if raw.replay.is_some() && raw.seed.is_some() {
            return Err(Usage::new(
                "--replay and --seed are mutually exclusive: \
                 the replayed entropy tape is the entropy, not the seed",
            ));
        }

        if raw.replay.is_some() && duration.is_some() {
            return Err(Usage::new(
                "--replay and --duration are mutually exclusive: \
                 the recording is the budget, so a replayed run lasts exactly as \
                 long as the entropy it was given",
            ));
        }

        if raw.replay.is_some() && raw.exit_fast {
            return Err(Usage::new(
                "--replay and --exit-fast are mutually exclusive: \
                 the recording is the exact input budget, so a replay runs every input it holds",
            ));
        }

        if raw.replay.is_some() && raw.function_selector.is_some() {
            return Err(Usage::new(
                "--replay and --function-selector are mutually exclusive: \
                 each recorded input already names the function to replay",
            ));
        }

        if raw.replay.is_some() && raw.max_generator_size.is_some() {
            return Err(Usage::new(
                "--replay and --max-generator-size are mutually exclusive: \
                 each recorded input already carries the test-case size to replay",
            ));
        }

        if raw.replay.is_some() && raw.generator_timeout.is_some() {
            return Err(Usage::new(
                "--replay and --generator-timeout are mutually exclusive: \
                 a wall-clock deadline would make a replayed attempt consume \
                 different entropy from the one that was recorded",
            ));
        }

        if raw.replay.is_some() && raw.chain_discard_limit.is_some() {
            return Err(Usage::new(
                "--replay and --chain-discard-limit are mutually exclusive: \
                 each recorded input already names the chain member to replay",
            ));
        }

        if raw.replay.is_some() && raw.sizing_strategy.is_some() {
            return Err(Usage::new(
                "--replay and --sizing-strategy are mutually exclusive: \
                 each recorded input already carries the test-case size to replay",
            ));
        }

        #[cfg_attr(
            not(any(feature = "honggfuzz", feature = "libfuzzer")),
            allow(unused_mut)
        )]
        let mut engine = raw
            .engine
            .as_deref()
            .map(|text| {
                Schedule::parse(text, crate::registry::engines()).map_err(|error| {
                    let name = match &error {
                        ScheduleError::UnknownEngine { name, .. } => name.as_str(),
                        ScheduleError::LibFuzzerFork => "libfuzzer-fork",
                        _ => return Usage::new(error.to_string()),
                    };
                    match crate::registry::missing_engine_feature(name) {
                        Some(feature) => Usage::new(format!(
                            "engine `{name}` requires the `{feature}` Cargo feature"
                        )),
                        None => Usage::new(error.to_string()),
                    }
                })
            })
            .transpose()?;

        #[cfg(feature = "honggfuzz")]
        if let Some(mode) = raw.honggfuzz_mode {
            select_delegated_mode(
                &mut engine,
                "--honggfuzz-mode",
                "honggfuzz",
                "it configures honggfuzz's input transport",
                mode.adapter(),
            )?;
        }
        #[cfg(feature = "libfuzzer")]
        if let Some(mode) = raw.libfuzzer_mode {
            select_delegated_mode(
                &mut engine,
                "--libfuzzer-mode",
                "libfuzzer",
                "it configures libFuzzer's loop",
                mode.adapter(),
            )?;
        }

        let selected =
            if input.is_some() {
                None
            } else {
                Some(engine.clone().or_else(crate::registry::default_schedule).ok_or_else(|| {
                Usage::new(format!(
                    "this build has no default tree engine: install Bennet and Darcy with \
                     `{}`, or select an enabled engine with --engine",
                    crate::registry::RECOMMENDED_INSTALL
                ))
            })?)
            };
        let selected_stages = selected.as_ref().map_or(&[][..], Schedule::stages);
        let selected_label = selected.as_ref().map_or_else(
            || "exact input".to_owned(),
            |schedule| schedule.selection_label(),
        );
        let single = selected.as_ref().is_none_or(Schedule::is_single);
        let single_chain = selected.as_ref().and_then(Schedule::single_chain).is_some();
        let chain_discard_limit = chain_discard_limit.or_else(|| {
            (raw.engine.is_none() && raw.replay.is_none() && single_chain)
                .then_some(crate::registry::DEFAULT_CHAIN_DISCARD_LIMIT)
        });
        if chain_discard_limit.is_some() && !single_chain {
            return Err(Usage::new(format!(
                "--chain-discard-limit requires a single-stage tree-engine chain such as \
                 --engine=bennet/darcy; this command selected {}",
                selected_label
            )));
        }
        let descriptor = selected
            .as_ref()
            .map_or(crate::registry::NEUTRAL_DESCRIPTOR, Schedule::descriptor);
        let mut unsupported = vec![
            (
                "--exit-fast",
                raw.exit_fast && descriptor.loop_owner == austen_lib::LoopOwner::DelegatedTool,
            ),
            (
                "--strict-duration",
                raw.strict_duration
                    && descriptor.loop_owner == austen_lib::LoopOwner::DelegatedTool,
            ),
            (
                "--tyche",
                raw.tyche.is_some()
                    && descriptor.loop_owner == austen_lib::LoopOwner::DelegatedTool,
            ),
            ("--seed", raw.seed.is_some() && !descriptor.generic_seed),
            ("--record", raw.record.is_some() && !descriptor.entropy_tape),
            ("--replay", raw.replay.is_some() && !descriptor.entropy_tape),
            (
                "--function-selector",
                raw.function_selector.is_some()
                    && descriptor.function_selection != FunctionSelectionOwner::Austen,
            ),
            (
                "--max-generator-size",
                raw.max_generator_size.is_some() && !descriptor.producer_options,
            ),
            (
                "--sizing-strategy",
                raw.sizing_strategy.is_some() && !descriptor.producer_options,
            ),
            (
                "--max-array-length",
                raw.max_array_length.is_some() && !descriptor.max_array_length,
            ),
            (
                "--generator-timeout",
                raw.generator_timeout.is_some() && !descriptor.producer_options,
            ),
            (
                "--ignore-custom-generators",
                raw.ignore_custom_generators && !descriptor.producer_options,
            ),
        ];
        if selected.is_some()
            && descriptor.function_selection == FunctionSelectionOwner::Generator
            && !descriptor.producer_options
        {
            unsupported.extend([
                ("--inline", raw.inline.is_some()),
                ("--max-unfolds", raw.max_unfolds.is_some()),
            ]);
        }
        unsupported.extend(
            production_only
                .map(|(flag, requested)| (flag, requested && !descriptor.producer_options)),
        );
        let why = if !single {
            "a multi-stage schedule carries every stage's restrictions"
        } else {
            match descriptor.loop_owner {
                austen_lib::LoopOwner::DelegatedTool => {
                    "a delegated fuzzer owns the loop and its entropy"
                }
                austen_lib::LoopOwner::Austen => {
                    "this raw-input generator has no tree entropy or producer pipeline"
                }
            }
        };
        exclusive(&selected_label, why, &unsupported)?;

        if !single {
            exclusive(
                "a multi-stage --engine",
                "one campaign per stage runs in order, and each of these speaks of exactly \
                 one campaign — stage budgets go in the schedule as `:seconds`",
                &[
                    ("--record", raw.record.is_some()),
                    ("--replay", raw.replay.is_some()),
                    ("--tyche", raw.tyche.is_some()),
                    ("--exit-fast", raw.exit_fast),
                    ("--strict-duration", raw.strict_duration),
                    ("--harness", raw.harness.is_some()),
                    ("--duration", duration.is_some()),
                ],
            )?;
        }
        let stated_single = match selected_stages {
            [stage] => match stage.budget {
                austen_lib::StageBudget::Seconds(seconds) => Some(seconds.get()),
                austen_lib::StageBudget::Unstated => None,
            },
            _ => None,
        };
        if let (Some(stated), Some(_)) = (stated_single, duration) {
            return Err(Usage::new(format!(
                "--duration and --engine={} both bound the campaign: keep one \
                 (the schedule already says {stated} seconds)",
                selected.as_ref().expect("a stated budget has a schedule")
            )));
        }

        if raw.strict_duration {
            exclusive(
                "--strict-duration",
                "it bounds one finite Austen-owned live campaign before dispatch",
                &[
                    ("--input", input.is_some()),
                    ("--replay", raw.replay.is_some()),
                    ("--build-only", raw.build_only),
                ],
            )?;
            if stated_single
                .or_else(|| duration.filter(|&seconds| seconds != 0))
                .is_none()
            {
                return Err(Usage::new(
                    "--strict-duration requires an explicit nonzero duration, from \
                     --duration=<seconds> or a single --engine=<name>:<seconds> stage",
                ));
            }
        }

        if raw.corpus_snapshot_interval.is_some() {
            exclusive(
                "--corpus-snapshot-interval",
                "it configures a running time-based campaign",
                &[
                    ("--input", input.is_some()),
                    ("--replay", raw.replay.is_some()),
                    ("--build-only", raw.build_only),
                ],
            )?;
        }

        let shrink = raw
            .shrink
            .as_ref()
            .map(|named| match named.as_deref() {
                None => Ok(ShrinkChoice::Default),
                Some(name) => crate::registry::resolve_shrinker(name)
                    .map(ShrinkChoice::Named)
                    .ok_or_else(|| {
                        Usage::new(format!(
                            "unknown shrinker `{name}`: expected one of {}",
                            crate::registry::shrinker_names()
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))
                    }),
            })
            .transpose()?;

        if shrink.is_some() {
            exclusive(
                "--shrink",
                "it reduces what a campaign found, so a campaign has to run",
                &[
                    ("--input", input.is_some()),
                    ("--build-only", raw.build_only),
                ],
            )?;
        }

        if raw.minimize_suite {
            exclusive(
                "--minimize-suite",
                "it needs a campaign and a target recipe for the common evaluation harness",
                &[
                    ("--input", input.is_some()),
                    ("--build-only", raw.build_only),
                    ("--harness", raw.harness.is_some()),
                    ("--no-coverage", raw.no_coverage),
                ],
            )?;
        }

        let mode = if let Some(source) = input {
            TestMode::SingleInput { source }
        } else if let Some(tape) = raw.replay {
            TestMode::Replay {
                tape,
                record: raw.record,
            }
        } else if raw.build_only {
            TestMode::Build
        } else {
            TestMode::Campaign {
                duration_secs: match (stated_single, duration.filter(|&secs| secs != 0)) {
                    (Some(stated), _) => Some(stated),
                    (None, Some(secs)) if single => Some(secs),
                    (None, None) if single => None,
                    _ => selected_stages
                        .iter()
                        .map(|stage| match stage.budget {
                            austen_lib::StageBudget::Seconds(seconds) => Some(seconds.get()),
                            austen_lib::StageBudget::Unstated => None,
                        })
                        .sum::<Option<u64>>(),
                },
                seed: raw.seed.unwrap_or_else(clock_seed),
                record: raw.record,
            }
        };

        Ok(TestArgs {
            spec: raw.spec,
            target,
            harness: raw.harness,
            harness_manifest: raw.harness_manifest,
            cflags: raw.cflag,
            coverage: raw.coverage,
            no_coverage: raw.no_coverage,
            engine,
            selected,
            size_selector: raw
                .sizing_strategy
                .map(SizingStrategyMode::kind)
                .unwrap_or_default(),
            function_selector: raw
                .function_selector
                .map(FunctionSelectorMode::kind)
                .unwrap_or_default(),
            max_generator_size,
            generator_timeout,
            chain_discard_limit,
            corpus_snapshot_interval,
            mode,
            function: raw.function,
            ignore_custom_generators: raw.ignore_custom_generators,
            crashes: raw.crashes,
            shrink,
            minimize_suite: raw.minimize_suite,
            work: raw.work,
            dsl_log_dir: raw.dsl_log_dir,
            pass_options,
            check_heap_ownership: raw.check_heap_ownership,
            allow_unstable_addresses: raw.allow_unstable_addresses,
            report: raw.report,
            stats: raw.stats,
            tyche: raw.tyche,
            exit_fast: raw.exit_fast,
            strict_duration: raw.strict_duration,
            fail_on_crash: raw.fail_on_crash,
            clean: raw.clean,
            output: crate::render::Output {
                progress: when("--progress", raw.progress.as_deref())?,
                color: when("--color", raw.color.as_deref())?,
                verbosity: crate::render::Verbosity {
                    quiet: raw.quiet,
                    level: if raw.trace_inputs {
                        raw.verbose.max(TRACE_INPUTS_LEVEL)
                    } else {
                        raw.verbose
                    },
                },
                events: raw.events,
            },
        })
    }
}
