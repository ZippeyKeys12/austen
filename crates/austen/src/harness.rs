use austen_lib::{object, observe};
use std::path::{Path, PathBuf};
use std::process::Command;

use austen_lib::object::prepare::{
    self as object_prepare, DEFAULT_OBJCOPY, PRIVATE_MAIN_SYMBOL, PreparationPlan, PreparedLayout,
    first_unused_name,
};
use austen_lib::wire::{ALLOCATION_REDIRECTS, Region};

use crate::args::Target;
use crate::runtime::{LINK_NAME, Runtime};
use crate::shim::{function_link_names, generate_with_link_names};

const SHIM_PLACEHOLDER: &str = "{{SHIM}}";

const DRIVER_PLACEHOLDER: &str = "{{DRIVER}}";

const HOOKS_PLACEHOLDER: &str = "{{HOOKS}}";

const BINARY_MARKER_PLACEHOLDER: &str = "{{BINARY_MARKER}}";

const TEST_CFLAGS: &[&str] = &["-D__AUSTEN_TEST"];

const DEFAULT_CFLAGS: &[&str] = &["-fno-stack-protector"];

const GLOBAL_LAYOUT_CFLAGS: &[&str] = &["-fdata-sections", "-fno-common"];

const GLOBAL_BASE: u64 = 0x6000_0000;
const GLOBAL_ALIGNMENT: u64 = 64 * 1024;
const GLOBAL_SECTION_PREFIX: &str = ".austen.global";

pub struct Compile {
    pub cc: PathBuf,
    pub cflags: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub type Link = austen_lib::external::HarnessLink;

pub struct Expect {
    pub asker: &'static str,
    pub consumes: &'static [austen_lib::instrumentation::Coverage],
    pub rejects: &'static [austen_lib::instrumentation::Coverage],
    pub remedy: &'static str,
}

impl From<austen_lib::external::TargetCompiler> for Compile {
    fn from(tc: austen_lib::external::TargetCompiler) -> Compile {
        Compile {
            cc: tc.program,
            cflags: tc.request.cflags.iter().map(|f| f.to_string()).collect(),
            env: tc
                .request
                .env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }
}

pub struct Built {
    pub harness: PathBuf,
    pub object: Option<PathBuf>,
    pub instrumented: bool,
    pub instrumentation: austen_lib::instrumentation::Instrumentation,
    pub coverage_guards: Option<u64>,
}

impl Built {
    pub fn prebuilt(harness: PathBuf) -> Self {
        Self {
            harness,
            object: None,
            instrumented: false,
            instrumentation: austen_lib::instrumentation::Instrumentation::Unknown,
            coverage_guards: None,
        }
    }
}

pub struct Request<'a> {
    pub runtime: &'a Runtime,
    pub spec: &'a Path,
    pub custom_generators: Option<&'a austen_lib::driver::LoadedGeneratorSources>,
    pub ignore_custom_generators: bool,
    pub target: &'a Target,
    pub target_path: &'a Path,
    pub dir: &'a Path,
    pub scripts: bool,
    pub pass_options: austen_lib::driver::PassOptions,
    pub check_heap_ownership: bool,
    pub allow_unstable_addresses: bool,
    pub no_coverage: bool,
    pub function: Option<&'a str>,
    pub function_selector: Option<austen_lib::FunctionSelectorKind>,
    pub max_generator_size: Option<austen_lib::driver::TestCaseSize>,
    pub size_selector: Option<austen_lib::SizeSelectorKind>,
    pub generator_timeout: crate::args::GeneratorTimeout,
    pub engine: Option<&'static str>,
    pub engine_descriptor: austen_lib::EngineDescriptor,
    pub chain_discard_limit: Option<std::num::NonZeroU64>,
    pub compile: Option<Compile>,
    pub coverage_cflags: &'a [String],
    pub cflags: &'a [String],
    pub compile_cwd: Option<&'a Path>,
    pub link: Option<Link>,
    pub expect: Option<Expect>,
    pub reporter: &'a observe::Reporter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    CompileTarget,
    PrepareTarget,
    LinkHarness,
}

impl Step {
    fn what(self) -> &'static str {
        match self {
            Step::CompileTarget => "a C compiler",
            Step::PrepareTarget => "object/archive preparation tools (`objcopy` and `ar`)",
            Step::LinkHarness => "a Rust compiler",
        }
    }
}

impl std::fmt::Display for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Step::CompileTarget => "compiling the target",
            Step::PrepareTarget => "preparing target symbols and sections",
            Step::LinkHarness => "compiling the generated harness",
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("snapshot the specification: {source}")]
    SpecSnapshot {
        #[source]
        source: austen_lib::driver::GeneratorSourceError,
    },
    #[error("parse {path}: {message}")]
    ParseSpec { path: PathBuf, message: String },
    #[error("generate the shim for {path}: {message}")]
    Shim { path: PathBuf, message: String },
    #[error(
        "the runtime template {path} has no {SHIM_PLACEHOLDER} marker — \
         the runtime directory does not match this austen"
    )]
    Template { path: PathBuf },
    #[error(
        "could not run `{program}`: {source}\n\
         austen test builds the generated harness, so it needs {what}. \
         Install it, or pass --harness=<already-built-harness>.",
        what = .step.what()
    )]
    Spawn {
        program: String,
        step: Step,
        #[source]
        source: std::io::Error,
    },
    #[error("`{program}` failed with {status} while {step}")]
    Failed {
        program: String,
        status: std::process::ExitStatus,
        step: Step,
    },
    #[error("could not prepare the target entry point in {path}: {message}")]
    PreparedTarget { path: PathBuf, message: String },
    #[error("could not prepare specified globals in {path}: {message}")]
    PreparedGlobals { path: PathBuf, message: String },
    #[error("{message}")]
    ForeignInstrumentation { message: String },
}

fn tool(env: &str, default: &str) -> String {
    std::env::var(env).unwrap_or_else(|_| default.to_string())
}

fn objcopy_tool() -> String {
    tool("OBJCOPY", DEFAULT_OBJCOPY)
}

fn compiler(
    compile: Option<&Compile>,
    coverage_cflags: &[String],
    extra_cflags: &[String],
) -> (String, Vec<String>, Vec<(String, String)>) {
    let (program, mut cflags, env) = match compile {
        Some(c) => (c.cc.display().to_string(), c.cflags.clone(), c.env.clone()),
        None => (
            tool("CC", "cc"),
            DEFAULT_CFLAGS.iter().map(|flag| flag.to_string()).collect(),
            Vec::new(),
        ),
    };
    cflags.extend(coverage_cflags.iter().cloned());
    cflags.extend(extra_cflags.iter().cloned());
    cflags.extend(TEST_CFLAGS.iter().map(|flag| flag.to_string()));
    (program, cflags, env)
}

fn source_compile_args(
    mut cflags: Vec<String>,
    private_main: &str,
    source: &Path,
    wrapper_source: Option<&Path>,
    object: &Path,
) -> Vec<String> {
    cflags.push(format!("-Dmain={private_main}"));
    cflags.extend(GLOBAL_LAYOUT_CFLAGS.iter().map(|flag| flag.to_string()));
    let primary = if let Some(wrapper_source) = wrapper_source {
        cflags.extend(["-include".to_string(), source.display().to_string()]);
        wrapper_source
    } else {
        source
    };
    cflags.extend([
        "-c".to_string(),
        primary.display().to_string(),
        "-o".to_string(),
        object.display().to_string(),
    ]);
    cflags
}

fn source_main_symbol(path: &Path) -> Result<String, BuildError> {
    let source = std::fs::read(path).map_err(|source| BuildError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hash = crate::stamp::Fnv1a::new();
    hash.eat(path.as_os_str().as_encoded_bytes());
    let keyed = format!("{PRIVATE_MAIN_SYMBOL}_{}", hash.hex());
    Ok(first_unused_name(&keyed, |candidate| {
        source
            .windows(candidate.len())
            .any(|window| window == candidate.as_bytes())
    }))
}

const UNINTERPOSED_ALLOCATOR_ENTRIES: &[&str] = &[
    "reallocarray",
    "posix_memalign",
    "memalign",
    "valloc",
    "pvalloc",
    "cfree",
    "mmap",
    "munmap",
    "mremap",
    "brk",
    "sbrk",
];

fn allocation_lifetime_preserving_import(name: &str) -> bool {
    use austen_lib::instrumentation::Coverage;

    if matches!(
        name,
        "abort"
            | "_exit"
            | "__assert_fail"
            | "memcpy"
            | "memmove"
            | "memset"
            | "memcmp"
            | "strlen"
            | "strcmp"
            | "strncmp"
            | "__stack_chk_fail"
            | "__stack_chk_guard"
            | "_GLOBAL_OFFSET_TABLE_"
            | "__divti3"
            | "__udivti3"
            | "__modti3"
            | "__umodti3"
            | "__multi3"
            | "__ashlti3"
            | "__ashrti3"
            | "__lshrti3"
            | "__sanitizer_cov_pcs_init"
            | "__cyg_profile_func_exit"
            | "__start___sancov_guards"
            | "__stop___sancov_guards"
            | "__start___sancov_cntrs"
            | "__stop___sancov_cntrs"
            | "__start___sancov_pcs"
            | "__stop___sancov_pcs"
    ) {
        return true;
    }
    match Coverage::of(name) {
        Some(Coverage::TraceDiv) => {
            matches!(
                name,
                "__sanitizer_cov_trace_div4" | "__sanitizer_cov_trace_div8"
            )
        }
        Some(
            Coverage::TracePcGuard
            | Coverage::SancovCounters
            | Coverage::TraceCmp
            | Coverage::LegacyTraceCmp
            | Coverage::TraceGep
            | Coverage::IndirectCalls
            | Coverage::LegacyIndirectCalls
            | Coverage::StackDepth
            | Coverage::TracePc
            | Coverage::FunctionTrace
            | Coverage::AflInlineMap,
        ) => true,
        None => false,
    }
}

fn prepare_target(
    req: &Request,
    raw_object: &Path,
    globals: &[TargetGlobal],
    archive_roots: &[&str],
    main: Option<&object_prepare::MainRequirement>,
) -> Result<PreparationPlan, BuildError> {
    use object_prepare::{GlobalLinkage, GlobalRequirement, InputKind};

    let kind = match req.target {
        Target::CSource(_) => InputKind::CompiledSource,
        Target::Object(_) => InputKind::Object,
        Target::Archive(_) => InputKind::Archive,
    };
    let globals = globals
        .iter()
        .map(|global| GlobalRequirement {
            source_name: global.c_name.clone(),
            link_name: global.link_name(),
            size: global.size,
            section_name_base: format!("{GLOBAL_SECTION_PREFIX}.{}", global.address.num),
            linkage: match &global.linkage {
                austen_lib::spec::GlobalLinkage::External => GlobalLinkage::External,
                austen_lib::spec::GlobalLinkage::Internal { owner } => GlobalLinkage::Internal {
                    owner: owner.clone(),
                    effective_owner: if matches!(kind, InputKind::CompiledSource) && owner == "main"
                    {
                        main.expect("source compilation has a private-main plan")
                            .replacement
                            .clone()
                    } else {
                        owner.clone()
                    },
                },
            },
        })
        .collect::<Vec<_>>();
    let redirects = ALLOCATION_REDIRECTS.map(|redirect| object_prepare::AllocatorRedirect {
        original: redirect.original,
        replacement: redirect.replacement,
        cleanup_replacement: redirect.cleanup_replacement,
    });
    object_prepare::plan(
        &object_prepare::PreparationRequest {
            kind,
            target_path: req.target_path,
            raw_object,
            directory: req.dir,
            archive_roots,
            globals: &globals,
            main,
            check_heap_ownership: req.check_heap_ownership,
            placement: object_prepare::GlobalPlacement {
                base: GLOBAL_BASE,
                alignment: GLOBAL_ALIGNMENT,
            },
        },
        &object_prepare::PreparationTools {
            objcopy: objcopy_tool(),
            ar: tool("AR", "ar"),
        },
        &object_prepare::AllocationPolicy {
            redirects: &redirects,
            other_allocator_entries: UNINTERPOSED_ALLOCATOR_ENTRIES,
            preserves_lifetimes: &allocation_lifetime_preserving_import,
        },
    )
    .map_err(preparation_error)
}

fn preparation_error(error: object_prepare::PreparationError) -> BuildError {
    use object_prepare::PreparationError;
    match error {
        PreparationError::Read { path, source } => BuildError::Read { path, source },
        PreparationError::Write { path, source } => BuildError::Write { path, source },
        PreparationError::PreparedTarget { path, message } => {
            BuildError::PreparedTarget { path, message }
        }
        PreparationError::PreparedGlobals { path, message } => {
            BuildError::PreparedGlobals { path, message }
        }
        PreparationError::Spawn { program, source } => BuildError::Spawn {
            program,
            step: Step::PrepareTarget,
            source,
        },
        PreparationError::Failed { program, status } => BuildError::Failed {
            program,
            step: Step::PrepareTarget,
            status,
        },
    }
}

fn validate_linked_globals(harness: &Path, prepare: &PreparedLayout) -> Result<(), BuildError> {
    if prepare.globals.is_empty() {
        return Ok(());
    }
    let fail = |message: String| BuildError::PreparedGlobals {
        path: harness.to_path_buf(),
        message,
    };
    let wanted = prepare
        .globals
        .iter()
        .map(|global| global.link_name.as_str())
        .collect::<std::collections::HashSet<_>>();
    let image = object::linked_definitions(harness, |name| wanted.contains(name))
        .map_err(|error| fail(format!("could not inspect the linked harness: {error}")))?;
    validate_linked_global_image(&image, prepare).map_err(fail)
}

fn validate_linked_global_image(
    image: &object::LinkedDefinitions,
    prepare: &PreparedLayout,
) -> Result<(), String> {
    if image.format != object::RelocatableFormat::Elf || !image.executable {
        return Err(
            "the linked harness is not a fixed-address ELF executable (PIE would rebase published global addresses)"
                .to_string(),
        );
    }
    let host_architecture = object_prepare::placement_host_architecture()?;
    if image.architecture != host_architecture {
        return Err(format!(
            "linked harness architecture {:?} does not match Austen's host architecture {:?}",
            image.architecture, host_architecture
        ));
    }

    let checked_end = |address: u64, size: u64, what: &str| {
        address
            .checked_add(size)
            .ok_or_else(|| format!("{what} extent overflows the address space"))
    };
    let extent = |base: u64, size: u64| Region { base, size };
    let in_writable_load_segment = |base: u64, size: u64| {
        image.load_segments.iter().any(|segment| {
            segment.writable && extent(segment.address, segment.size).covers(&extent(base, size))
        })
    };

    for planned in &prepare.sections {
        let found = image
            .sections
            .iter()
            .filter(|section| section.name == planned.output)
            .collect::<Vec<_>>();
        let [section] = found.as_slice() else {
            return Err(format!(
                "linked section {:?} occurs {} times; expected one retained section",
                planned.output,
                found.len()
            ));
        };
        if section.address != planned.start
            || section.size != planned.size
            || !section.allocated
            || !section.writable
        {
            return Err(format!(
                "linked section {:?} does not have its planned writable loaded extent (expected address {:#x}, size {}; found address {:#x}, size {}, allocated {}, writable {})",
                planned.output,
                planned.start,
                planned.size,
                section.address,
                section.size,
                section.allocated,
                section.writable
            ));
        }
        if !in_writable_load_segment(section.address, section.size) {
            return Err(format!(
                "linked section {:?} is not wholly contained in a writable ELF load segment",
                planned.output
            ));
        }
    }

    let heap_start = austen_lib::wire::DEFAULT_HEAP_BASE;
    let heap_size = austen_lib::wire::DEFAULT_HEAP_BYTES;
    let heap_end = checked_end(heap_start, heap_size, "synthetic heap")?;
    let mut extents = Vec::with_capacity(prepare.globals.len());
    for global in &prepare.globals {
        let global_end = checked_end(
            global.address,
            global.size,
            &format!("linked symbol `{}`", global.link_name),
        )?;
        if extent(heap_start, heap_size).overlaps(&extent(global.address, global.size)) {
            return Err(format!(
                "linked symbol `{}` at {:#x}..{:#x} overlaps the synthetic heap reservation {:#x}..{:#x}",
                global.link_name, global.address, global_end, heap_start, heap_end
            ));
        }

        let definitions = image
            .definitions
            .iter()
            .filter(|definition| definition.name == global.link_name)
            .collect::<Vec<_>>();
        if definitions.is_empty()
            || definitions.iter().any(|definition| {
                definition.kind != object::DefinitionKind::Data
                    || definition.weak
                    || definition.address != global.address
                    || definition.size != global.size
                    || definition.section.as_ref().is_none_or(|section| {
                        section.name != global.section
                            || !section.allocated
                            || !section.writable
                            || !extent(section.address, section.size)
                                .covers(&extent(definition.address, definition.size))
                    })
            })
        {
            return Err(format!(
                "linked symbol `{}` is not one strong Data definition with its planned address {:#x}, {}-byte extent, and writable containing section {:?} (found {:?})",
                global.link_name,
                global.address,
                global.size,
                global.section,
                definitions
                    .iter()
                    .map(|definition| (
                        definition.kind,
                        definition.weak,
                        definition.address,
                        definition.size,
                        definition.section.as_ref().map(|section| &section.name),
                    ))
                    .collect::<Vec<_>>()
            ));
        }
        if !in_writable_load_segment(global.address, global.size) {
            return Err(format!(
                "linked symbol `{}` is not wholly contained in a writable ELF load segment",
                global.link_name
            ));
        }
        extents.push((global.address, global_end, global.link_name.as_str()));
    }

    extents.sort_unstable_by_key(|(start, end, name)| (*start, *end, *name));
    for pair in extents.windows(2) {
        let (left_start, left_end, left_name) = pair[0];
        let (right_start, right_end, right_name) = pair[1];
        if extent(left_start, left_end - left_start)
            .overlaps(&extent(right_start, right_end - right_start))
        {
            return Err(format!(
                "linked global extents overlap: `{left_name}` at {left_start:#x}..{left_end:#x} and `{right_name}` at {right_start:#x}..{right_end:#x}"
            ));
        }
    }
    Ok(())
}

pub(crate) fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-'
                )
        })
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

pub(crate) fn shell_line(program: &str, args: &[String]) -> String {
    let mut out = shell_quote(program);
    for a in args {
        out.push(' ');
        out.push_str(&shell_quote(a));
    }
    out
}

pub(crate) fn shell_command(program: &str, args: &[String], env: &[(String, String)]) -> String {
    let mut out = String::new();
    for (key, value) in env {
        out.push_str(key);
        out.push('=');
        out.push_str(&shell_quote(value));
        out.push(' ');
    }
    out.push_str(&shell_line(program, args));
    out
}

fn shell_command_in(
    cwd: Option<&Path>,
    program: &str,
    args: &[String],
    env: &[(String, String)],
) -> String {
    let command = shell_command(program, args, env);
    match cwd {
        Some(cwd) => format!(
            "(CDPATH= cd -- {} && {command})",
            shell_quote(&cwd.display().to_string())
        ),
        None => command,
    }
}

fn run(
    program: &str,
    args: &[String],
    env: &[(String, String)],
    cwd: Option<&Path>,
    step: Step,
) -> Result<(), BuildError> {
    tracing::debug!(
        target: "austen::build",
        step = ?step,
        command = %shell_command_in(cwd, program, args, env),
        "running",
    );
    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    for (key, value) in env {
        command.env(key, value);
    }
    let status = command.status().map_err(|source| BuildError::Spawn {
        program: program.to_string(),
        step,
        source,
    })?;
    if !status.success() {
        return Err(BuildError::Failed {
            program: program.to_string(),
            status,
            step,
        });
    }
    Ok(())
}

struct Generated {
    source: String,
    preparation_roots: Vec<String>,
    link_roots: Vec<String>,
    globals: Vec<TargetGlobal>,
    c_wrappers: String,
    main: Option<object_prepare::MainRequirement>,
    static_functions: Vec<StaticFunctionSymbols>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StaticFunctionSymbols {
    source: String,
    callable: String,
}

fn warn_inaccessible_static_archive_functions(req: &Request, functions: &[StaticFunctionSymbols]) {
    if functions.is_empty() || !matches!(req.target, Target::Archive(_)) {
        return;
    }

    let definitions = match object::archive_text_definitions(req.target_path, |name| {
        functions
            .iter()
            .any(|function| function.source == name || function.callable == name)
    }) {
        Ok(definitions) => definitions,
        Err(error) => {
            tracing::debug!(
                target: "austen::build",
                path = %req.target_path.display(),
                %error,
                "could not inspect prebuilt archive for inaccessible static functions"
            );
            return;
        }
    };

    for function in functions {
        let local_source = definitions
            .iter()
            .any(|definition| definition.name == function.source && !definition.global);
        let accessible = definitions
            .iter()
            .any(|definition| definition.name == function.callable && definition.global);
        if local_source && !accessible {
            req.reporter.emit(|| {
                observe::RunEvent::Warning(observe::Warning::StaticFunctionInPrebuiltArchive {
                    function: function.source.clone(),
                })
            });
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetGlobal {
    spec: austen_lib::spec::GlobalSpec,
    size: u64,
}

impl std::ops::Deref for TargetGlobal {
    type Target = austen_lib::spec::GlobalSpec;

    fn deref(&self) -> &Self::Target {
        &self.spec
    }
}

fn generate(req: &Request) -> Result<Generated, BuildError> {
    let json = std::fs::read_to_string(req.spec).map_err(|source| BuildError::Read {
        path: req.spec.to_path_buf(),
        source,
    })?;
    let module =
        austen_lib::spec::SpecModule::from_json(&json).map_err(|e| BuildError::ParseSpec {
            path: req.spec.to_path_buf(),
            message: e.to_string(),
        })?;
    let callable_main = module
        .functions
        .iter()
        .any(|function| function.name == "main");
    let main = match req.target {
        Target::CSource(_) => Some(object_prepare::MainRequirement {
            replacement: source_main_symbol(req.target_path)?,
            callable: callable_main,
        }),
        Target::Object(_) | Target::Archive(_) if callable_main => {
            Some(object_prepare::MainRequirement {
                replacement: object_prepare::replacement_main_symbol(req.target_path)
                    .map_err(preparation_error)?,
                callable: true,
            })
        }
        Target::Object(_) | Target::Archive(_) => None,
    };
    let mut link_names = function_link_names(&module);
    let static_functions = module
        .functions
        .iter()
        .zip(&link_names)
        .filter(|(function, _)| {
            matches!(
                &function.linkage,
                austen_lib::spec::FunctionLinkage::Internal
            )
        })
        .map(|(function, callable)| StaticFunctionSymbols {
            source: function.name.clone(),
            callable: callable.clone(),
        })
        .collect::<Vec<_>>();
    for (function, link_name) in module.functions.iter().zip(&mut link_names) {
        if function.name == "main" {
            *link_name = main
                .as_ref()
                .expect("a callable main always has a target symbol plan")
                .replacement
                .clone();
        }
    }
    let shim = generate_with_link_names(&module, &link_names).map_err(|e| BuildError::Shim {
        path: req.spec.to_path_buf(),
        message: format!("{e:?}"),
    })?;
    let globals = module
        .globals
        .iter()
        .zip(&shim.global_sizes)
        .map(|(global, size)| TargetGlobal {
            spec: global.clone(),
            size: *size,
        })
        .collect::<Vec<_>>();
    let mut preparation_roots = shim.preparation_roots;
    preparation_roots.extend(shim.owner_roots);
    let mut link_roots = shim.externs;
    if let Some(link) = &req.link {
        let mut add_root = |root: String| {
            preparation_roots.push(root.clone());
            link_roots.push(root);
        };
        if let Some(driver) = link.driver_symbol {
            add_root(driver.to_string());
        }
        if let Some(hooks) = &link.persistent_hooks {
            for hook in hooks.symbols() {
                add_root(hook.to_string());
            }
        }
    }
    let c_wrappers = shim.c_wrappers;
    let shim = shim.source;

    let template =
        std::fs::read_to_string(&req.runtime.template).map_err(|source| BuildError::Read {
            path: req.runtime.template.clone(),
            source,
        })?;
    for placeholder in [
        SHIM_PLACEHOLDER,
        DRIVER_PLACEHOLDER,
        HOOKS_PLACEHOLDER,
        BINARY_MARKER_PLACEHOLDER,
    ] {
        if template.matches(placeholder).count() != 1 {
            return Err(BuildError::Template {
                path: req.runtime.template.clone(),
            });
        }
    }
    let driver = driver_decl(req.link.as_ref().and_then(|l| l.driver_symbol));
    let hooks = hooks_decl(req.link.as_ref().and_then(|l| l.persistent_hooks.as_ref()));
    let marker = binary_marker_decl(req.link.as_ref().and_then(|l| l.binary_marker));
    let indented = shim
        .lines()
        .map(|l| {
            if l.is_empty() {
                String::new()
            } else {
                format!("    {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Generated {
        source: template
            .replace(SHIM_PLACEHOLDER, &indented)
            .replace(DRIVER_PLACEHOLDER, &driver)
            .replace(HOOKS_PLACEHOLDER, &hooks)
            .replace(BINARY_MARKER_PLACEHOLDER, &marker),
        preparation_roots,
        link_roots,
        globals,
        c_wrappers,
        main,
        static_functions,
    })
}

fn binary_marker_decl(marker: Option<austen_lib::external::BinaryMarker>) -> String {
    let Some(marker) = marker else {
        return "fn retain_binary_marker() {}".to_string();
    };
    let bytes = marker
        .bytes
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "#[used]\nstatic AUSTEN_BINARY_MARKER: [u8; {}] = [{bytes}];\n\n\
         fn retain_binary_marker() {{\n    \
         let _ = unsafe {{ core::ptr::read_volatile(&raw const AUSTEN_BINARY_MARKER) }};\n}}",
        marker.bytes.len()
    )
}

fn driver_decl(symbol: Option<&str>) -> String {
    let Some(symbol) = symbol else {
        return "const DRIVER: Option<InProcessDriver> = None;".to_string();
    };
    format!(
        "unsafe extern \"C\" {{\n    \
             fn {symbol}(\n        \
                 argc: *mut core::ffi::c_int,\n        \
                 argv: *mut *mut *mut core::ffi::c_char,\n        \
                 cb: extern \"C\" fn(*const u8, usize) -> core::ffi::c_int,\n    \
             ) -> core::ffi::c_int;\n\
         }}\n\n\
         const DRIVER: Option<InProcessDriver> = Some({symbol});"
    )
}

fn hooks_decl(hooks: Option<&austen_lib::external::PersistentHookSymbols>) -> String {
    let Some(austen_lib::external::PersistentHookSymbols {
        clear_new_coverage,
        reset_local_coverage,
        reset_stack_depth,
        count_counters,
        check_stack_depth,
    }) = hooks
    else {
        return "const HOOKS: *const PersistentHooks = core::ptr::null();".to_string();
    };
    let slots = [
        ("clear_new_coverage", "ClearNewCoverage", clear_new_coverage),
        (
            "reset_local_coverage",
            "ResetLocalCoverage",
            reset_local_coverage,
        ),
        ("reset_stack_depth", "ResetStackDepth", reset_stack_depth),
        ("count_counters", "CountCounters", count_counters),
        ("check_stack_depth", "CheckStackDepth", check_stack_depth),
    ];
    let decls = slots
        .iter()
        .map(|(_, _, symbol)| format!("    fn {symbol}();\n"))
        .collect::<String>();
    let fields = slots
        .iter()
        .map(|(slot, newtype, symbol)| format!("    {slot}: {newtype}({symbol}),\n"))
        .collect::<String>();
    format!(
        "unsafe extern \"C\" {{\n{decls}}}\n\n\
         static PERSISTENT_HOOKS: PersistentHooks = PersistentHooks {{\n{fields}}};\n\n\
         const HOOKS: *const PersistentHooks = &raw const PERSISTENT_HOOKS;"
    )
}

fn check(
    req: &Request,
    object: &Path,
    archive_roots: &[&str],
) -> Result<(bool, austen_lib::instrumentation::Instrumentation), BuildError> {
    use austen_lib::instrumentation::{self as coverage, Found, Instrumentation};

    let Some(expect) = &req.expect else {
        return Ok((false, Instrumentation::Absent));
    };
    let intended = req.compile.is_some();

    let link_line = link_line(req);
    let (resolved, unreadable) = coverage::resolved_by(link_line.iter().copied());
    for e in unreadable {
        req.reporter.emit(|| {
            observe::RunEvent::Warning(observe::Warning::LinkLineUnreadable {
                entry: e.path().to_path_buf(),
                message: e.to_string(),
            })
        });
    }
    tracing::debug!(
        target: "austen::build",
        asker = expect.asker,
        resolves = ?resolved.iter().map(|c| c.token()).collect::<Vec<_>>(),
        entries = link_line.len(),
        "what the harness link line defines"
    );

    let probed = if matches!(req.target, Target::Archive(_)) {
        coverage::probe_archive(
            expect.consumes,
            expect.rejects,
            &resolved,
            object,
            archive_roots,
        )
    } else {
        coverage::probe(expect.consumes, expect.rejects, &resolved, object)
    };
    match probed {
        Ok(Found::Consumable(found)) => {
            tracing::debug!(
                target: "austen::build",
                asker = expect.asker,
                coverage = ?found.iter().map(|e| e.coverage.token()).collect::<Vec<_>>(),
                "the target carries this engine's instrumentation"
            );
            Ok((
                true,
                if intended {
                    Instrumentation::Compiled
                } else {
                    Instrumentation::Detected
                },
            ))
        }
        Ok(found @ (Found::Clean | Found::Inert(_))) => {
            tracing::debug!(target: "austen::build", asker = expect.asker, ?found, "no usable gradient");
            let inert = match &found {
                Found::Inert(evidence) => evidence.first().map(|e| observe::InertCoverage {
                    carries: e.coverage.token().to_string(),
                    usable_by: austen_lib::instrumentation::consumers(
                        e.coverage,
                        crate::registry::engines(),
                    )
                    .iter()
                    .map(|c| c.to_string())
                    .collect(),
                }),
                _ => None,
            };
            req.reporter.emit(|| {
                observe::RunEvent::Warning(observe::Warning::UninstrumentedTarget {
                    target: object.to_path_buf(),
                    engine: expect.asker.to_string(),
                    inert,
                })
            });
            Ok((false, Instrumentation::Absent))
        }
        Ok(Found::Unlinkable(found)) => Err(BuildError::ForeignInstrumentation {
            message: foreign(object, expect, &found),
        }),
        Ok(Found::Rejected(found)) => Err(BuildError::ForeignInstrumentation {
            message: rejected(object, expect, &found),
        }),
        Ok(Found::Defines(found)) => Err(BuildError::ForeignInstrumentation {
            message: defines(object, expect, &found),
        }),
        Err(e) => {
            req.reporter.emit(|| {
                observe::RunEvent::Warning(observe::Warning::InstrumentationUnknown {
                    target: object.to_path_buf(),
                    message: e.to_string(),
                })
            });
            Ok((intended, Instrumentation::Unknown))
        }
    }
}

fn guard_count(object: &Path) -> Option<u64> {
    match object::sancov_guard_count(object) {
        Ok(guards) => {
            tracing::debug!(
                target: "austen::build",
                %guards,
                "guards the target declares"
            );
            Some(guards)
        }
        Err(e) => {
            tracing::debug!(
                target: "austen::build",
                entry = %e.path().display(),
                error = %e,
                "no guard count: the whole coverage map will be scanned"
            );
            None
        }
    }
}

fn link_line<'a>(req: &'a Request<'a>) -> Vec<&'a Path> {
    let runtime = req.runtime.library.as_path();
    std::iter::once(runtime)
        .chain(
            req.link
                .iter()
                .flat_map(|l| &l.link_args)
                .filter_map(austen_lib::external::LinkArg::path),
        )
        .collect()
}

fn foreign(
    object: &Path,
    expect: &Expect,
    found: &[austen_lib::instrumentation::Evidence],
) -> String {
    let first = &found[0];
    let steered =
        austen_lib::instrumentation::steered_by(first.coverage, crate::registry::engines());
    let asker = crate::engine_label(expect.asker);
    format!(
        "{} references `{}`, the instrumentation {} emits, and {steered}; \
         this build was asked for by {}.\n{}",
        object.display(),
        first.symbol,
        first.coverage.built_with(),
        asker,
        expect.remedy,
    )
}

fn rejected(
    object: &Path,
    expect: &Expect,
    found: &[austen_lib::instrumentation::Evidence],
) -> String {
    let first = &found[0];
    let asker = crate::engine_label(expect.asker);
    format!(
        "{} references `{}`, the instrumentation {} emits, but {} rejects that \
         instrumentation at run time.\n{}",
        object.display(),
        first.symbol,
        first.coverage.built_with(),
        asker,
        expect.remedy,
    )
}

fn defines(
    object: &Path,
    expect: &Expect,
    found: &[austen_lib::instrumentation::Evidence],
) -> String {
    let first = &found[0];
    let asker = crate::engine_label(expect.asker);
    format!(
        "{} defines `{}` instead of referencing it — a coverage runtime appears \
         to be linked into the target, and its definition would collide with or \
         silently preempt the one the harness link supplies; this build was \
         asked for by {}.\n{}",
        object.display(),
        first.symbol,
        asker,
        expect.remedy,
    )
}

pub fn build(req: &Request) -> Result<Built, BuildError> {
    let source_path = req.dir.join("harness_main.rs");
    let harness = req.dir.join("harness");
    let write = |path: &Path, contents: &str| -> Result<(), BuildError> {
        std::fs::write(path, contents).map_err(|source| BuildError::Write {
            path: path.to_path_buf(),
            source,
        })
    };

    let Generated {
        source,
        preparation_roots,
        link_roots,
        globals,
        c_wrappers,
        main,
        static_functions,
    } = {
        let _phase = req.reporter.phase(observe::Phase::Shim);
        generate(req)?
    };
    if req.scripts {
        let spec_copy = req.dir.join("spec.json");
        if req.ignore_custom_generators {
            let text = std::fs::read_to_string(req.spec).map_err(|source| BuildError::Read {
                path: req.spec.to_path_buf(),
                source,
            })?;
            let spec = austen_lib::driver::spec_without_generators(&text).map_err(|source| {
                BuildError::ParseSpec {
                    path: req.spec.to_path_buf(),
                    message: source.to_string(),
                }
            })?;
            std::fs::write(&spec_copy, spec).map_err(|source| BuildError::Write {
                path: spec_copy.clone(),
                source,
            })?;
        } else if let Some(sources) = req.custom_generators {
            sources
                .snapshot_spec(&spec_copy)
                .map_err(|source| BuildError::SpecSnapshot { source })?;
        } else {
            std::fs::copy(req.spec, &spec_copy).map_err(|source| BuildError::Write {
                path: spec_copy.clone(),
                source,
            })?;
        }
    }

    let (cc, cflags, cc_env) = compiler(req.compile.as_ref(), req.coverage_cflags, req.cflags);
    let preparation_roots = preparation_roots
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let link_roots = link_roots.iter().map(String::as_str).collect::<Vec<_>>();
    let raw_object = match req.target {
        Target::CSource(_) => req.dir.join("target.o"),
        Target::Object(_) | Target::Archive(_) => req.target_path.to_path_buf(),
    };
    let wrapper_path = (matches!(req.target, Target::CSource(_)) && !c_wrappers.is_empty())
        .then(|| req.dir.join("target_wrappers.c"));
    if let Some(path) = &wrapper_path {
        write(path, &c_wrappers)?;
    }
    let cc_args = match req.target {
        Target::CSource(_) => Some(source_compile_args(
            cflags,
            &main
                .as_ref()
                .expect("every C source has a private-main symbol plan")
                .replacement,
            req.target_path,
            wrapper_path.as_deref(),
            &raw_object,
        )),
        Target::Object(_) | Target::Archive(_) => None,
    };
    let mut compiled = false;
    if let Some(args) = &cc_args {
        record_command(req, "target-compiler", &cc, args, &cc_env, req.compile_cwd);
        let target_recipe = source_object_recipe(
            req,
            &cc,
            args,
            &cc_env,
            wrapper_path.as_ref().map(|_| c_wrappers.as_str()),
        );
        if !reusable_source_object(req, &target_recipe, &raw_object) {
            let _ = std::fs::remove_file(req.dir.join(RECIPE_NAME));
            let _ = std::fs::remove_file(req.dir.join(TARGET_RECIPE_NAME));
            write(&source_path, &source)?;
            {
                let _phase = req.reporter.phase(observe::Phase::CompileTarget);
                run(&cc, args, &cc_env, req.compile_cwd, Step::CompileTarget)?;
            }
            write(&req.dir.join(TARGET_RECIPE_NAME), &target_recipe)?;
            compiled = true;
        }
    }
    warn_inaccessible_static_archive_functions(req, &static_functions);
    let prepare = prepare_target(
        req,
        &raw_object,
        &globals,
        &preparation_roots,
        main.as_ref(),
    )?;
    let object = prepare.link_input().to_path_buf();

    let mut rustc_args = vec![
        "--edition".to_string(),
        "2024".to_string(),
        "-O".to_string(),
        "--crate-name".to_string(),
        "austen_harness_run".to_string(),
        "-o".to_string(),
        harness.display().to_string(),
        source_path.display().to_string(),
        "-L".to_string(),
        format!("native={}", req.runtime.dir.display()),
        "-l".to_string(),
        format!("dylib={LINK_NAME}"),
        "-C".to_string(),
        format!("link-arg={}", object.display()),
        "-C".to_string(),
        format!("link-arg=-Wl,-rpath,{}", req.runtime.dir.display()),
    ];
    if req.check_heap_ownership {
        rustc_args.extend([
            "--cfg".to_string(),
            "austen_check_heap_ownership".to_string(),
        ]);
    }
    if let Some(link) = &req.link {
        if let Some(linker) = &link.linker {
            rustc_args.extend(["-C".to_string(), format!("linker={}", linker.display())]);
        }
        for arg in &link.link_args {
            rustc_args.extend(["-C".to_string(), format!("link-arg={arg}")]);
        }
    }
    for arg in prepare.linker_args() {
        rustc_args.extend(["-C".to_string(), format!("link-arg={arg}")]);
    }
    rustc_args.extend(["-C".to_string(), "link-arg=-lc".to_string()]);

    let cmds = Commands {
        cc,
        cc_env,
        cc_args,
        cc_cwd: req.compile_cwd.map(Path::to_path_buf),
        prepare,
        rustc: tool("RUSTC", "rustc"),
        rustc_args,
    };
    record_command(
        req,
        "harness-compiler",
        &cmds.rustc,
        &cmds.rustc_args,
        &[],
        None,
    );
    if let Some(linker) = req.link.as_ref().and_then(|link| link.linker.as_ref()) {
        req.reporter
            .emit(|| observe::RunEvent::tool_selected("linker", linker.clone()));
    }
    for command in cmds.prepare.commands() {
        record_command(
            req,
            "target-preparation",
            command.program(),
            command.args(),
            &[],
            None,
        );
    }

    let recipe = recipe(req, &cmds);
    let preparation_present = cmds.prepare.required_outputs().all(Path::is_file);
    let up_to_date = !compiled
        && preparation_present
        && reusable(req, &recipe, &source_path, &source, &harness, &object);

    if up_to_date {
        tracing::debug!(
            target: "austen::build",
            harness = %harness.display(),
            "the harness in this directory is already this build's",
        );
        req.reporter.emit(|| observe::RunEvent::HarnessReused {
            harness: harness.clone(),
        });
        cmds.prepare.reuse().map_err(preparation_error)?;
    } else {
        let _ = std::fs::remove_file(req.dir.join(RECIPE_NAME));
        write(&source_path, &source)?;
        if !cmds.prepare.commands().is_empty() {
            let _phase = req.reporter.phase(observe::Phase::CompileTarget);
            cmds.prepare
                .execute(|command| {
                    tracing::debug!(
                        target: "austen::build",
                        step = ?Step::PrepareTarget,
                        command = %shell_line(command.program(), command.args()),
                        "running",
                    );
                })
                .map_err(preparation_error)?;
        }
    }

    let (instrumented, instrumentation) = check(req, &object, &link_roots)?;
    let coverage_guards = guard_count(&object);

    if !up_to_date {
        let _phase = req.reporter.phase(observe::Phase::CompileHarness);
        run(&cmds.rustc, &cmds.rustc_args, &[], None, Step::LinkHarness)?;
    }
    validate_linked_globals(&harness, cmds.prepare.layout())?;
    if !up_to_date {
        write(&req.dir.join(RECIPE_NAME), &recipe)?;
    }

    if req.scripts {
        write_scripts(req, &cmds)?;
    }
    Ok(Built {
        harness,
        object: Some(object),
        instrumented,
        instrumentation,
        coverage_guards,
    })
}

fn record_command(
    req: &Request,
    role: &str,
    program: &str,
    arguments: &[String],
    environment: &[(String, String)],
    working_directory: Option<&Path>,
) {
    req.reporter.emit(|| observe::RunEvent::BuildCommand {
        directory: req.dir.to_path_buf(),
        role: role.to_string(),
        program: program.into(),
        arguments: arguments.to_vec(),
        environment: environment.iter().cloned().collect(),
        working_directory: working_directory.map(Path::to_path_buf),
    });
}

const RECIPE_NAME: &str = ".build-recipe";

const TARGET_RECIPE_NAME: &str = ".target-recipe";

fn source_object_recipe(
    req: &Request,
    cc: &str,
    args: &[String],
    env: &[(String, String)],
    wrappers: Option<&str>,
) -> String {
    let mut recipe = format!(
        "austen-target-recipe 1\ncompile {}\ninput {} {}\n",
        shell_command_in(req.compile_cwd, cc, args, env),
        req.target_path.display(),
        fingerprint(req.target_path)
    );
    if let Some(wrappers) = wrappers {
        let mut hash = crate::stamp::Fnv1a::new();
        hash.eat(wrappers.as_bytes());
        recipe.push_str(&format!("wrappers {}\n", hash.hex()));
    }
    recipe
}

fn reusable_source_object(req: &Request, recipe: &str, object: &Path) -> bool {
    req.cflags.is_empty()
        && object.is_file()
        && std::fs::read_to_string(req.dir.join(TARGET_RECIPE_NAME))
            .is_ok_and(|have| have == recipe)
}

struct Commands {
    cc: String,
    cc_env: Vec<(String, String)>,
    cc_args: Option<Vec<String>>,
    cc_cwd: Option<PathBuf>,
    prepare: PreparationPlan,
    rustc: String,
    rustc_args: Vec<String>,
}

fn recipe(req: &Request, cmds: &Commands) -> String {
    let mut out = String::from("austen-build-recipe 1\n");
    match &cmds.cc_args {
        Some(args) => {
            out.push_str("compile ");
            out.push_str(&shell_command_in(
                cmds.cc_cwd.as_deref(),
                &cmds.cc,
                args,
                &cmds.cc_env,
            ));
        }
        None => out.push_str("compile none"),
    }
    out.push('\n');
    if cmds.prepare.commands().is_empty() {
        out.push_str("prepare none\n");
    }
    for command in cmds.prepare.commands() {
        out.push_str("prepare ");
        out.push_str(&shell_line(command.program(), command.args()));
        out.push('\n');
    }
    out.push_str("link ");
    out.push_str(&shell_line(&cmds.rustc, &cmds.rustc_args));
    out.push('\n');

    let prepare_inputs = cmds
        .prepare
        .additional_inputs()
        .iter()
        .map(PathBuf::as_path);
    for path in std::iter::once(req.target_path)
        .chain(link_line(req))
        .chain(prepare_inputs)
    {
        out.push_str(&format!("input {} {}\n", path.display(), fingerprint(path)));
    }
    out
}

fn fingerprint(path: &Path) -> String {
    let Ok(meta) = std::fs::metadata(path) else {
        return "unreadable".to_string();
    };
    let stamp = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_else(|| "no-mtime".to_string());
    format!("{} {stamp}", meta.len())
}

fn reusable(
    req: &Request,
    recipe: &str,
    source_path: &Path,
    source: &str,
    harness: &Path,
    object: &Path,
) -> bool {
    if !harness.is_file() || !object.is_file() {
        return false;
    }
    let matches =
        |path: &Path, want: &str| std::fs::read_to_string(path).is_ok_and(|have| have == want);
    matches(&req.dir.join(RECIPE_NAME), recipe) && matches(source_path, source)
}

fn write_scripts(req: &Request, cmds: &Commands) -> Result<(), BuildError> {
    let header = "\
#!/bin/sh
# @generated by `austen test` — safe to edit, safe to delete.
set -eu
here=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)
";
    let mut build = String::from(header);
    build.push_str("\n# Compile the target to an object.\n");
    match &cmds.cc_args {
        Some(args) => {
            build.push_str(&shell_command_in(
                cmds.cc_cwd.as_deref(),
                &cmds.cc,
                args,
                &cmds.cc_env,
            ));
            build.push('\n');
        }
        None => build.push_str("# (the target was already compiled)\n"),
    }
    build.push_str(
        "\n# Prepare target entry points, allocation imports, and fixed-address global sections.\n",
    );
    if cmds.prepare.commands().is_empty() {
        build.push_str("# (the target does not need a symbol/section/import rewrite)\n");
    }
    for command in cmds.prepare.commands() {
        build.push_str(&shell_line(command.program(), command.args()));
        build.push('\n');
    }
    build.push_str("\n# Compile the generated shim and link it against the prebuilt runtime.\n");
    build.push_str(&shell_line(&cmds.rustc, &cmds.rustc_args));
    build.push_str("\necho \"harness: $here/harness\"\n");

    let mut defaults = String::new();
    let mut default = |arg: String, condition: &str| {
        let option = arg.split('=').next().expect("an argument has an option");
        defaults.push_str(&format!(
            "if {condition} && ! has_option {} \"$@\"; then\n    set -- {} \"$@\"\nfi\n",
            shell_quote(option),
            shell_quote(&arg),
        ));
    };
    if let Some(function) = req.function {
        default(format!("--function={function}"), "\"$saved_function\"");
    }
    if req.ignore_custom_generators && req.engine_descriptor.producer_options {
        default("--ignore-custom-generators".into(), "\"$saved_producer\"");
    }
    if let Some(selector) = req.function_selector.filter(|_| {
        req.engine_descriptor.function_selection == austen_lib::FunctionSelectionOwner::Austen
    }) {
        default(
            format!("--function-selector={}", selector.token()),
            "\"$saved_live_policy\"",
        );
    }
    for arg in [
        max_generator_size_arg(req.max_generator_size),
        sizing_strategy_arg(req.size_selector),
        generator_timeout_arg(req.generator_timeout),
        chain_discard_limit_arg(req.chain_discard_limit),
    ]
    .into_iter()
    .flatten()
    .filter(|_| req.engine_descriptor.producer_options)
    {
        default(arg, "\"$saved_live_policy\"");
    }
    if let Some(engine) = req.engine {
        default(format!("--engine={engine}"), "\"$saved_engine\"");
    }
    let production_only = req.pass_options.production_only_requests();
    for arg in pass_option_args(req.pass_options) {
        let option = arg.split('=').next().expect("an argument has an option");
        let production_only = production_only.iter().any(|(name, _)| *name == option);
        if (production_only && !req.engine_descriptor.producer_options)
            || (option == "--max-array-length" && !req.engine_descriptor.max_array_length)
        {
            continue;
        }
        let condition = if option == "--max-array-length" || production_only {
            "\"$saved_producer\""
        } else {
            "true"
        };
        default(arg, condition);
    }
    let no_coverage = if req.no_coverage {
        "--no-coverage "
    } else {
        ""
    };
    let allow_unstable_addresses = if req.allow_unstable_addresses {
        "--allow-unstable-addresses "
    } else {
        ""
    };
    let run_sh = format!(
        r#"{header}
"$here/build.sh"

# Extra arguments override the saved campaign defaults. Replay and exact input
# omit policies they do not use; choosing another engine drops its predecessor's
# production settings. Wire conversion settings stay paired with this harness.
# Values stay quoted, including paths containing spaces.
saved_engine=true
saved_function=true
saved_producer=true
saved_live_policy=true
for argument do
    case "$argument" in
        --input|--input=*)
            saved_engine=false
            saved_function=false
            saved_producer=false
            saved_live_policy=false ;;
        --replay|--replay=*) saved_live_policy=false ;;
        --engine|--engine=*)
            saved_engine=false
            saved_producer=false
            saved_live_policy=false ;;
        --) break ;;
    esac
done
has_option() {{
    option=$1
    shift
    for supplied do
        case "$supplied" in
            "$option"|"$option"=*) return 0 ;;
            --) break ;;
        esac
    done
    return 1
}}
{defaults}
exec austen test --harness="$here/harness" "$here/spec.json" \
    {no_coverage}{allow_unstable_addresses}--crashes="$here/crashes" --work="$here" "$@"
"#,
    );

    for (name, contents) in [("build.sh", &build), ("run.sh", &run_sh)] {
        let path = req.dir.join(name);
        std::fs::write(&path, contents).map_err(|source| BuildError::Write {
            path: path.clone(),
            source,
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
        }
    }
    Ok(())
}

pub(crate) fn max_generator_size_arg(
    size: Option<austen_lib::driver::TestCaseSize>,
) -> Option<String> {
    size.filter(|size| *size != austen_lib::driver::DEFAULT_MAX_GENERATOR_SIZE)
        .map(|size| format!("--max-generator-size={}", size.get()))
}

pub(crate) fn sizing_strategy_arg(kind: Option<austen_lib::SizeSelectorKind>) -> Option<String> {
    kind.filter(|kind| *kind != austen_lib::SizeSelectorKind::default())
        .map(|kind| format!("--sizing-strategy={}", kind.token()))
}

pub(crate) fn generator_timeout_arg(timeout: crate::args::GeneratorTimeout) -> Option<String> {
    use crate::args::GeneratorTimeout as T;
    match timeout {
        T::Inapplicable => None,
        T::After(d) if d == austen_lib::driver::DEFAULT_GENERATOR_TIMEOUT => None,
        T::After(d) => Some(format!("--generator-timeout={}", d.as_millis())),
        T::Disabled => Some("--generator-timeout=0".to_owned()),
    }
}

pub(crate) fn chain_discard_limit_arg(limit: Option<std::num::NonZeroU64>) -> Option<String> {
    limit.map(|limit| format!("--chain-discard-limit={limit}"))
}

pub(crate) fn pass_option_args(options: austen_lib::driver::PassOptions) -> Vec<String> {
    let mut args = Vec::new();
    if options.inline != austen_lib::driver::InlineMode::Nothing {
        args.push(format!("--inline={}", options.inline.as_str()));
    }
    if let Some(max) = options.max_unfolds {
        args.push(format!("--max-unfolds={max}"));
    }
    if options.path_pruning {
        args.push("--path-pruning".into());
    }
    if options.smt_pruning {
        args.push("--smt-pruning".into());
    }
    if options.experimental_product_arg_destruction {
        args.push("--experimental-product-arg-destruction".into());
    }
    if options.experimental_return_pruning {
        args.push("--experimental-return-pruning".into());
    }
    if options.experimental_arg_pruning {
        args.push("--experimental-arg-pruning".into());
    }
    if options.sink_args {
        args.push("--sink-args".into());
    }
    if options.specialize_guarded_choices {
        args.push("--specialize-guarded-choices".into());
    }
    if options.max_array_length != austen_lib::driver::PassOptions::NONE.max_array_length {
        args.push(format!("--max-array-length={}", options.max_array_length));
    }
    args
}
