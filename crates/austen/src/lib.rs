pub mod args;
pub mod harness;
mod prepared;
mod provenance;
pub mod registry;
pub mod render;
pub(crate) mod report;
pub mod runtime;
mod sanity;
pub mod shim;
pub mod stamp;
mod statistics;

use austen_lib::observe;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use austen_lib::observe::RunEvent;
use austen_lib::shrink::{AnyShrinker, HarnessKind, Shrinker, SuiteTool};

pub use args::{Cmd, InputSource, SanityArgs, SanityFormat, TestArgs, TestMode, Usage};
pub use austen_lib::observe::report::{Blame, ErrorKind, RunReport, Stage};
use austen_lib::observe::report::{Budget, RunProvenance, RuntimeProvenance};

pub const EXIT_FAILURE: i32 = 2;
pub const EXIT_CRASHES: i32 = 1;
pub const EXIT_SANITY_WARNINGS: i32 = 1;
pub const EXIT_INTERRUPTED: i32 = 130;

pub(crate) fn engine_label(token: &str) -> String {
    crate::registry::engines()
        .resolve(token)
        .map(|engine| engine.selection_label())
        .unwrap_or_else(|| token.to_string())
}

fn install_interrupt_handler() -> observe::Cancel {
    use std::sync::OnceLock;
    static INSTALLED: OnceLock<observe::Cancel> = OnceLock::new();
    INSTALLED
        .get_or_init(|| {
            let cancel = observe::Cancel::new();
            let handler = cancel.clone();
            let presses = std::sync::atomic::AtomicU8::new(0);
            let _ = ctrlc::set_handler(move || {
                if presses.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 1 {
                    std::process::exit(EXIT_INTERRUPTED);
                }
                handler.request();
            });
            cancel
        })
        .clone()
}

fn install_tracing(output: &render::Output) {
    use std::sync::OnceLock;
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        use tracing_subscriber::{EnvFilter, fmt, prelude::*};
        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(output.verbosity.filter()));
        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().with_writer(std::io::stderr).without_time())
            .try_init();
    });
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Usage(#[from] Usage),
    #[error("{0}")]
    Runtime(#[from] runtime::RuntimeError),
    #[error("{0}")]
    Harness(#[from] harness::BuildError),
    #[error("{0}")]
    Driver(#[from] austen_lib::driver::DriverError),
    #[error("{0}")]
    External(#[from] austen_lib::external::ExternalError),
    #[error("{0}")]
    ShrinkNotFound(austen_lib::external::NotFound),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Failure {
    pub(crate) stage: Stage,
    pub(crate) kind: ErrorKind,
    pub(crate) blame: Blame,
}

fn classify(e: &Error) -> Failure {
    use Blame as Who;
    use ErrorKind as K;
    use austen_lib::CampaignError as C;
    use austen_lib::driver::DriverError as D;
    use austen_lib::external::ExternalError as EE;
    use harness::BuildError as B;
    fn at(stage: Stage, kind: ErrorKind, blame: Blame) -> Failure {
        Failure { stage, kind, blame }
    }
    fn build_step(step: &harness::Step, blame: Blame) -> Failure {
        match step {
            harness::Step::CompileTarget => at(Stage::CompileTarget, K::BuildCc, blame),
            harness::Step::PrepareTarget => at(Stage::CompileTarget, K::BuildObjcopy, blame),
            harness::Step::LinkHarness => at(Stage::CompileHarness, K::BuildRustc, blame),
        }
    }
    fn external(e: &EE) -> Failure {
        match e {
            EE::NotFound(_) => at(Stage::Spawn, K::ExternalNotFound, Who::Environment),
            EE::Spawn { .. } => at(Stage::Spawn, K::ExternalSpawn, Who::Environment),
            EE::Failed { .. } => at(Stage::Spawn, K::ExternalFailed, Who::Environment),
            EE::Unrunnable { .. } => at(Stage::Spawn, K::ExternalUnrunnable, Who::Spec),
            EE::Io { .. } => at(Stage::Dispatch, K::ExternalIo, Who::Environment),
            EE::CandidateGate(_) => at(Stage::Generate, K::DriverFindings, Who::Austen),
        }
    }
    fn campaign(e: &C) -> Failure {
        match e {
            C::WithReport { source, .. } => campaign(source),
            C::EngineOpen(austen_lib::EngineOpenError::Custom { .. }) => {
                at(Stage::Generate, K::DriverUnrunnable, Who::Spec)
            }
            C::EngineOpen(_) => at(Stage::Generate, K::DriverUnrunnable, Who::Austen),
            C::Io { .. } => at(Stage::Convert, K::DriverIo, Who::Environment),
            C::HeapTimeout { .. } => at(Stage::Spawn, K::DriverHeapTimeout, Who::Austen),
            C::HeapMalformed { .. } => at(Stage::Spawn, K::DriverHeapMalformed, Who::Austen),
            C::SpoolPathConflict { .. } => at(Stage::Spawn, K::DriverIo, Who::Environment),
            C::SpoolPathResolution { .. } => at(Stage::Spawn, K::DriverIo, Who::Environment),
            C::Unrunnable(_) => at(Stage::Generate, K::DriverUnrunnable, Who::Spec),
            C::OracleInternal(_) => at(Stage::Dispatch, K::DriverUnrunnable, Who::Austen),
            C::Generator { phase, .. } => at(
                match *phase {
                    austen_lib::GeneratorPhase::Prepare => Stage::Spawn,
                    austen_lib::GeneratorPhase::Open => Stage::Generate,
                    austen_lib::GeneratorPhase::Live => Stage::Dispatch,
                },
                K::DriverUnrunnable,
                Who::Environment,
            ),
            C::GeneratorTrustMismatch { .. } => {
                at(Stage::Generate, K::DriverUnrunnable, Who::Austen)
            }
            C::LibAfl { during, .. } => {
                let stage = if during.is_run() {
                    Stage::Dispatch
                } else {
                    Stage::Spawn
                };
                let blame = if during.is_resource() {
                    Who::Environment
                } else {
                    Who::Austen
                };
                at(stage, K::DriverLibAfl, blame)
            }
            C::External(e) => external(e),
            C::Findings(_) => at(Stage::Dispatch, K::DriverFindings, Who::Environment),
        }
    }
    match e {
        Error::Usage(_) => at(Stage::Args, K::Usage, Who::Environment),
        Error::Runtime(_) => at(Stage::Runtime, K::Runtime, Who::Environment),
        Error::Io { .. } => at(Stage::Runtime, K::Io, Who::Environment),
        Error::Harness(b) => match b {
            B::Read { .. } | B::Write { .. } | B::SpecSnapshot { .. } => {
                at(Stage::Runtime, K::BuildIo, Who::Environment)
            }
            B::Template { .. } => at(Stage::Runtime, K::BuildTemplate, Who::Environment),
            B::ParseSpec { .. } => at(Stage::SpecParse, K::BuildParseSpec, Who::Spec),
            B::Shim { .. } => at(Stage::Shim, K::BuildShim, Who::Spec),
            B::Spawn { step, .. } => build_step(step, Who::Environment),
            B::Failed {
                step: step @ harness::Step::PrepareTarget,
                ..
            } => build_step(step, Who::Environment),
            B::Failed { step, .. } => build_step(step, Who::Spec),
            B::PreparedTarget { .. } | B::PreparedGlobals { .. } => {
                build_step(&harness::Step::PrepareTarget, Who::Environment)
            }
            B::ForeignInstrumentation { .. } => at(
                Stage::CompileTarget,
                K::BuildForeignInstrumentation,
                Who::Environment,
            ),
        },
        Error::Driver(d) => match d {
            D::GeneratorSource(source) => {
                use austen_lib::driver::GeneratorSourceError as G;
                match source {
                    G::Read { .. } | G::Write { .. } => {
                        at(Stage::Convert, K::DriverIo, Who::Environment)
                    }
                    G::Spec { .. } => at(Stage::SpecParse, K::DriverParseSpec, Who::Spec),
                    G::Context { .. } => at(Stage::Convert, K::DriverUnrunnable, Who::Austen),
                    G::UnknownFunction { .. } => {
                        at(Stage::Convert, K::DriverNoSuchFunction, Who::Environment)
                    }
                    G::Functions { source, .. }
                        if matches!(
                            source.as_ref(),
                            austen_lib::spec::ConvertError::NoFunctions
                        ) =>
                    {
                        at(Stage::Convert, K::DriverNoFunctions, Who::Spec)
                    }
                    G::Json { .. }
                    | G::Functions { .. }
                    | G::MissingEntry { .. }
                    | G::AmbiguousEntry { .. } => at(Stage::Convert, K::DriverConvert, Who::Spec),
                }
            }
            D::Read { .. } | D::Write { .. } => at(Stage::Convert, K::DriverIo, Who::Environment),
            D::NotARecording { .. } => at(Stage::Convert, K::DriverNotARecording, Who::Environment),
            D::ParseSpec { .. } => at(Stage::SpecParse, K::DriverParseSpec, Who::Spec),
            D::NoFunctions { .. } => at(Stage::Convert, K::DriverNoFunctions, Who::Spec),
            D::NoSuchFunction { .. } => {
                at(Stage::Convert, K::DriverNoSuchFunction, Who::Environment)
            }
            D::DuplicateFunction { .. } | D::TooManyFunctions { .. } => {
                at(Stage::Convert, K::DriverConvert, Who::Spec)
            }
            D::Convert { .. } => at(Stage::Convert, K::DriverConvert, Who::Spec),
            D::NotWellScoped { .. } => at(Stage::Convert, K::DriverNotWellScoped, Who::Austen),
            D::MalformedFunctionTable { .. } => {
                at(Stage::Convert, K::DriverMalformedFunctionTable, Who::Austen)
            }
            D::WireDiverged { .. } => at(Stage::Convert, K::DriverWireDiverged, Who::Austen),
            D::BadInput(_) => at(Stage::Generate, K::DriverBadInput, Who::Austen),
            D::HostileEnv { .. } => at(Stage::Convert, K::DriverHostileEnv, Who::Environment),
            D::HarnessOracleCapability { .. } => {
                at(Stage::Spawn, K::DriverUnrunnable, Who::Environment)
            }
            D::Campaign(c) => campaign(c),
            D::StageFailed { source, .. } => campaign(source),
        },
        Error::External(e) => external(e),
        Error::ShrinkNotFound(_) => at(Stage::Spawn, K::ShrinkNotFound, Who::Environment),
    }
}

#[derive(Default)]
pub(crate) struct Trace {
    pub(crate) provenance: Option<RunProvenance>,
    runtime: Option<RuntimeProvenance>,
    resolved_shrinker: Option<String>,
    crashes: Option<PathBuf>,
    corpus: Option<PathBuf>,
    pub(crate) work: Option<PathBuf>,
    pub(crate) harness: Option<PathBuf>,
    pub(crate) effective_check_heap_ownership: Option<bool>,
    function: Option<String>,
    pub(crate) entropy: Option<austen_lib::driver::EntropySource>,
}

pub fn main() -> i32 {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match args::parse(&argv) {
        Ok(Cmd::Help) => {
            print!("{}", args::usage());
            0
        }
        Ok(Cmd::Version) => {
            println!("austen {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Ok(Cmd::Sanity(args)) => match sanity::run(&args) {
            Ok(report) => {
                let has_warnings = report.has_warnings();
                if let Err(error) = sanity::render(args.format, &report) {
                    eprintln!("austen: {error}");
                    EXIT_FAILURE
                } else if args.fail_on_warning && has_warnings {
                    EXIT_SANITY_WARNINGS
                } else {
                    0
                }
            }
            Err(error) => {
                eprintln!("austen: {error}");
                EXIT_FAILURE
            }
        },
        Ok(Cmd::Test(t)) => {
            install_tracing(&t.output);
            let cancel = install_interrupt_handler();
            match test(&t) {
                Ok(outcome) => {
                    if cancel.is_cancelled() {
                        EXIT_INTERRUPTED
                    } else if t.fail_on_crash && outcome.campaign.findings.confirmed() > 0 {
                        EXIT_CRASHES
                    } else {
                        0
                    }
                }
                Err(e) => {
                    eprintln!("austen: {e}");
                    if matches!(e, Error::Usage(_)) {
                        eprint!("\n{}", args::usage());
                    }
                    EXIT_FAILURE
                }
            }
        }
        Err(e) => {
            eprintln!("austen: {e}\n");
            eprint!("{}", args::usage());
            EXIT_FAILURE
        }
    }
}

fn cache_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("AUSTEN_CACHE_DIR") {
        return PathBuf::from(d);
    }
    if let Some(d) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(d).join("austen");
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".cache").join("austen"))
        .unwrap_or_else(|| std::env::temp_dir().join("austen"))
}

#[allow(clippy::too_many_arguments)]
fn run_key_with_options(
    spec: &Path,
    target: Option<&Path>,
    function: Option<&str>,
    pass_options: austen_lib::driver::PassOptions,
    max_generator_size: Option<austen_lib::driver::TestCaseSize>,
    size_selector: Option<austen_lib::SizeSelectorKind>,
    generator_timeout: args::GeneratorTimeout,
    coverage: &[args::CoverageMode],
    cflags: &[String],
    compile_cwd: Option<&Path>,
    check_heap_ownership: bool,
    custom_generator_identity: Option<&[u8]>,
    no_coverage: bool,
) -> String {
    fn part(hash: &mut stamp::Fnv1a, bytes: &[u8]) {
        hash.eat(&(bytes.len() as u64).to_le_bytes());
        hash.eat(bytes);
    }

    fn opt_part(hash: &mut stamp::Fnv1a, bytes: Option<&[u8]>) {
        match bytes {
            Some(bytes) => {
                hash.eat(&[1]);
                part(hash, bytes);
            }
            None => hash.eat(&[0]),
        }
    }

    let mut h = stamp::Fnv1a::new();
    part(&mut h, b"selector-prefixed-input-v1");
    part(&mut h, spec.as_os_str().as_encoded_bytes());
    opt_part(&mut h, std::fs::read(spec).ok().as_deref());
    if let Some(identity) = custom_generator_identity {
        part(&mut h, b"custom-atg-producers-v1");
        part(&mut h, identity);
    }
    opt_part(&mut h, target.map(|t| t.as_os_str().as_encoded_bytes()));
    opt_part(&mut h, function.map(str::as_bytes));
    for arg in harness::pass_option_args(pass_options) {
        part(&mut h, arg.as_bytes());
    }
    if let Some(arg) = harness::max_generator_size_arg(max_generator_size) {
        part(&mut h, arg.as_bytes());
    }
    if let Some(arg) = harness::sizing_strategy_arg(size_selector) {
        part(&mut h, arg.as_bytes());
    }
    if let Some(arg) = harness::generator_timeout_arg(generator_timeout) {
        part(&mut h, arg.as_bytes());
    }
    if !coverage.is_empty() {
        part(&mut h, b"campaign-coverage-v1");
        for mode in coverage {
            part(&mut h, mode.token().as_bytes());
        }
    }
    if no_coverage {
        part(&mut h, b"--no-coverage");
    }
    if !cflags.is_empty() {
        part(&mut h, b"user-cflags-v1");
        part(
            &mut h,
            compile_cwd
                .expect("compiler flags always capture their working directory")
                .as_os_str()
                .as_encoded_bytes(),
        );
        for flag in cflags {
            part(&mut h, flag.as_bytes());
        }
    }
    part(
        &mut h,
        if check_heap_ownership {
            b"check-heap-ownership-v1"
        } else {
            b"no-malloc-redirect-v1"
        },
    );
    h.eat(runtime::host().as_bytes());
    h.hex()
}

fn absolute(path: &Path) -> Result<PathBuf, Error> {
    path.canonicalize().map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn prepare_work_dir(
    requested: &Path,
    clean: bool,
    protected: &[austen_lib::ProtectedPath],
) -> Result<PathBuf, Error> {
    if clean {
        let work = austen_lib::tools::path_safety::PathTraversal::inspect(requested).map_err(
            |source| Error::Io {
                path: requested.to_path_buf(),
                source,
            },
        )?;
        if work.is_filesystem_root() {
            return Err(Usage::from_message(format!(
                "--clean refuses to recursively remove filesystem root {}",
                requested.display()
            ))
            .into());
        }
        let mut broad_roots = vec![(
            "current working directory",
            std::env::current_dir().map_err(|source| Error::Io {
                path: PathBuf::from("."),
                source,
            })?,
        )];
        if let Some(home) = std::env::var_os("HOME") {
            broad_roots.push(("home directory", PathBuf::from(home)));
        }
        for (role, path) in broad_roots {
            let anchor = austen_lib::tools::path_safety::PathTraversal::inspect(&path).map_err(
                |source| Error::Io {
                    path: path.clone(),
                    source,
                },
            )?;
            if anchor.crosses(&work) {
                return Err(Usage::from_message(format!(
                    "--clean refuses to recursively remove {} or its ancestor {}",
                    role,
                    requested.display()
                ))
                .into());
            }
        }
        for path in protected {
            let configured = austen_lib::tools::path_safety::PathTraversal::inspect(&path.path)
                .map_err(|source| Error::Io {
                    path: path.path.clone(),
                    source,
                })?;
            if configured.overlaps(&work) {
                return Err(Usage::from_message(format!(
                    "--clean work directory {} overlaps the configured {} {}",
                    requested.display(),
                    path.role,
                    path.path.display()
                ))
                .into());
            }
        }
        match std::fs::remove_dir_all(requested) {
            Ok(()) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(Error::Io {
                    path: requested.to_path_buf(),
                    source,
                });
            }
        }
    }
    std::fs::create_dir_all(requested).map_err(|source| Error::Io {
        path: requested.to_path_buf(),
        source,
    })?;
    absolute(requested)
}

pub fn test(t: &TestArgs) -> Result<austen_lib::driver::Report, Error> {
    let reporter = t.output.reporter().map_err(|source| Error::Io {
        path: t.output.events.clone().unwrap_or_default(),
        source,
    })?;
    let cancel = install_interrupt_handler();

    let capture = t
        .report
        .as_ref()
        .map(|_| provenance::Capture::new(t, reporter.clone()));
    let reporter = capture
        .as_ref()
        .map_or(reporter, |capture| observe::Reporter::new(capture.clone()));

    let mut trace = Trace::default();
    let resolved_input = resolve_supplied_input(t);
    let canonical_input = resolved_input
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .map(canonical_input_text);
    let out = resolved_input
        .and_then(|input| run_test(t, input.as_ref(), &mut trace, &reporter, &cancel));
    trace.provenance = capture.map(|capture| {
        let mut provenance = capture.snapshot();
        let configuration = &mut provenance.configuration;
        provenance.runtime = trace.runtime.take();
        configuration.resolved_shrinker = trace.resolved_shrinker.take();
        configuration.work = trace.work.clone().or(configuration.work.take());
        configuration.crashes = trace.crashes.take().or(configuration.crashes.take());
        configuration.corpus = trace.corpus.take();
        provenance
    });
    let doc = report::build(
        &invocation(
            t,
            canonical_input.as_deref(),
            cancel.is_cancelled(),
            trace.effective_check_heap_ownership,
        ),
        &trace,
        out.as_ref(),
    );
    if let Ok(report) = &out {
        summarize(report, &doc, &t.stats, t.dsl_log_dir.as_deref());
    }
    if let Some(path) = &t.report {
        match &out {
            Ok(_) => report::write(path, &doc)?,
            Err(_) => {
                if let Err(w) = report::write(path, &doc) {
                    reporter.emit(|| {
                        RunEvent::Warning(observe::Warning::ReportWriteFailed {
                            path: path.clone(),
                            message: w.to_string(),
                        })
                    });
                }
            }
        }
    }
    out
}

fn resolve_supplied_input(t: &TestArgs) -> Result<Option<austen_lib::wire::AustenInput>, Error> {
    match &t.mode {
        TestMode::SingleInput {
            source: InputSource::Inline(input),
        } => Ok(Some(input.clone())),
        TestMode::SingleInput {
            source: InputSource::Stdin,
        } => {
            let mut bytes = Vec::new();
            std::io::stdin()
                .read_to_end(&mut bytes)
                .map_err(|source| Error::Io {
                    path: PathBuf::from("<stdin>"),
                    source,
                })?;
            Ok(Some(austen_lib::wire::AustenInput::from_bytes(bytes)))
        }
        _ => Ok(None),
    }
}

fn canonical_input_text(input: &austen_lib::wire::AustenInput) -> String {
    input.to_hex()
}

fn invocation<'a>(
    t: &'a TestArgs,
    input: Option<&'a str>,
    interrupted: bool,
    effective_check_heap_ownership: Option<bool>,
) -> report::Invocation<'a> {
    report::Invocation {
        interrupted,
        spec: &t.spec,
        target: t.target.as_ref().map(|k| k.path().as_path()),
        input,
        planned_budget: planned_budget(t),
        planned_duration: t.mode.duration_secs(),
        function_selector: t.reported_function_selector(),
        check_heap_ownership: t.check_heap_ownership,
        effective_check_heap_ownership,
        fail_on_crash: t.fail_on_crash,
        multi_stage: !t.is_single_stage(),
    }
}

fn run_mode(t: &TestArgs) -> observe::RunMode {
    use observe::RunMode;
    match t.mode {
        TestMode::SingleInput { .. } => RunMode::SingleInput,
        TestMode::Replay { .. } => RunMode::Replay,
        TestMode::Build => RunMode::Build,
        TestMode::Campaign { .. } => RunMode::Campaign,
    }
}

fn planned_budget(t: &TestArgs) -> Budget {
    match t.mode {
        TestMode::Build => Budget::Build,
        TestMode::SingleInput { .. } => Budget::None,
        TestMode::Replay { .. } => Budget::Tape,
        TestMode::Campaign {
            duration_secs: Some(_),
            ..
        } => Budget::Seconds,
        TestMode::Campaign {
            duration_secs: None,
            ..
        } => Budget::Unbounded,
    }
}

struct BuildPlan {
    compile: Option<harness::Compile>,
    coverage_cflags: Vec<String>,
    cflags: Vec<String>,
    link: Option<austen_lib::external::HarnessLink>,
    expect: Option<harness::Expect>,
}

impl BuildPlan {
    fn same_harness(&self, other: &BuildPlan) -> bool {
        let compile = |plan: &BuildPlan| {
            plan.compile
                .as_ref()
                .map(|c| (c.cc.clone(), c.cflags.clone(), c.env.clone()))
        };
        compile(self) == compile(other)
            && self.coverage_cflags == other.coverage_cflags
            && self.cflags == other.cflags
            && self.link == other.link
    }
}

const INTERNAL_COMPILE: austen_lib::instrumentation::CompileRequest =
    austen_lib::instrumentation::CompileRequest {
        cflags: &["-fsanitize-coverage=trace-pc-guard"],
        env: &[],
    };

fn coverage_mode_cflags(modes: &[args::CoverageMode]) -> Vec<String> {
    modes
        .iter()
        .map(|mode| match mode {
            args::CoverageMode::FunctionTrace => "-finstrument-functions".to_string(),
            args::CoverageMode::FunctionTraceAfterInlining => {
                "-finstrument-functions-after-inlining".to_string()
            }
            mode => format!("-fsanitize-coverage={}", mode.token()),
        })
        .collect()
}

pub(crate) fn takes_opaque_cflag_operand(flag: &str) -> bool {
    matches!(
        flag,
        "-ccc-install-dir"
            | "-gen-cdb-fragment-path"
            | "-ccc-arcmt-migrate"
            | "-ccc-gcc-name"
            | "-ccc-objcmt-migrate"
            | "--analyzer-output"
            | "-arcmt-migrate-report-output"
            | "-B"
            | "-b"
            | "-cxx-isystem"
            | "-darwin-target-variant-triple"
            | "-darwin-target-variant"
            | "-dependency-dot"
            | "-dependency-file"
            | "-dsym-dir"
            | "-dumpdir"
            | "-D"
            | "-fexperimental-openacc-macro-override"
            | "-fmodules-user-build-path"
            | "-F"
            | "-G"
            | "-iapinotes-modules"
            | "-idirafter"
            | "-iframeworkwithsysroot"
            | "-iframework"
            | "-imacros"
            | "-include-pch"
            | "-include-pth"
            | "-include"
            | "-iprefix"
            | "-iquote"
            | "-isysroot"
            | "-isystem-after"
            | "-isystem"
            | "-ivfsoverlay"
            | "-iwithprefixbefore"
            | "-iwithprefix"
            | "-iwithsysroot"
            | "-I"
            | "-L"
            | "-hlsl-entry"
            | "-meabi"
            | "-MF"
            | "-MJ"
            | "-mllvm"
            | "-mmlir"
            | "-module-dependency-dir"
            | "-MQ"
            | "-mthread-model"
            | "-MT"
            | "-o"
            | "-resource-dir"
            | "-serialize-diagnostics"
            | "-stdlib++-isystem"
            | "-T"
            | "-U"
            | "-vfsoverlay"
            | "-working-directory"
            | "-Xanalyzer"
            | "-Xarch_device"
            | "-Xarch_host"
            | "-Xassembler"
            | "-Xclang"
            | "-Xcuda-fatbinary"
            | "-Xcuda-ptxas"
            | "-Xlinker"
            | "-Xopenmp-target"
            | "-Xpreprocessor"
            | "-x"
            | "-z"
            | "-arch"
            | "-target"
            | "--target"
            | "--sysroot"
    ) || flag.starts_with("-Xarch_")
        || flag.starts_with("-Xoffload-linker")
        || flag.starts_with("-Xopenmp-target=")
}

pub(crate) fn interpreted_cflag_words(cflags: &[String]) -> (Vec<&str>, Option<&str>) {
    let mut words = Vec::new();
    let mut awaiting_operand = None;
    let mut options_ended = false;
    for flag in cflags {
        if awaiting_operand.take().is_some() {
            continue;
        }
        if options_ended {
            continue;
        }
        if flag == "--" {
            options_ended = true;
            continue;
        }
        if takes_opaque_cflag_operand(flag) {
            awaiting_operand = Some(flag.as_str());
            continue;
        }
        words.push(flag.as_str());
    }
    (words, awaiting_operand)
}

fn user_coverage_controls(cflags: &[String]) -> Vec<&str> {
    interpreted_cflag_words(cflags)
        .0
        .into_iter()
        .filter(|flag| austen_lib::instrumentation::is_control_word(flag))
        .collect()
}

fn has_user_coverage_controls(cflags: &[String]) -> bool {
    !user_coverage_controls(cflags).is_empty()
}

fn explicit_coverage(t: &TestArgs) -> bool {
    !t.coverage.is_empty() || has_user_coverage_controls(&t.cflags)
}

fn implied_coverage(
    request: austen_lib::instrumentation::CompileRequest,
    coverage_cflags: &[String],
    cflags: &[String],
) -> austen_lib::instrumentation::Implied {
    let controls = user_coverage_controls(cflags);
    austen_lib::instrumentation::implied_by(
        request
            .cflags
            .iter()
            .copied()
            .chain(coverage_cflags.iter().map(String::as_str))
            .chain(controls),
        request.env.iter().copied(),
    )
}

struct CoverageContract {
    engine: &'static str,
    request: austen_lib::instrumentation::CompileRequest,
    consumes: &'static [austen_lib::instrumentation::Coverage],
    rejects: &'static [austen_lib::instrumentation::Coverage],
    blind: bool,
}

impl CoverageContract {
    fn asker(&self) -> String {
        engine_label(self.engine)
    }
}

fn neutral_coverage_contract() -> CoverageContract {
    CoverageContract {
        engine: "exact-input execution",
        request: INTERNAL_COMPILE,
        consumes: austen_lib::instrumentation::EVALUATION_FLAVORS,
        rejects: &[],
        blind: true,
    }
}

fn coverage_contract(engine: austen_lib::AnyEngine) -> CoverageContract {
    use austen_lib::external::DelegatedEngine;
    let blind = engine.descriptor().coverage.can_run_blind;
    match engine {
        austen_lib::AnyEngine::AustenDriven(_) => CoverageContract {
            engine: austen_lib::Engine::name(&engine),
            request: INTERNAL_COMPILE,
            consumes: austen_lib::instrumentation::EVALUATION_FLAVORS,
            rejects: &[],
            blind,
        },
        austen_lib::AnyEngine::Delegated(ext) => CoverageContract {
            engine: DelegatedEngine::name(&ext),
            request: ext.compile_request(),
            consumes: ext.coverage(),
            rejects: ext.rejected_coverage(),
            blind,
        },
    }
}

fn validate_coverage_request(
    request: austen_lib::instrumentation::CompileRequest,
    coverage_cflags: &[String],
    cflags: &[String],
    asker: &str,
    consumes: &[austen_lib::instrumentation::Coverage],
    rejects: &[austen_lib::instrumentation::Coverage],
    can_run_blind: bool,
) -> Result<austen_lib::instrumentation::Implied, Usage> {
    use austen_lib::instrumentation::{self as coverage, Coverage};

    let implied = implied_coverage(request, coverage_cflags, cflags);

    if !implied.invalid.is_empty() {
        let invalid = implied
            .invalid
            .iter()
            .map(|flag| format!("`{flag}`"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Usage::from_message(format!(
            "the selected Clang coverage compiler does not accept {invalid}. \
             Remove the invalid control or replace mutually exclusive coverage insertion shapes with one shape."
        )));
    }

    if !implied.unsupported.is_empty() {
        let controls = implied
            .unsupported
            .iter()
            .map(|control| {
                if control.starts_with('-') {
                    format!("`{control}`")
                } else {
                    format!("`-fsanitize-coverage={control}`")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Usage::from_message(format!(
            "the recognized compiler coverage setting(s) {controls} are not supported by any registered engine; remove them"
        )));
    }

    if !implied.unrecognised.is_empty() || !implied.unrecognised_disabled.is_empty() {
        let enabled = implied
            .unrecognised
            .iter()
            .map(|group| format!("`-fsanitize-coverage={group}`"));
        let disabled = implied
            .unrecognised_disabled
            .iter()
            .map(|group| format!("`-fno-sanitize-coverage={group}`"));
        let unknown = enabled.chain(disabled).collect::<Vec<_>>().join(", ");
        let known = Coverage::SPELLINGS
            .iter()
            .filter_map(|(spelling, _)| match spelling {
                coverage::Spelling::SancovGroup(group)
                | coverage::Spelling::SancovMetadata(group)
                | coverage::Spelling::SancovModifier(group)
                | coverage::Spelling::SancovUnsupported(group) => Some(*group),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Usage::from_message(format!(
            "austen cannot assess the sanitizer-coverage setting {unknown}; \
             recognized groups are: {known}"
        )));
    }

    let dependent = implied
        .flavors
        .iter()
        .copied()
        .filter(|flavor| {
            matches!(
                flavor,
                Coverage::TraceCmp
                    | Coverage::TraceDiv
                    | Coverage::TraceGep
                    | Coverage::IndirectCalls
            )
        })
        .collect::<Vec<_>>();
    let has_base = implied.flavors.iter().any(|flavor| {
        matches!(
            flavor,
            Coverage::TracePcGuard | Coverage::SancovCounters | Coverage::TracePc
        )
    });
    if !dependent.is_empty() && !has_base {
        let dormant = dependent
            .iter()
            .map(|flavor| format!("`{}`", flavor.token()))
            .collect::<Vec<_>>()
            .join(", ");
        let bases = [
            (Coverage::TracePcGuard, "trace-pc-guard"),
            (Coverage::SancovCounters, "inline-8bit-counters"),
            (Coverage::TracePc, "trace-pc"),
        ];
        let selected_consumes_profile = implied
            .flavors
            .iter()
            .all(|flavor| consumes.contains(flavor) && !rejects.contains(flavor));
        let selected_bases = bases
            .iter()
            .filter(|(flavor, _)| consumes.contains(flavor) && !rejects.contains(flavor))
            .map(|(_, group)| format!("`-fsanitize-coverage={group}`"))
            .collect::<Vec<_>>();
        let remedy = if selected_consumes_profile && !selected_bases.is_empty() {
            format!("Add {} as a base mode.", selected_bases.join(" or "))
        } else {
            let alternatives = whole_profile_consumers(&implied.flavors)
                .into_iter()
                .filter_map(|candidate| {
                    let candidate_bases = bases
                        .iter()
                        .filter(|(flavor, _)| coverage_consumers(*flavor).contains(&candidate))
                        .map(|(_, group)| format!("`-fsanitize-coverage={group}`"))
                        .collect::<Vec<_>>();
                    (!candidate_bases.is_empty()).then(|| {
                        format!(
                            "{} with {}",
                            engine_label(candidate),
                            candidate_bases.join(" or ")
                        )
                    })
                })
                .collect::<Vec<_>>();
            if alternatives.is_empty() {
                "Add a guard, inline-counter, or direct-PC base mode and select an engine that supports the resulting complete profile."
                    .to_string()
            } else {
                format!(
                    "Add a base and use {}; switching engines without a base will not activate these callbacks.",
                    alternatives.join(", or ")
                )
            }
        };
        return Err(Usage::from_message(format!(
            "compiler coverage settings leave {dormant} without a base coverage mode, so Clang emits no callbacks for that requested feedback. {remedy}"
        )));
    }

    let rejected = implied
        .flavors
        .iter()
        .copied()
        .filter(|flavor| rejects.contains(flavor))
        .collect::<Vec<_>>();
    let unsupported = implied
        .flavors
        .iter()
        .copied()
        .filter(|flavor| !rejects.contains(flavor) && !consumes.contains(flavor))
        .collect::<Vec<_>>();
    let explicitly_changed = !coverage_cflags.is_empty() || has_user_coverage_controls(cflags);
    if explicitly_changed && implied.flavors.is_empty() && !can_run_blind {
        let alternatives = crate::registry::engines()
            .iter()
            .filter(|engine| engine.descriptor().coverage.can_run_blind)
            .map(|engine| austen_lib::Engine::name(&engine))
            .map(engine_label)
            .collect::<Vec<_>>();
        let remedy = if alternatives.is_empty() {
            "Remove the disabling override; this build has no engine that can run blind."
                .to_string()
        } else {
            format!(
                "Remove the disabling override, or select an engine that can run blind: {}.",
                alternatives.join(" or ")
            )
        };
        return Err(Usage::from_message(format!(
            "compiler coverage settings disable every feedback flavor for {asker}, which cannot run blind. \
             {remedy}"
        )));
    }
    if rejected.is_empty() && unsupported.is_empty() {
        if explicitly_changed {
            tracing::info!(
                target: "austen::build",
                asker,
                coverage = ?implied.flavors.iter().map(|f| f.token()).collect::<Vec<_>>(),
                "user compiler coverage settings are supported"
            );
        }
        return Ok(implied);
    }

    let mut detail = Vec::new();
    for flavor in &rejected {
        detail.push(format!(
            "  `{}`: rejected at run time; {}",
            flavor.token(),
            coverage_support_clause(*flavor)
        ));
    }
    for flavor in &unsupported {
        detail.push(format!(
            "  `{}`: not supported by this engine; {}",
            flavor.token(),
            coverage_support_clause(*flavor)
        ));
    }

    let alternatives = whole_profile_consumers(&implied.flavors);
    let remedy = if alternatives.is_empty() {
        "No registered engine supports the complete effective profile; remove the listed coverage setting(s)."
            .to_string()
    } else {
        format!(
            "Use {} for the complete profile, or remove the listed coverage setting(s).",
            alternatives
                .iter()
                .map(|name| engine_label(name))
                .collect::<Vec<_>>()
                .join(" or ")
        )
    };
    Err(Usage::from_message(format!(
        "compiler coverage settings are incompatible with {asker}:\n{}\n{remedy}",
        detail.join("\n")
    )))
}

fn whole_profile_consumers(flavors: &[austen_lib::instrumentation::Coverage]) -> Vec<&'static str> {
    flavors.first().map_or_else(Vec::new, |first| {
        coverage_consumers(*first)
            .into_iter()
            .filter(|candidate| {
                flavors
                    .iter()
                    .all(|flavor| coverage_consumers(*flavor).contains(candidate))
            })
            .collect()
    })
}

fn coverage_observers(flavor: austen_lib::instrumentation::Coverage) -> Vec<&'static str> {
    if austen_lib::instrumentation::EVALUATION_FLAVORS.contains(&flavor) {
        return crate::registry::engines()
            .iter()
            .filter(|engine| engine.descriptor().coverage.observed_by_austen)
            .map(|engine| austen_lib::Engine::name(&engine))
            .collect();
    }
    Vec::new()
}

fn coverage_consumers(flavor: austen_lib::instrumentation::Coverage) -> Vec<&'static str> {
    coverage_observers(flavor)
        .into_iter()
        .chain(austen_lib::instrumentation::consumers(
            flavor,
            crate::registry::engines(),
        ))
        .collect()
}

fn coverage_support_clause(flavor: austen_lib::instrumentation::Coverage) -> String {
    let observers = coverage_observers(flavor);
    let feedback = austen_lib::instrumentation::consumers(flavor, crate::registry::engines());
    if observers.is_empty() && feedback.is_empty() {
        return austen_lib::instrumentation::steered_by(flavor, crate::registry::engines());
    }
    let mut clauses = Vec::new();
    if !observers.is_empty() {
        clauses.push(format!(
            "Austen observes it for retention under {}",
            observers
                .iter()
                .map(|observer| engine_label(observer))
                .collect::<Vec<_>>()
                .join(" or ")
        ));
    }
    if !feedback.is_empty() {
        clauses.push(format!(
            "{} steer{} by it",
            feedback
                .iter()
                .map(|consumer| engine_label(consumer))
                .collect::<Vec<_>>()
                .join(" or "),
            if feedback.len() == 1 { "s" } else { "" }
        ));
    }
    clauses.join("; ")
}

fn validate_selected_coverage(t: &TestArgs) -> Result<(), Usage> {
    if !explicit_coverage(t) {
        return Ok(());
    }
    let contracts: Vec<CoverageContract> = if t.schedule().is_none() {
        vec![neutral_coverage_contract()]
    } else {
        t.stages()
            .iter()
            .map(|stage| coverage_contract(stage.engine.representative()))
            .collect()
    };
    for contract in contracts {
        validate_coverage_request(
            contract.request,
            &coverage_mode_cflags(&t.coverage),
            &t.cflags,
            &contract.asker(),
            contract.consumes,
            contract.rejects,
            contract.blind,
        )?;
    }
    Ok(())
}

fn compiler_coverage_event_for(t: &TestArgs, engine: austen_lib::AnyEngine) -> RunEvent {
    let coverage_cflags = coverage_mode_cflags(&t.coverage);
    let contract = coverage_contract(engine);
    let flavors = implied_coverage(contract.request, &coverage_cflags, &t.cflags)
        .flavors
        .iter()
        .map(|flavor| flavor.token().to_string())
        .collect();
    let controls = t
        .coverage
        .iter()
        .map(|mode| format!("--coverage={}", mode.token()))
        .chain(
            user_coverage_controls(&t.cflags)
                .into_iter()
                .map(str::to_string),
        )
        .collect();
    RunEvent::CompilerCoverage {
        engine: contract.engine.to_string(),
        controls,
        flavors,
    }
}

fn announce_compiler_coverage(t: &TestArgs, reporter: &observe::Reporter) {
    if explicit_coverage(t) {
        let mut announced: Vec<&'static str> = Vec::new();
        for stage in t.stages() {
            let engine = stage.engine.representative();
            let asker = coverage_contract(engine).engine;
            if announced.contains(&asker) {
                continue;
            }
            announced.push(asker);
            reporter.emit(|| compiler_coverage_event_for(t, engine));
        }
    }
}

enum HarnessSource<'a> {
    Building {
        plan: Box<BuildPlan>,
        target: &'a args::Target,
        stage_plans: Vec<Option<BuildPlan>>,
    },
    Supplied(PathBuf),
}

fn harness_source(t: &TestArgs) -> Result<HarnessSource<'_>, Error> {
    if let Some(supplied) = &t.harness {
        return Ok(HarnessSource::Supplied(absolute(supplied)?));
    }
    let Some(target) = &t.target else {
        return Err(Usage::from_message("no harness and no target").into());
    };
    let mut plans = t
        .stages()
        .iter()
        .map(|stage| {
            plan_for(
                target,
                stage.engine.representative(),
                &t.coverage,
                &t.cflags,
            )
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter();
    let plan = match plans.next() {
        Some(plan) => plan,
        None => plan_for(target, None, &t.coverage, &t.cflags)?,
    };
    let stage_plans = std::iter::once(None)
        .chain(plans.map(|p| (!p.same_harness(&plan)).then_some(p)))
        .collect();
    Ok(HarnessSource::Building {
        plan: Box::new(plan),
        target,
        stage_plans,
    })
}

fn plan_for(
    target: &args::Target,
    engine: impl Into<Option<austen_lib::AnyEngine>>,
    coverage: &[args::CoverageMode],
    cflags: &[String],
) -> Result<BuildPlan, Error> {
    use austen_lib::external::{DelegatedEngine as _, ExternalError};
    let engine = engine.into();
    let coverage_cflags = coverage_mode_cflags(coverage);
    let contract = engine.map_or_else(neutral_coverage_contract, coverage_contract);
    validate_coverage_request(
        contract.request,
        &coverage_cflags,
        cflags,
        &contract.asker(),
        contract.consumes,
        contract.rejects,
        contract.blind,
    )?;
    if engine.is_none_or(|engine| engine.descriptor().loop_owner == austen_lib::LoopOwner::Austen) {
        let instrumented = if matches!(target, args::Target::CSource(_)) {
            austen_lib::instrumentation::coverage_compiler(contract.request)
                .map(harness::Compile::from)
                .map(Some)
                .map_err(ExternalError::from)
        } else {
            Ok(None)
        };
        return match instrumented {
            Ok(compile) => Ok(BuildPlan {
                compile,
                coverage_cflags,
                cflags: cflags.to_vec(),
                link: None,
                expect: Some(harness::Expect {
                    asker: contract.engine,
                    consumes: contract.consumes,
                    rejects: contract.rejects,
                    remedy: "Rebuild the target without that instrumentation, or pass the `.c` source and let austen compile it.",
                }),
            }),
            Err(error) if !coverage_cflags.is_empty() || has_user_coverage_controls(cflags) => {
                Err(error.into())
            }
            Err(_) => Ok(BuildPlan {
                compile: None,
                coverage_cflags,
                cflags: cflags.to_vec(),
                link: None,
                expect: None,
            }),
        };
    }
    let Some(austen_lib::AnyEngine::Delegated(ext)) = engine else {
        unreachable!("only delegated engines reach the delegated build-plan branch")
    };
    let compile = if matches!(target, args::Target::CSource(_)) {
        let compiler = ext.target_compiler().map_err(ExternalError::from)?;
        if let Some(compiler) = &compiler {
            debug_assert_eq!(
                compiler.request, contract.request,
                "DelegatedEngine::compile_request and target_compiler disagreed"
            );
        }
        compiler.map(harness::Compile::from)
    } else {
        None
    };
    let link = ext.harness_link().map_err(ExternalError::from)?;
    let expect = Some(harness::Expect {
        asker: contract.engine,
        consumes: contract.consumes,
        rejects: contract.rejects,
        remedy: "Rebuild the target for this engine, select one that consumes it, \
                 or pass the `.c` source and let austen compile it.",
    });
    Ok(BuildPlan {
        compile,
        coverage_cflags,
        cflags: cflags.to_vec(),
        link,
        expect,
    })
}

fn secondary_plan_for(
    target: &args::Target,
    engine: impl Into<Option<austen_lib::AnyEngine>>,
    cflags: &[String],
) -> Result<BuildPlan, Error> {
    plan_for(target, engine, &[], cflags)
}

fn coverage_plan(target: &args::Target, cflags: &[String]) -> Result<BuildPlan, Error> {
    use austen_lib::{external::ExternalError, instrumentation as coverage};
    let implied = validate_coverage_request(
        coverage::EVALUATION,
        &[],
        cflags,
        "common coverage evaluation",
        coverage::EVALUATION_FLAVORS,
        &[],
        true,
    )?;
    if !implied
        .metadata
        .iter()
        .any(|metadata| metadata == coverage::PC_TABLE)
    {
        return Err(Usage::from_message(
            "common coverage evaluation needs `pc-table` metadata for its exact instrumented-site denominator; remove the setting that disables or resets it, or add `-fsanitize-coverage=pc-table` afterwards",
        )
        .into());
    }
    let compile = if matches!(target, args::Target::CSource(_)) {
        Some(harness::Compile::from(
            coverage::evaluation_compiler().map_err(ExternalError::from)?,
        ))
    } else {
        None
    };
    Ok(BuildPlan {
        compile,
        coverage_cflags: Vec::new(),
        cflags: cflags.to_vec(),
        link: None,
        expect: Some(harness::Expect {
            asker: "common coverage evaluation",
            consumes: coverage::EVALUATION_FLAVORS,
            rejects: &[],
            remedy: "Rebuild the target without that instrumentation, or pass the \
                     `.c` source and let austen compile it — the evaluation build \
                     is the same for every engine, so switching engines cannot \
                     help.",
        }),
    })
}

fn harness_kind(source: &HarnessSource, reporter: &observe::Reporter) -> Option<HarnessKind> {
    match source {
        HarnessSource::Building { plan, .. } => Some(HarnessKind::of_link(plan.link.as_ref())),
        HarnessSource::Supplied(bin) => {
            match HarnessKind::of_binary(bin, crate::registry::engines()) {
                Ok(kind) => Some(kind),
                Err(e) => {
                    reporter.emit(|| {
                        RunEvent::Warning(observe::Warning::HarnessKindUnknown {
                            harness: bin.clone(),
                            message: e.to_string(),
                        })
                    });
                    None
                }
            }
        }
    }
}

struct ShrinkSetup {
    shrinker: AnyShrinker,
    build: Option<BuildPlan>,
}

fn resolve_shrinker(
    t: &TestArgs,
    source: &HarnessSource,
    reporter: &observe::Reporter,
) -> Result<Option<ShrinkSetup>, Error> {
    let Some(choice) = t.shrink else {
        return Ok(None);
    };
    let kind = harness_kind(source, reporter);
    let shrinker = match choice {
        args::ShrinkChoice::Default => {
            crate::registry::shrinker_for_harness(kind.unwrap_or(HarnessKind::Forkserver))
                .or_else(|| crate::registry::shrinkers().next())
                .expect("parsing refuses a bare --shrink without a reducer feature")
        }
        args::ShrinkChoice::Named(s) => s,
    };
    let reuse = shrink_harness_reusable(kind, &shrinker, !t.coverage.is_empty());
    let build = if reuse {
        None
    } else {
        match source {
            HarnessSource::Supplied(_) => {
                return Err(Usage::from_message(format!(
                    "--shrink={} drives the `{}` harness kind and --harness supplied a \
                     `{}` one, which cannot be rebuilt. Drop --harness so austen can \
                     build it one, or enable and select a minimizer for the supplied harness kind.",
                    Shrinker::name(&shrinker),
                    Shrinker::drives(&shrinker).token(),
                    kind.expect("a mismatch implies a measured kind").token(),
                ))
                .into());
            }
            HarnessSource::Building { target, .. } => {
                let engine = Shrinker::builds_with(&shrinker)
                    .map(austen_lib::external::AnyDelegatedEngine::Fuzzer)
                    .map(austen_lib::AnyEngine::Delegated)
                    .or_else(|| {
                        t.schedule()
                            .and_then(austen_lib::Schedule::first_austen_driven)
                    });
                Some(secondary_plan_for(target, engine, &t.cflags)?)
            }
        }
    };
    let path = shrinker.preflight().map_err(Error::ShrinkNotFound)?;
    reporter.emit(|| RunEvent::tool_selected(shrinker.name(), path));
    Ok(Some(ShrinkSetup { shrinker, build }))
}

fn shrink_harness_reusable(
    kind: Option<HarnessKind>,
    shrinker: &AnyShrinker,
    has_campaign_coverage: bool,
) -> bool {
    !has_campaign_coverage && kind.is_none_or(|candidate| Shrinker::drives(shrinker) == candidate)
}

fn run_test(
    t: &TestArgs,
    supplied_input: Option<&austen_lib::wire::AustenInput>,
    trace: &mut Trace,
    reporter: &observe::Reporter,
    cancel: &observe::Cancel,
) -> Result<austen_lib::driver::Report, Error> {
    trace.function.clone_from(&t.function);
    validate_selected_coverage(t)?;
    let compile_cwd = (!t.cflags.is_empty())
        .then(|| absolute(Path::new(".")))
        .transpose()?;
    let spec = absolute(&t.spec)?;
    let target = t.target.as_ref().map(|t| absolute(t.path())).transpose()?;
    let custom_generators = if t
        .schedule()
        .is_some_and(austen_lib::driver::uses_tree_generators)
        && !t.ignore_custom_generators
    {
        Some(
            austen_lib::driver::LoadedGeneratorSources::load(&spec, t.function.as_deref())
                .map_err(austen_lib::driver::DriverError::from)?,
        )
    } else {
        None
    };
    let custom_generator_identity = custom_generators
        .as_ref()
        .and_then(austen_lib::driver::LoadedGeneratorSources::identity_bytes);
    let selected_engine = t.canonical_engine();
    let engine_matters = !matches!(t.mode, TestMode::SingleInput { .. });
    let supplied_bin = t.harness.as_deref().map(absolute).transpose()?;
    let explicit_manifest = t.harness_manifest.is_some();
    let manifest_path = if let Some(path) = &t.harness_manifest {
        Some(absolute(path)?)
    } else if let Some(bin) = supplied_bin.as_deref() {
        let path = bin
            .parent()
            .expect("an absolute harness path has a parent")
            .join(prepared::NAME);
        path.is_file().then(|| absolute(&path)).transpose()?
    } else {
        None
    };
    let single_stage = t.is_single_stage();
    let prepared = if let Some(path) = &manifest_path {
        if explicit_manifest && !single_stage {
            return Err(Usage::from_message(
                "--harness-manifest currently requires a single-stage --engine",
            )
            .into());
        }
        if !single_stage {
            None
        } else {
            let manifest = prepared::read(path)
                .map_err(|why| Usage::from_message(format!("prepared harness: {why}")))?;
            if !explicit_manifest
                && !manifest.matches_engine(engine_matters.then_some(selected_engine.as_str()))
            {
                None
            } else {
                let bin = supplied_bin
                    .as_deref()
                    .expect("a manifest requires a supplied harness");
                Some(
                    prepared::load(
                        path,
                        manifest,
                        bin,
                        &spec,
                        engine_matters.then_some(selected_engine.as_str()),
                        t.stages().first().is_none_or(|stage| {
                            match stage.engine.representative() {
                                austen_lib::AnyEngine::AustenDriven(_) => true,
                                austen_lib::AnyEngine::Delegated(engine) => {
                                    austen_lib::DelegatedEngine::capabilities(&engine)
                                        .austen_coverage
                                }
                            }
                        }),
                        !t.no_coverage,
                    )
                    .map_err(|why| Usage::from_message(format!("prepared harness: {why}")))?,
                )
            }
        }
    } else {
        None
    };

    let requested_work = match &t.work {
        Some(dir) => dir.clone(),
        None => cache_dir().join("runs").join(run_key_with_options(
            &spec,
            target.as_deref(),
            t.function.as_deref(),
            t.pass_options,
            t.reported_max_generator_size(),
            t.reported_size_selector(),
            t.reported_generator_timeout(),
            &t.coverage,
            &t.cflags,
            compile_cwd.as_deref(),
            t.check_heap_ownership,
            custom_generator_identity.as_deref(),
            t.no_coverage,
        )),
    };
    let mut cli_protected = cli_protected_paths(t, &spec, target.as_deref());
    if let Some(sources) = &custom_generators {
        cli_protected.extend(sources.protected_paths());
    }
    if let Some(bin) = &supplied_bin {
        cli_protected.push(austen_lib::ProtectedPath::new(
            "resolved supplied harness",
            bin.clone(),
        ));
    }
    if prepared.is_some()
        && let Some(path) = &manifest_path
    {
        cli_protected.push(austen_lib::ProtectedPath::new(
            "prepared harness manifest",
            path.clone(),
        ));
    }
    if let Some(prepared) = &prepared {
        cli_protected.extend(prepared.protected_paths());
    }
    let work = prepare_work_dir(&requested_work, t.clean, &cli_protected)?;
    trace.work = Some(work.clone());
    reporter.emit(|| {
        RunEvent::RunStarted(observe::RunStarted {
            spec: spec.clone(),
            target: target.clone(),
            harness: t.harness.clone(),
            work: work.clone(),
            engine: t.schedule().map(austen_lib::Schedule::canonical_engine),
            mode: run_mode(t),
            budget_secs: t.mode.duration_secs(),
        })
    });

    let source = harness_source(t)?;
    announce_compiler_coverage(t, reporter);

    let (shrinker, shrink_plan) = match resolve_shrinker(t, &source, reporter)? {
        Some(s) => (Some(s.shrinker), s.build),
        None => (None, None),
    };
    trace.resolved_shrinker = shrinker
        .as_ref()
        .map(|shrinker| shrinker.name().to_string());
    let suite_minimizer = t.minimize_suite.then(|| {
        crate::registry::suite_minimizer()
            .expect("parsing refuses --minimize-suite without the `aflpp` feature")
    });
    if let Some(tool) = &suite_minimizer {
        let path = SuiteTool::preflight(tool).map_err(Error::ShrinkNotFound)?;
        reporter.emit(|| RunEvent::tool_selected(SuiteTool::name(tool), path));
    }

    let mut shrink_harness = None;
    let mut stage_harnesses: Vec<Option<austen_lib::driver::HarnessFacts>> = Vec::new();
    let cov_harness;
    let built = match source {
        HarnessSource::Supplied(bin) => {
            if let Some(prepared) = prepared {
                trace.effective_check_heap_ownership = Some(prepared.check_heap_ownership);
                cov_harness = prepared.evaluation;
                prepared.built
            } else {
                cov_harness = Err(if t.no_coverage {
                    prepared::COVERAGE_DISABLED.to_string()
                } else {
                    concat!(
                        "--harness supplies a binary austen did not build, so ",
                        "there is no evaluation binary to measure through; drop ",
                        "--harness and pass the target, or measure a run that ",
                        "builds one",
                    )
                    .to_string()
                });
                harness::Built::prebuilt(bin)
            }
        }
        HarnessSource::Building {
            plan,
            target: kind,
            stage_plans,
        } => {
            let preserve_campaign = matches!(
                t.mode,
                TestMode::Replay { .. } | TestMode::SingleInput { .. }
            ) && work.join("run.sh").exists();
            let build_dir = if preserve_campaign {
                work.join("replay-harness")
            } else {
                work.clone()
            };
            let rt = {
                let _phase = reporter.phase(observe::Phase::Runtime);
                runtime::find()?
            };
            trace.runtime = Some(provenance::runtime_provenance(&rt));
            if runtime::is_stale(&rt) {
                reporter.emit(|| {
                    RunEvent::Warning(observe::Warning::StaleRuntime {
                        dir: rt.dir.clone(),
                        installed: rt.stamp.version.clone(),
                        running: env!("CARGO_PKG_VERSION").to_string(),
                    })
                });
            }
            if runtime::is_foreign(&rt) {
                reporter.emit(|| {
                    RunEvent::Warning(observe::Warning::ForeignRuntime {
                        dir: rt.dir.clone(),
                        staged_from: rt.stamp.source.clone(),
                        running_from: runtime::source().to_string(),
                        overridden: rt.origin == runtime::RuntimeOrigin::Override,
                    })
                });
            }
            let build_at = |dir: &Path, scripts: bool, p: BuildPlan| -> Result<_, Error> {
                std::fs::create_dir_all(dir).map_err(|source| Error::Io {
                    path: dir.to_path_buf(),
                    source,
                })?;
                let BuildPlan {
                    compile,
                    coverage_cflags,
                    cflags,
                    link,
                    expect,
                } = p;
                Ok(harness::build(&harness::Request {
                    runtime: &rt,
                    spec: &spec,
                    custom_generators: custom_generators.as_ref(),
                    ignore_custom_generators: t.ignore_custom_generators,
                    target: kind,
                    target_path: target.as_deref().expect("a target implies a path"),
                    dir,
                    scripts,
                    pass_options: t.pass_options,
                    check_heap_ownership: t.check_heap_ownership,
                    allow_unstable_addresses: t.allow_unstable_addresses,
                    no_coverage: t.no_coverage,
                    function: t.function.as_deref(),
                    function_selector: t.reported_function_selector(),
                    max_generator_size: t.reported_max_generator_size(),
                    size_selector: t.reported_size_selector(),
                    generator_timeout: t.reported_generator_timeout(),
                    engine: t
                        .stages()
                        .first()
                        .filter(|_| {
                            !matches!(t.mode, TestMode::Replay { .. }) || t.engine.is_some()
                        })
                        .map(|stage| stage.engine.name()),
                    engine_descriptor: t
                        .stages()
                        .first()
                        .map_or(crate::registry::NEUTRAL_DESCRIPTOR, |stage| {
                            stage.engine.descriptor()
                        }),
                    chain_discard_limit: t.reported_chain_discard_limit(),
                    compile,
                    coverage_cflags: &coverage_cflags,
                    cflags: &cflags,
                    compile_cwd: compile_cwd.as_deref(),
                    link,
                    expect,
                    reporter,
                })?)
            };
            let campaign = build_at(&build_dir, !preserve_campaign, *plan)?;
            for (index, stage_plan) in stage_plans.into_iter().enumerate() {
                let Some(stage_plan) = stage_plan else {
                    stage_harnesses.push(None);
                    continue;
                };
                let built = build_at(
                    &build_dir.join("stage-harness").join(index.to_string()),
                    false,
                    stage_plan,
                )?;
                stage_harnesses.push(Some(austen_lib::driver::HarnessFacts {
                    harness: built.harness,
                    instrumented: built.instrumented,
                    instrumentation: built.instrumentation,
                    coverage_guards: built.coverage_guards,
                }));
            }
            if let Some(shrink_plan) = shrink_plan {
                shrink_harness =
                    Some(build_at(&build_dir.join("shrink-harness"), false, shrink_plan)?.harness);
            }
            cov_harness = if t.no_coverage {
                Err(prepared::COVERAGE_DISABLED.to_string())
            } else {
                let dir = build_dir.join("cov-harness");
                let evaluation = coverage_plan(kind, &t.cflags)
                    .and_then(|p| build_at(&dir, false, p))
                    .map(|built| austen_lib::driver::EvaluationBinary {
                        harness: built.harness,
                        object: built.object,
                        instrumentation: built.instrumentation,
                    });
                if t.minimize_suite {
                    let binary = evaluation?;
                    if let Err(why) = binary.observed() {
                        return Err(Usage::from_message(format!(
                            "--minimize-suite needs observed coverage instrumentation, but {why}"
                        ))
                        .into());
                    }
                    Ok(binary)
                } else {
                    evaluation.map_err(|e| e.to_string())
                }
            };
            let manifest_path = build_dir.join(prepared::NAME);
            prepared::write(
                &build_dir,
                &spec,
                t.schedule().map(|_| selected_engine.as_str()),
                &campaign,
                &cov_harness,
                t.check_heap_ownership,
            )
            .map_err(|message| Error::Io {
                path: manifest_path,
                source: std::io::Error::other(message),
            })?;
            trace.effective_check_heap_ownership = Some(t.check_heap_ownership);
            campaign
        }
    };
    let harness_bin = built.harness;
    trace.harness = Some(harness_bin.clone());

    let mut cfg =
        austen_lib::driver::RunConfig::new(spec, harness_bin, work.clone(), t.schedule().cloned());
    cfg.custom_generators = custom_generators;
    cfg.ignore_custom_generators = t.ignore_custom_generators;
    cfg.tyche.clone_from(&t.tyche);
    cfg.exit_fast = t.exit_fast;
    cfg.strict_duration = t.strict_duration;
    cfg.allow_unstable_addresses = t.allow_unstable_addresses;
    cfg.additional_protected_paths = cli_protected;
    cfg.stage_harnesses = stage_harnesses;
    cfg.duration_secs = t.mode.duration_secs();
    cfg.corpus_snapshot_interval = t.corpus_snapshot_interval;
    match &t.mode {
        TestMode::Campaign { seed, record, .. } => {
            cfg.seed = *seed;
            if t.descriptor().generic_seed && !t.output.verbosity.quiet {
                eprintln!("Using seed: {seed}");
            }
            cfg.record.clone_from(record);
        }
        TestMode::Build => cfg.build_only = true,
        TestMode::Replay { tape, record } => {
            cfg.replay = Some(tape.clone());
            cfg.record.clone_from(record);
        }
        TestMode::SingleInput { .. } => {
            cfg.input = Some(
                supplied_input
                    .expect("single-input mode was resolved before staging")
                    .clone(),
            );
        }
    }
    cfg.function.clone_from(&t.function);
    cfg.dsl_log_dir.clone_from(&t.dsl_log_dir);
    cfg.pass_options = t.pass_options;
    cfg.function_selector = t.function_selector;
    cfg.max_test_case_size = t.max_generator_size;
    cfg.size_selector = t.size_selector;
    cfg.generator_timeout = t.reported_generator_timeout().applied();
    cfg.chain_discard_limit = t.reported_chain_discard_limit();
    cfg.instrumented = built.instrumented;
    cfg.instrumentation = built.instrumentation;
    cfg.coverage_guards = built.coverage_guards;
    if t.mode.draws_entropy() && t.descriptor().generic_seed {
        trace.entropy = Some(austen_lib::driver::EntropySource::planned(&cfg));
    }
    cfg.cov_harness = cov_harness;
    cfg.crashes = t.crashes.clone().unwrap_or_else(|| work.join("crashes"));
    cfg.corpus = work.join("corpus");
    trace.crashes = Some(cfg.crashes.clone());
    trace.corpus = Some(cfg.corpus.clone());
    cfg.shrink = shrinker;
    cfg.shrink_harness = shrink_harness;
    cfg.suite_minimizer = suite_minimizer;
    let verbosity = t.output.verbosity;
    cfg.tool_output = if verbosity.level >= 1 && !verbosity.quiet && !t.output.dynamic() {
        austen_lib::external::ToolOutput::Inherit
    } else {
        austen_lib::external::ToolOutput::Silence
    };
    cfg.reporter = reporter.clone();
    cfg.cancel = cancel.clone();
    cfg.debug_child = std::env::var_os(austen_lib::driver::DEBUG_CHILD_ENV).is_some();

    let started = std::time::Instant::now();
    let outcome = austen_lib::driver::run(&cfg);
    reporter.emit(|| RunEvent::RunFinished {
        outcome: match (&outcome, cancel.is_cancelled()) {
            (_, true) => observe::Outcome::Interrupted,
            (Ok(_), false) => observe::Outcome::Completed,
            (Err(_), false) => observe::Outcome::Failed,
        },
        elapsed: started.elapsed(),
    });
    let report = outcome?;
    if let Some(outcome) = &report.campaign.external {
        write_fuzz_script(&work, outcome)?;
    }
    Ok(report)
}

fn cli_protected_paths(
    t: &TestArgs,
    resolved_spec: &Path,
    resolved_target: Option<&Path>,
) -> Vec<austen_lib::ProtectedPath> {
    let mut paths = vec![
        austen_lib::ProtectedPath::new("specification spelling", &t.spec),
        austen_lib::ProtectedPath::new("resolved specification", resolved_spec),
    ];
    if let Some(target) = &t.target {
        paths.push(austen_lib::ProtectedPath::new(
            "original target spelling",
            target.path(),
        ));
    }
    if let Some(path) = resolved_target {
        paths.push(austen_lib::ProtectedPath::new("resolved target", path));
    }
    if let Some(path) = &t.harness {
        paths.push(austen_lib::ProtectedPath::new(
            "supplied harness spelling",
            path,
        ));
    }
    if let Some(path) = &t.harness_manifest {
        paths.push(austen_lib::ProtectedPath::new(
            "supplied harness manifest",
            path,
        ));
    }
    if let TestMode::Replay { tape, .. } = &t.mode {
        paths.push(austen_lib::ProtectedPath::new(
            "replay recording spelling",
            tape,
        ));
    }
    if let Some(path) = &t.report {
        paths.push(austen_lib::ProtectedPath::new("report destination", path));
    }
    if let Some(path) = &t.tyche {
        paths.push(austen_lib::ProtectedPath::new("Tyche destination", path));
    }
    if let Some(path) = t
        .output
        .events
        .as_ref()
        .filter(|path| path.as_os_str() != "-")
    {
        paths.push(austen_lib::ProtectedPath::new("event stream", path));
    }
    paths
}

fn summarize(
    report: &austen_lib::driver::Report,
    doc: &RunReport,
    stats_blocks: &[args::StatBlock],
    dsl_log_dir: Option<&Path>,
) {
    let post_note = || {
        let k = report.campaign.findings.post_failed;
        if k > 0 {
            format!(
                " ({k} postcondition violation{})",
                if k == 1 { "" } else { "s" }
            )
        } else {
            String::new()
        }
    };
    let corpus_note = || {
        if let Some(kept) = &report.campaign.corpus
            && kept.unique > 0
        {
            eprintln!(
                "  corpus: {} distinct input{} in {}",
                kept.unique,
                if kept.unique == 1 { "" } else { "s" },
                kept.dir.display(),
            );
        }
    };
    if report.stages.len() > 1 {
        for stage in &report.stages {
            let counts = match stage.campaign.as_ref() {
                Some(campaign) => match (&campaign.internal, campaign.tally) {
                    (Some(stats), _) => format!(
                        "{} inputs, {} crashes",
                        stats.inputs,
                        campaign.findings.confirmed()
                    ),
                    (None, Some(tally)) => format!(
                        "{} inputs seen, {} crashes",
                        tally.seen,
                        campaign.findings.confirmed()
                    ),
                    (None, None) => format!("{} crashes", campaign.findings.confirmed()),
                },
                None => "no report".to_string(),
            };
            eprintln!(
                "  stage {}/{} {}: {:.1} s, {counts}",
                stage.index + 1,
                report.stages.len(),
                stage.engine,
                stage.elapsed.as_secs_f64(),
            );
        }
    }
    if let Some(stats) = &report.campaign.internal {
        eprintln!(
            "done: {} inputs, {} crashes{}{}{}",
            stats.inputs,
            report.campaign.findings.confirmed(),
            post_note(),
            if stats.rejected > 0 {
                format!(" ({} attempts rejected)", stats.rejected)
            } else {
                String::new()
            },
            doc.seed
                .map_or_else(String::new, |seed| format!(", seed: {seed:016x}")),
        );
        eprint!(
            "{}",
            statistics::render(doc.functions.as_deref(), stats_blocks, dsl_log_dir)
        );
        corpus_note();
        eprintln!("{}", coverage_summary(&report.coverage));
        return;
    }
    let Some(outcome) = &report.campaign.external else {
        corpus_note();
        eprintln!("{}", coverage_summary(&report.coverage));
        eprint!(
            "{}",
            statistics::render(doc.functions.as_deref(), stats_blocks, dsl_log_dir)
        );
        return;
    };

    let count = |n: Option<u64>| n.map_or_else(|| "?".to_string(), |n| n.to_string());
    eprintln!(
        "done ({}): {} inputs seen, {} selected, {} dispatched, {} crashes{}",
        if outcome.instrumented {
            "coverage-guided"
        } else {
            "blind"
        },
        count(outcome.tally.map(|t| t.seen)),
        count(outcome.tally.map(|t| t.selected)),
        count(outcome.tally.map(|t| t.dispatched)),
        report.campaign.findings.confirmed(),
        post_note(),
    );
    eprint!(
        "{}",
        statistics::render(doc.functions.as_deref(), stats_blocks, dsl_log_dir)
    );
    corpus_note();
    let found = &report.campaign.findings;
    if found.artifacts != found.confirmed() {
        eprintln!(
            "  from {} saved artifact{} ({} distinct, {} replayed{}: {})",
            found.artifacts,
            if found.artifacts == 1 { "" } else { "s" },
            found.unique,
            found.replayed(),
            if found.capped() { " — capped" } else { "" },
            found
                .verdicts
                .iter()
                .map(|(v, n)| format!("{n} {}", v.token()))
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    eprintln!("{}", coverage_summary(&report.coverage));
}

fn coverage_summary(outcome: &austen_lib::driver::CoverageOutcome) -> String {
    match outcome {
        austen_lib::driver::CoverageOutcome::Skipped(reason) => {
            format!("  coverage: N/A ({reason})")
        }
        austen_lib::driver::CoverageOutcome::Measured(coverage) => match coverage.sites() {
            Some(sites) => format!(
                "  coverage: {} / {sites} instrumented blocks",
                coverage.covered()
            ),
            None => {
                "  coverage: N/A (evaluation binary reported no instrumented sites)".to_string()
            }
        },
    }
}

fn write_fuzz_script(
    work: &Path,
    outcome: &austen_lib::external::ExternalOutcome,
) -> Result<(), Error> {
    let mut script = format!(
        "#!/bin/sh\n\
         # @generated by `austen test --engine={}` — safe to edit, safe to delete.\n\
         set -eu\n\n",
        outcome.engine,
    );
    for key in austen_lib::wire::mode_vars() {
        script.push_str(&format!("unset {key}\n"));
    }
    for (key, value) in outcome.launch_env() {
        script.push_str(&format!("export {key}={}\n", harness::shell_quote(value)));
    }
    script.push('\n');
    script.push_str("exec ");
    script.push_str(&harness::shell_line(
        &outcome.program.display().to_string(),
        &outcome.argv,
    ));
    script.push('\n');

    let path = work.join("fuzz.sh");
    std::fs::write(&path, &script).map_err(|source| Error::Io {
        path: path.clone(),
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    Ok(())
}
