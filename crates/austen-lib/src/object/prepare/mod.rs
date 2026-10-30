use std::path::{Path, PathBuf};
use std::process::Command;

pub const PRIVATE_MAIN_SYMBOL: &str = "__austen_target_main";
pub const DEFAULT_OBJCOPY: &str = if cfg!(target_os = "macos") {
    "llvm-objcopy"
} else {
    "objcopy"
};
const ASAN_OPTIONS_SYMBOL: &str = "__asan_default_options";
const PRIVATE_ASAN_OPTIONS_SYMBOL: &str = "__austen_target_asan_default_options";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    CompiledSource,
    Object,
    Archive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlobalLinkage {
    External,
    Internal {
        owner: String,
        effective_owner: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalRequirement {
    pub source_name: String,
    pub link_name: String,
    pub size: u64,
    pub section_name_base: String,
    pub linkage: GlobalLinkage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainRequirement {
    pub replacement: String,
    pub callable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalPlacement {
    pub base: u64,
    pub alignment: u64,
}

pub struct PreparationRequest<'a> {
    pub kind: InputKind,
    pub target_path: &'a Path,
    pub raw_object: &'a Path,
    pub directory: &'a Path,
    pub archive_roots: &'a [&'a str],
    pub globals: &'a [GlobalRequirement],
    pub main: Option<&'a MainRequirement>,
    pub check_heap_ownership: bool,
    pub placement: GlobalPlacement,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparationTools {
    pub objcopy: String,
    pub ar: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocatorRedirect {
    pub original: &'static str,
    pub replacement: &'static str,
    pub cleanup_replacement: Option<&'static str>,
}

impl AllocatorRedirect {
    fn with_cleanup(mut self, cleanup: bool) -> Self {
        if cleanup && let Some(owning) = self.cleanup_replacement {
            self.replacement = owning;
        }
        self
    }

    fn reserved_names(self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.replacement).chain(self.cleanup_replacement)
    }
}

pub struct AllocationPolicy<'a> {
    pub redirects: &'a [AllocatorRedirect],
    pub other_allocator_entries: &'a [&'a str],
    pub preserves_lifetimes: &'a dyn Fn(&str) -> bool,
}

#[derive(Debug, thiserror::Error)]
pub enum PreparationError {
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
    #[error("could not prepare the target entry point in {path}: {message}")]
    PreparedTarget { path: PathBuf, message: String },
    #[error("could not prepare specified globals in {path}: {message}")]
    PreparedGlobals { path: PathBuf, message: String },
    #[error("could not run `{program}`: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`{program}` failed with {status} while preparing the target")]
    Failed {
        program: String,
        status: std::process::ExitStatus,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreparedLayout {
    pub globals: Vec<PreparedGlobal>,
    pub sections: Vec<PreparedSection>,
}

pub struct PreparationPlan {
    link_input: PathBuf,
    commands: Vec<CommandSpec>,
    inputs: Vec<PathBuf>,
    materialized: Vec<MaterializedInput>,
    layout: PreparedLayout,
}

impl PreparationPlan {
    pub fn unchanged(link_input: PathBuf) -> Self {
        Self {
            link_input,
            commands: Vec::new(),
            inputs: Vec::new(),
            materialized: Vec::new(),
            layout: PreparedLayout::default(),
        }
    }

    fn single_object(
        program: String,
        args: Vec<String>,
        output: PathBuf,
        validation: PreparedObject,
    ) -> Self {
        let layout = PreparedLayout {
            globals: validation.globals.clone(),
            sections: validation.sections.clone(),
        };
        Self {
            link_input: output.clone(),
            commands: vec![CommandSpec {
                program,
                args,
                output,
                validation: Some(validation),
            }],
            inputs: Vec::new(),
            materialized: Vec::new(),
            layout,
        }
    }

    pub fn link_input(&self) -> &Path {
        &self.link_input
    }

    pub fn commands(&self) -> &[CommandSpec] {
        &self.commands
    }

    pub fn additional_inputs(&self) -> &[PathBuf] {
        &self.inputs
    }

    pub fn required_outputs(&self) -> impl Iterator<Item = &Path> {
        self.commands.iter().map(CommandSpec::output)
    }

    pub fn layout(&self) -> &PreparedLayout {
        &self.layout
    }

    pub fn linker_args(&self) -> Vec<String> {
        global_link_args(&self.layout)
    }

    pub fn execute(
        &self,
        mut before_command: impl FnMut(&CommandSpec),
    ) -> Result<(), PreparationError> {
        self.materialize()?;
        for command in self.commands() {
            create_parent(&command.output)?;
            if let Err(source) = std::fs::remove_file(&command.output)
                && source.kind() != std::io::ErrorKind::NotFound
            {
                return Err(PreparationError::Write {
                    path: command.output.clone(),
                    source,
                });
            }
            before_command(command);
            let status = Command::new(&command.program)
                .args(&command.args)
                .status()
                .map_err(|source| PreparationError::Spawn {
                    program: command.program.clone(),
                    source,
                })?;
            if !status.success() {
                return Err(PreparationError::Failed {
                    program: command.program.clone(),
                    status,
                });
            }
        }
        self.validate()
    }

    pub fn reuse(&self) -> Result<(), PreparationError> {
        self.materialize()?;
        self.validate()
    }

    fn materialize(&self) -> Result<(), PreparationError> {
        for input in &self.materialized {
            create_parent(&input.path)?;
            std::fs::write(&input.path, &input.bytes).map_err(|source| {
                PreparationError::Write {
                    path: input.path.clone(),
                    source,
                }
            })?;
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), PreparationError> {
        for command in self.commands() {
            if let Some(validation) = &command.validation {
                validate_prepared_target(&command.output, validation)?;
            }
            let metadata =
                std::fs::metadata(&command.output).map_err(|source| PreparationError::Read {
                    path: command.output.clone(),
                    source,
                })?;
            if !metadata.is_file() {
                let message = "the prepared output is not a regular file".to_string();
                let has_globals = command
                    .validation
                    .as_ref()
                    .map_or(!self.layout.globals.is_empty(), |object| {
                        !object.globals.is_empty()
                    });
                return Err(if has_globals {
                    PreparationError::PreparedGlobals {
                        path: command.output.clone(),
                        message,
                    }
                } else {
                    PreparationError::PreparedTarget {
                        path: command.output.clone(),
                        message,
                    }
                });
            }
        }
        Ok(())
    }
}

fn create_parent(path: &Path) -> Result<(), PreparationError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|source| PreparationError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

impl CommandSpec {
    pub fn program(&self) -> &str {
        &self.program
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }

    pub fn output(&self) -> &Path {
        &self.output
    }
}

struct MaterializedInput {
    path: PathBuf,
    bytes: Vec<u8>,
}

struct Context<'a> {
    request: &'a PreparationRequest<'a>,
    tools: &'a PreparationTools,
    allocation: &'a AllocationPolicy<'a>,
}

impl<'a> std::ops::Deref for Context<'a> {
    type Target = PreparationRequest<'a>;

    fn deref(&self) -> &Self::Target {
        self.request
    }
}

pub fn plan(
    request: &PreparationRequest<'_>,
    tools: &PreparationTools,
    allocation: &AllocationPolicy<'_>,
) -> Result<PreparationPlan, PreparationError> {
    if request.kind == InputKind::CompiledSource && request.main.is_none() {
        return Err(PreparationError::PreparedTarget {
            path: request.target_path.to_path_buf(),
            message: "a compiled source requires its private-main requirement".to_string(),
        });
    }
    if !request.globals.is_empty() && !request.placement.alignment.is_power_of_two() {
        return Err(PreparationError::PreparedGlobals {
            path: request.target_path.to_path_buf(),
            message: format!(
                "fixed global-section placement has unsupported non-power-of-two alignment {}",
                request.placement.alignment,
            ),
        });
    }
    let context = Context {
        request,
        tools,
        allocation,
    };
    prepare_target(&context)
}

fn global_link_args(layout: &PreparedLayout) -> Vec<String> {
    if layout.globals.is_empty() {
        return Vec::new();
    }
    let mut args = vec!["-no-pie".to_string(), "-Wl,--no-gc-sections".to_string()];
    for global in &layout.globals {
        args.push(format!(
            "-Wl,--undefined={}",
            super::decorated_c_symbol(&global.link_name)
        ));
    }
    for section in &layout.sections {
        args.push(format!(
            "-Wl,--section-start={}={:#x}",
            section.output, section.start
        ));
    }
    args
}

fn allocation_rename(redirect: &AllocatorRedirect) -> String {
    symbol_rename(redirect.original, redirect.replacement)
}

fn allocation_lifetime_changing_entry(name: &str, policy: &AllocationPolicy<'_>) -> bool {
    policy
        .redirects
        .iter()
        .any(|redirect| name == redirect.original)
        || policy.other_allocator_entries.contains(&name)
}

fn allocation_lifetime_preserving_import(name: &str, policy: &AllocationPolicy<'_>) -> bool {
    policy
        .redirects
        .iter()
        .any(|redirect| name == redirect.original)
        || (policy.preserves_lifetimes)(name)
}

pub fn first_unused_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    first_unused_spelling(base, '_', taken)
}

fn first_unused_spelling(base: &str, separator: char, taken: impl Fn(&str) -> bool) -> String {
    for suffix in 0_u32.. {
        let candidate = if suffix == 0 {
            base.to_string()
        } else {
            format!("{base}{separator}{suffix}")
        };
        if !taken(&candidate) {
            return candidate;
        }
    }
    unreachable!("the u32 name suffix space is exhausted")
}

fn symbol_rename(original: &str, replacement: &str) -> String {
    let raw = super::decorated_c_symbol;
    format!("{}={}", raw(original), raw(replacement))
}

fn main_symbol_rename(replacement: &str) -> String {
    symbol_rename("main", replacement)
}

fn symbol_rename_args(
    main: Option<&str>,
    asan_options: Option<&str>,
    redirects: &[AllocatorRedirect],
) -> Vec<String> {
    main.map(main_symbol_rename)
        .into_iter()
        .chain(asan_options.map(|replacement| symbol_rename(ASAN_OPTIONS_SYMBOL, replacement)))
        .chain(redirects.iter().map(allocation_rename))
        .flat_map(|rename| ["--redefine-sym".to_string(), rename])
        .collect()
}

pub fn replacement_main_symbol(path: &Path) -> Result<String, PreparationError> {
    let taken = super::matching(path, super::Side::Either, |name| {
        name.starts_with(PRIVATE_MAIN_SYMBOL)
    })
    .map_err(|error| PreparationError::PreparedTarget {
        path: path.to_path_buf(),
        message: format!("could not choose a private entry-point name: {error}"),
    })?
    .into_iter()
    .collect::<std::collections::HashSet<_>>();

    Ok(replacement_main_symbol_from(&taken))
}

fn replacement_main_symbol_from(taken: &std::collections::HashSet<String>) -> String {
    first_unused_name(PRIVATE_MAIN_SYMBOL, |candidate| taken.contains(candidate))
}

fn internal_alias_names(globals: &[GlobalRequirement]) -> Vec<String> {
    let mut aliases = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for global in globals {
        if matches!(global.linkage, GlobalLinkage::Internal { .. }) {
            let alias = global.link_name.clone();
            if seen.insert(alias.clone()) {
                aliases.push(alias);
            }
        }
    }
    aliases
}

struct RelocatableInspection {
    facts: super::RelocatableDefinitions,
}

impl RelocatableInspection {
    fn read(object: &Path) -> Result<Self, super::ProbeError> {
        super::relocatable_definitions(object, |_| true).map(|facts| Self { facts })
    }

    fn of_member(member: &super::SelectedArchiveMember) -> Result<Self, super::ProbeError> {
        member
            .relocatable_definitions(|_| true)
            .map(|facts| Self { facts })
    }

    fn mentions(&self) -> impl Iterator<Item = &str> {
        self.facts
            .definitions
            .iter()
            .filter(|definition| definition.global)
            .map(|definition| definition.name.as_str())
            .chain(self.facts.imports.iter().map(String::as_str))
    }

    fn imports(&self, name: &str) -> bool {
        self.facts.imports.iter().any(|import| import == name)
    }
}

fn taken_main_names<'a>(
    inspections: impl IntoIterator<Item = &'a RelocatableInspection>,
) -> std::collections::HashSet<String> {
    inspections
        .into_iter()
        .flat_map(RelocatableInspection::mentions)
        .filter(|name| name.starts_with(PRIVATE_MAIN_SYMBOL))
        .map(str::to_string)
        .collect()
}

fn asan_options_replacement<'a>(
    inspections: impl IntoIterator<Item = &'a RelocatableInspection>,
) -> Result<Option<String>, PreparationError> {
    let inspections = inspections.into_iter().collect::<Vec<_>>();
    if !inspections
        .iter()
        .any(|inspection| defines_global_symbol(inspection, ASAN_OPTIONS_SYMBOL))
    {
        return Ok(None);
    }
    let taken = inspections
        .iter()
        .flat_map(|inspection| {
            inspection
                .facts
                .definitions
                .iter()
                .map(|definition| definition.name.as_str())
                .chain(inspection.facts.imports.iter().map(String::as_str))
        })
        .filter(|name| name.starts_with(PRIVATE_ASAN_OPTIONS_SYMBOL))
        .collect::<std::collections::HashSet<_>>();
    Ok(Some(first_unused_name(
        PRIVATE_ASAN_OPTIONS_SYMBOL,
        |candidate| taken.contains(candidate),
    )))
}

fn linker_visible_alias<'a>(
    inspections: impl IntoIterator<Item = &'a RelocatableInspection>,
    aliases: &[String],
) -> Option<String> {
    let mut present = std::collections::HashSet::<&str>::new();
    for inspection in inspections {
        present.extend(inspection.mentions());
    }
    aliases
        .iter()
        .find(|alias| present.contains(alias.as_str()))
        .cloned()
}

fn defined_alias(
    inspection: &RelocatableInspection,
    aliases: impl IntoIterator<Item = String>,
) -> Option<String> {
    let defined = inspection
        .facts
        .definitions
        .iter()
        .map(|definition| definition.name.as_str())
        .collect::<std::collections::HashSet<_>>();
    aliases
        .into_iter()
        .find(|alias| defined.contains(alias.as_str()))
}

fn reserved_alias(inspection: &RelocatableInspection, aliases: &[String]) -> Option<String> {
    linker_visible_alias(std::iter::once(inspection), aliases)
        .or_else(|| defined_alias(inspection, aliases.iter().cloned()))
}

fn defines_global_symbol(inspection: &RelocatableInspection, name: &str) -> bool {
    inspection
        .facts
        .definitions
        .iter()
        .any(|definition| linker_visible_definition(definition, name))
}

fn reserved_allocation_interposer(
    inspection: &RelocatableInspection,
    policy: &AllocationPolicy<'_>,
) -> Option<&'static str> {
    policy
        .redirects
        .iter()
        .flat_map(|redirect| redirect.reserved_names())
        .find(|name| {
            inspection.imports(name)
                || inspection
                    .facts
                    .definitions
                    .iter()
                    .any(|definition| definition.name == *name)
        })
}

fn certified_allocation_redirects(
    inspections: &[&RelocatableInspection],
    policy: &AllocationPolicy<'_>,
) -> Result<Vec<AllocatorRedirect>, String> {
    certify_heap_ownership(inspections, policy)?;
    Ok(policy
        .redirects
        .iter()
        .copied()
        .filter(|redirect| {
            inspections
                .iter()
                .any(|inspection| inspection.imports(redirect.original))
        })
        .map(|redirect| redirect.with_cleanup(true))
        .collect())
}

fn certify_heap_ownership(
    inspections: &[&RelocatableInspection],
    policy: &AllocationPolicy<'_>,
) -> Result<(), String> {
    if inspections.is_empty() {
        return Err("the selected target closure is empty".to_string());
    }
    let mut definitions = std::collections::HashSet::new();
    for inspection in inspections {
        if matches!(inspection.facts.format, super::RelocatableFormat::Other) {
            return Err(
                "a selected target object is not a readable native relocatable object".to_string(),
            );
        }
        if !inspection.facts.imports_complete {
            return Err("a selected target object's import inventory is incomplete".to_string());
        }
        for definition in &inspection.facts.definitions {
            if allocation_lifetime_changing_entry(&definition.name, policy) {
                return Err(format!(
                    "the selected target closure defines `{}`; allocation lifetimes cannot be certified",
                    definition.name
                ));
            }
            if definition.global {
                if !strong_global(definition) {
                    return Err(format!(
                        "the selected target closure has ambiguous weak or common definition `{}`",
                        definition.name
                    ));
                }
                definitions.insert(definition.name.as_str());
            }
        }
    }
    for inspection in inspections {
        for import in &inspection.facts.imports {
            if !definitions.contains(import.as_str())
                && !allocation_lifetime_preserving_import(import, policy)
            {
                return Err(format!(
                    "unapproved external call `{import}` may change allocation lifetimes"
                ));
            }
        }
    }
    Ok(())
}

fn prepare_target(req: &Context) -> Result<PreparationPlan, PreparationError> {
    if matches!(req.kind, InputKind::Archive) {
        return prepare_archive(req);
    }
    let (raw_object, globals, main_plan) = (req.raw_object, req.globals, req.main);

    let callable_main = main_plan.is_some_and(|plan| plan.callable);
    let aliases = internal_alias_names(globals);
    let inspection = match RelocatableInspection::read(raw_object) {
        Ok(inspection) => inspection,
        Err(error) if globals.is_empty() && !req.check_heap_ownership => {
            let defines_main =
                super::defines_symbol(req.target_path, "main").unwrap_or_else(|probe_error| {
                    tracing::debug!(
                        target: "austen::build",
                        path = %req.target_path.display(),
                        error = %probe_error,
                        "could not inspect the target entry point; leaving it for the linker"
                    );
                    false
                });
            if !defines_main {
                tracing::debug!(
                    target: "austen::build",
                    path = %raw_object.display(),
                    %error,
                    "could not inspect relocatable target layout; no preparation is required"
                );
                return Ok(PreparationPlan::unchanged(raw_object.to_path_buf()));
            }
            return prepare_main_only_object(req);
        }
        Err(error) => {
            let message = format!("could not inspect the relocatable object: {error}");
            return Err(if req.check_heap_ownership {
                PreparationError::PreparedTarget {
                    path: raw_object.to_path_buf(),
                    message,
                }
            } else {
                PreparationError::PreparedGlobals {
                    path: raw_object.to_path_buf(),
                    message,
                }
            });
        }
    };
    let defines_main =
        inspection.facts.definitions.iter().any(|definition| {
            main_definition(definition, callable_main, linker_visible_definition)
        });
    if callable_main && !defines_main && !matches!(req.kind, InputKind::CompiledSource) {
        return Err(PreparationError::PreparedTarget {
            path: raw_object.to_path_buf(),
            message: "the specification exports `main`, but the target does not define it"
                .to_string(),
        });
    }
    if let Some(symbol) = reserved_allocation_interposer(&inspection, req.allocation) {
        return Err(PreparationError::PreparedTarget {
            path: raw_object.to_path_buf(),
            message: format!(
                "reserved harness allocation symbol `{symbol}` is already present in the object"
            ),
        });
    }
    if let Some(alias) = reserved_alias(&inspection, &aliases) {
        return Err(PreparationError::PreparedGlobals {
            path: raw_object.to_path_buf(),
            message: format!(
                "reserved internal-global alias `{alias}` is already present in the object"
            ),
        });
    }
    let allocation_redirects = if req.check_heap_ownership {
        certified_allocation_redirects(&[&inspection], req.allocation).map_err(|message| {
            PreparationError::PreparedTarget {
                path: raw_object.to_path_buf(),
                message,
            }
        })?
    } else {
        Vec::new()
    };
    let asan_options_replacement = asan_options_replacement([&inspection])?;
    if !defines_main
        && asan_options_replacement.is_none()
        && globals.is_empty()
        && allocation_redirects.is_empty()
    {
        return Ok(PreparationPlan::unchanged(raw_object.to_path_buf()));
    }

    let output = prepared_path(req, "o");
    let main_replacement = if defines_main {
        Some(match main_plan {
            Some(plan) => plan.replacement.clone(),
            None => replacement_main_symbol_from(&taken_main_names([&inspection])),
        })
    } else {
        None
    };
    let (sections, prepared_globals, global_args) =
        prepare_globals_from_inspection(&inspection, globals, req.placement).map_err(
            |message| PreparationError::PreparedGlobals {
                path: raw_object.to_path_buf(),
                message,
            },
        )?;
    let mut args = symbol_rename_args(
        main_replacement.as_deref(),
        asan_options_replacement.as_deref(),
        &allocation_redirects,
    );
    args.extend(global_args);
    args.extend([
        raw_object.display().to_string(),
        output.display().to_string(),
    ]);
    Ok(PreparationPlan::single_object(
        req.tools.objcopy.clone(),
        args,
        output,
        PreparedObject {
            main: main_replacement.map(PreparedSymbol::Definition),
            asan_options: asan_options_replacement.map(PreparedSymbol::Definition),
            allocation_redirects,
            globals: prepared_globals,
            sections,
        },
    ))
}

fn prepare_main_only_object(req: &Context) -> Result<PreparationPlan, PreparationError> {
    let output = prepared_path(req, "o");
    let replacement = match req.main {
        Some(plan) => plan.replacement.clone(),
        None => replacement_main_symbol(req.target_path)?,
    };
    let mut args = symbol_rename_args(Some(&replacement), None, &[]);
    args.extend([
        req.raw_object.display().to_string(),
        output.display().to_string(),
    ]);
    Ok(PreparationPlan::single_object(
        req.tools.objcopy.clone(),
        args,
        output,
        PreparedObject {
            main: Some(PreparedSymbol::Definition(replacement)),
            asan_options: None,
            allocation_redirects: Vec::new(),
            globals: Vec::new(),
            sections: Vec::new(),
        },
    ))
}

fn prepare_archive(req: &Context) -> Result<PreparationPlan, PreparationError> {
    use super::RelocatableFormat;

    let (globals, archive_roots, main_plan) = (req.globals, req.archive_roots, req.main);
    let fail = |message: String| {
        if globals.is_empty() {
            PreparationError::PreparedTarget {
                path: req.target_path.to_path_buf(),
                message,
            }
        } else {
            PreparationError::PreparedGlobals {
                path: req.target_path.to_path_buf(),
                message,
            }
        }
    };
    let callable_main = main_plan.is_some_and(|plan| plan.callable);
    let inspection_optional = globals.is_empty() && !callable_main && !req.check_heap_ownership;
    let data = match std::fs::read(req.target_path) {
        Ok(data) => data,
        Err(source) if inspection_optional => {
            tracing::debug!(
                target: "austen::build",
                path = %req.target_path.display(),
                error = %source,
                "could not inspect the target entry point; leaving it for the linker"
            );
            return Ok(PreparationPlan::unchanged(req.target_path.to_path_buf()));
        }
        Err(source) => {
            return Err(fail(format!(
                "could not resolve the archive member closure: {}",
                super::ProbeError::Io {
                    path: req.target_path.to_path_buf(),
                    source,
                }
            )));
        }
    };
    let unreadable_closure = |message: String| {
        if !inspection_optional
            || super::archive_extracts_symbol_bytes(&data, req.target_path, "main", archive_roots)
                .unwrap_or(false)
        {
            return Err(fail(message));
        }
        tracing::debug!(
            target: "austen::build",
            path = %req.target_path.display(),
            error = %message,
            "could not inspect the archive's entry point; leaving it for the linker"
        );
        Ok(PreparationPlan::unchanged(req.target_path.to_path_buf()))
    };
    let members = match super::selected_archive_members_bytes(&data, req.target_path, archive_roots)
    {
        Ok(members) => members,
        Err(error) => {
            return unreadable_closure(format!(
                "could not resolve the archive member closure: {error}"
            ));
        }
    };
    let aliases = internal_alias_names(globals);
    struct Member {
        member: super::SelectedArchiveMember,
        inspection: RelocatableInspection,
        rewrite: ObjectRewrite,
    }
    let mut staged = Vec::with_capacity(members.len());
    for member in members {
        let inspection = match RelocatableInspection::of_member(&member) {
            Ok(inspection) => inspection,
            Err(error) => {
                return unreadable_closure(format!(
                    "could not inspect selected archive member {}: {error}",
                    member.index
                ));
            }
        };
        staged.push(Member {
            member,
            inspection,
            rewrite: ObjectRewrite::default(),
        });
    }

    let defines_main = staged.iter().any(|member| {
        member
            .inspection
            .facts
            .definitions
            .iter()
            .any(|definition| main_definition(definition, callable_main, archive_hazard_definition))
    });
    if callable_main && !defines_main {
        return Err(fail(
            "the specification exports `main`, but the selected archive member closure does not define it"
                .to_string(),
        ));
    }
    if let Some(symbol) = staged
        .iter()
        .find_map(|member| reserved_allocation_interposer(&member.inspection, req.allocation))
    {
        return Err(fail(format!(
            "reserved harness allocation symbol `{symbol}` is already present in the selected archive member closure"
        )));
    }
    let allocation_redirects = if req.check_heap_ownership {
        certified_allocation_redirects(
            &staged
                .iter()
                .map(|member| &member.inspection)
                .collect::<Vec<_>>(),
            req.allocation,
        )
        .map_err(&fail)?
    } else {
        Vec::new()
    };
    let asan_options_replacement =
        asan_options_replacement(staged.iter().map(|member| &member.inspection))?;
    if !defines_main
        && asan_options_replacement.is_none()
        && globals.is_empty()
        && allocation_redirects.is_empty()
    {
        return Ok(PreparationPlan::unchanged(req.target_path.to_path_buf()));
    }
    if staged.is_empty() {
        return Err(fail(
            "the archive member closure is empty for the generated harness's link roots"
                .to_string(),
        ));
    }
    if let Some(alias) =
        linker_visible_alias(staged.iter().map(|member| &member.inspection), &aliases)
    {
        return Err(fail(format!(
            "reserved internal-global alias `{alias}` is already present in the selected archive member closure"
        )));
    }
    let inputs = staged
        .iter()
        .map(|member| member.member.source().to_path_buf())
        .filter(|path| path != req.target_path)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if !globals.is_empty() {
        for member in &staged {
            if member.inspection.facts.format == RelocatableFormat::Elf {
                continue;
            }
            return Err(fail(format!(
                "selected archive member {} is not an ELF relocatable object",
                member.member.index
            )));
        }
    }

    let mut assigned = vec![Vec::<ResolvedGlobal>::new(); staged.len()];
    for (global_index, global) in globals.iter().enumerate() {
        let mut providers = Vec::new();
        for (member_index, member) in staged.iter().enumerate() {
            if provides_global(&member.inspection.facts, global) {
                providers.push(member_index);
            }
        }
        let [member_index] = providers.as_slice() else {
            return Err(fail(format!(
                "specified global `{}` resolves to {} selected archive members; expected exactly one{}",
                global.source_name,
                providers.len(),
                match &global.linkage {
                    GlobalLinkage::External => "".to_string(),
                    GlobalLinkage::Internal { owner, .. } => format!(" with owner `{owner}`"),
                }
            )));
        };
        let definition = resolve_global(
            &staged[*member_index].inspection.facts,
            global_index,
            global,
        )
        .map_err(&fail)?;
        assigned[*member_index].push(definition);
    }

    for (member, resolved) in staged.iter().zip(&assigned) {
        let aliases = resolved.iter().filter_map(|item| {
            let spec = &globals[item.spec_index];
            matches!(&spec.linkage, GlobalLinkage::Internal { .. }).then(|| spec.link_name.clone())
        });
        if let Some(alias) = defined_alias(&member.inspection, aliases) {
            return Err(fail(format!(
                "reserved internal-global alias `{alias}` is already defined in selected archive member {}",
                member.member.index
            )));
        }
    }
    for (member, resolved) in staged.iter_mut().zip(assigned) {
        if !resolved.is_empty() {
            validate_global_architecture(&member.inspection.facts).map_err(&fail)?;
        }
        member.rewrite = plan_object_rewrite(&member.inspection.facts, &resolved, globals);
    }
    let all_input_sections = staged
        .iter()
        .flat_map(|member| member.inspection.facts.sections.iter().cloned())
        .collect::<std::collections::HashSet<_>>();
    validate_archive_section_names(
        staged.iter().map(|member| &member.rewrite),
        &all_input_sections,
    )
    .map_err(&fail)?;
    place_global_sections(
        &mut staged
            .iter_mut()
            .map(|member| &mut member.rewrite)
            .collect::<Vec<_>>(),
        req.placement,
    )
    .map_err(&fail)?;

    let main_replacement = if defines_main {
        Some(match main_plan {
            Some(plan) => plan.replacement.clone(),
            None => replacement_main_symbol_from(&taken_main_names(
                staged.iter().map(|member| &member.inspection),
            )),
        })
    } else {
        None
    };
    let member_dir = req.directory.join("target.members");
    let mut materialized = Vec::with_capacity(staged.len());
    let objcopy = req.tools.objcopy.clone();
    let mut commands = Vec::new();
    let mut archive_inputs = Vec::with_capacity(staged.len());
    let mut prepared_globals = Vec::new();
    let mut prepared_sections = Vec::new();
    for Member {
        member,
        inspection,
        rewrite,
    } in staged
    {
        let index = member.index;
        let raw = member_dir.join(format!("{index:08}.raw.o"));
        materialized.push(MaterializedInput {
            path: raw.clone(),
            bytes: member.into_bytes(),
        });
        let (sections, member_globals, global_args) =
            materialize_object_rewrite(&rewrite, globals).map_err(&fail)?;
        let rewrite_main = main_replacement
            .as_ref()
            .filter(|_| inspection.mentions().any(|name| name == "main"))
            .cloned();
        let rewrite_asan_options = asan_options_replacement
            .as_ref()
            .filter(|_| {
                inspection
                    .mentions()
                    .any(|name| name == ASAN_OPTIONS_SYMBOL)
            })
            .cloned();
        let member_redirects = allocation_redirects
            .iter()
            .copied()
            .filter(|redirect| inspection.imports(redirect.original))
            .collect::<Vec<_>>();
        let mut args = symbol_rename_args(
            rewrite_main.as_deref(),
            rewrite_asan_options.as_deref(),
            &member_redirects,
        );
        args.extend(global_args);
        if args.is_empty() {
            archive_inputs.push(raw);
            continue;
        }
        let output = member_dir.join(format!("{index:08}.prepared.o"));
        args.extend([raw.display().to_string(), output.display().to_string()]);
        let defines_main_here =
            inspection.facts.definitions.iter().any(|definition| {
                main_definition(definition, callable_main, strong_text_definition)
            });
        commands.push(CommandSpec {
            program: objcopy.clone(),
            args,
            output: output.clone(),
            validation: Some(PreparedObject {
                main: rewrite_main.map(|replacement| {
                    if defines_main_here {
                        PreparedSymbol::Definition(replacement)
                    } else {
                        PreparedSymbol::Reference(replacement)
                    }
                }),
                asan_options: rewrite_asan_options.map(|replacement| {
                    if defines_global_symbol(&inspection, ASAN_OPTIONS_SYMBOL) {
                        PreparedSymbol::Definition(replacement)
                    } else {
                        PreparedSymbol::Reference(replacement)
                    }
                }),
                allocation_redirects: member_redirects,
                globals: member_globals.clone(),
                sections: sections.clone(),
            }),
        });
        archive_inputs.push(output);
        prepared_globals.extend(member_globals);
        prepared_sections.extend(sections);
    }
    prepared_sections.sort_by_key(|section| section.start);

    let output = prepared_path(req, "a");
    let mut ar_args = vec!["crs".to_string(), output.display().to_string()];
    ar_args.extend(archive_inputs.iter().map(|path| path.display().to_string()));
    commands.push(CommandSpec {
        program: req.tools.ar.clone(),
        args: ar_args,
        output: output.clone(),
        validation: None,
    });
    Ok(PreparationPlan {
        link_input: output,
        commands,
        inputs,
        materialized,
        layout: PreparedLayout {
            globals: prepared_globals,
            sections: prepared_sections,
        },
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedSection {
    pub input: String,
    pub output: String,
    pub start: u64,
    pub size: u64,
    pub align: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedGlobal {
    pub original: String,
    pub link_name: String,
    pub section: String,
    pub address: u64,
    pub size: u64,
    pub internal: bool,
}

type PreparedGlobals = (Vec<PreparedSection>, Vec<PreparedGlobal>, Vec<String>);

fn reference_rename_applied(
    data: &[u8],
    output: &Path,
    side: super::Side,
    old: &str,
    new: &str,
) -> Result<bool, super::ProbeError> {
    let seen = super::matching_from(data, output, side, |name| name == old || name == new)?;
    Ok(!seen.iter().any(|name| name == old) && seen.iter().any(|name| name == new))
}

fn linker_visible_definition(definition: &super::RelocatableDefinition, name: &str) -> bool {
    definition.name == name && definition.global
}

fn strong_global(definition: &super::RelocatableDefinition) -> bool {
    definition.global && !definition.weak && !definition.common
}

fn archive_hazard_definition(definition: &super::RelocatableDefinition, name: &str) -> bool {
    definition.name == name && strong_global(definition)
}

fn callable_text_definition(definition: &super::RelocatableDefinition, name: &str) -> bool {
    linker_visible_definition(definition, name)
        && definition.kind == super::DefinitionKind::Text
        && !definition.common
}

fn strong_text_definition(definition: &super::RelocatableDefinition, name: &str) -> bool {
    callable_text_definition(definition, name) && !definition.weak
}

fn main_definition(
    definition: &super::RelocatableDefinition,
    callable: bool,
    hazard: fn(&super::RelocatableDefinition, &str) -> bool,
) -> bool {
    if callable {
        callable_text_definition(definition, "main")
    } else {
        hazard(definition, "main")
    }
}

fn owner_definition(definition: &super::RelocatableDefinition, name: &str) -> bool {
    strong_text_definition(definition, name) && definition.section.is_some()
}

fn strong_data_definition(
    definition: &super::RelocatableDefinition,
    name: &str,
    size: u64,
    global: bool,
) -> bool {
    definition.name == name
        && definition.kind == super::DefinitionKind::Data
        && definition.global == global
        && !definition.weak
        && !definition.common
        && definition.size == size
        && definition
            .section
            .as_ref()
            .is_some_and(|section| section.writable)
}

fn specified_global_definition(
    definition: &super::RelocatableDefinition,
    global: &GlobalRequirement,
) -> bool {
    let internal = matches!(&global.linkage, GlobalLinkage::Internal { .. });
    strong_data_definition(definition, &global.source_name, global.size, !internal)
}

fn owner_name(global: &GlobalRequirement) -> Option<&str> {
    match &global.linkage {
        GlobalLinkage::External => None,
        GlobalLinkage::Internal {
            effective_owner, ..
        } => Some(effective_owner),
    }
}

fn owner_definitions(facts: &super::RelocatableDefinitions, owner: &str) -> usize {
    facts
        .definitions
        .iter()
        .filter(|definition| owner_definition(definition, owner))
        .count()
}

fn provides_global(facts: &super::RelocatableDefinitions, global: &GlobalRequirement) -> bool {
    facts
        .definitions
        .iter()
        .filter(|definition| specified_global_definition(definition, global))
        .count()
        == 1
        && owner_name(global).is_none_or(|owner| owner_definitions(facts, owner) == 1)
}

#[derive(Clone)]
struct ResolvedGlobal {
    spec_index: usize,
    definition: super::RelocatableDefinition,
}

fn resolve_global(
    facts: &super::RelocatableDefinitions,
    spec_index: usize,
    global: &GlobalRequirement,
) -> Result<ResolvedGlobal, String> {
    let candidates = facts
        .definitions
        .iter()
        .filter(|definition| definition.name == global.source_name)
        .collect::<Vec<_>>();
    let exact = candidates
        .iter()
        .copied()
        .filter(|definition| specified_global_definition(definition, global))
        .collect::<Vec<_>>();
    let [definition] = exact.as_slice() else {
        let linkage = if matches!(&global.linkage, GlobalLinkage::Internal { .. }) {
            "local"
        } else {
            "global"
        };
        return Err(format!(
            "`{}` must resolve to exactly one strong, non-COMMON {linkage} Data symbol of the declared {}-byte extent in a writable containing section (found {} matching definition(s), {} exact)",
            global.source_name,
            global.size,
            candidates.len(),
            exact.len()
        ));
    };
    if candidates.len() != 1 {
        return Err(format!(
            "`{}` is ambiguous in the relocatable object (found {} exact-name definitions)",
            global.source_name,
            candidates.len()
        ));
    }
    let definition = *definition;
    let section = definition
        .section
        .as_ref()
        .expect("the exact predicate requires a section");
    if facts
        .sections
        .iter()
        .filter(|name| *name == &section.name)
        .count()
        != 1
    {
        return Err(format!(
            "the containing section {:?} of `{}` is not uniquely named in the object",
            section.name, global.source_name
        ));
    }
    if definition.size != 0
        && section
            .offset
            .checked_add(definition.size)
            .is_none_or(|end| end > section.size)
    {
        return Err(format!(
            "the extent of `{}` falls outside its containing section {:?}",
            global.source_name, section.name
        ));
    }
    if let GlobalLinkage::Internal { owner, .. } = &global.linkage {
        let actual_owner = owner_name(global).expect("internal globals have an owner");
        let owners = owner_definitions(facts, actual_owner);
        if owners != 1 {
            return Err(format!(
                "internal object `{}` names owner `{owner}` (linked here as `{actual_owner}`), which must be one strong global Text definition in the same relocatable object (found {owners})",
                global.source_name,
            ));
        }
    }
    Ok(ResolvedGlobal {
        spec_index,
        definition: definition.clone(),
    })
}

pub fn placement_host_architecture() -> Result<super::MachineArchitecture, String> {
    super::host_architecture().ok_or_else(|| {
        format!(
            "fixed global placement does not support Austen's host architecture `{}`",
            std::env::consts::ARCH
        )
    })
}

fn validate_global_architecture(facts: &super::RelocatableDefinitions) -> Result<(), String> {
    if facts.format != super::RelocatableFormat::Elf {
        return Err(
            "fixed global placement is currently supported only for ELF relocatable objects; Mach-O requires an ld64 segment/section placement contract"
                .to_string(),
        );
    }
    let host_architecture = placement_host_architecture()?;
    if facts.architecture != host_architecture {
        return Err(format!(
            "target architecture {:?} does not match Austen's host architecture {:?}",
            facts.architecture, host_architecture
        ));
    }
    Ok(())
}

#[derive(Default)]
struct ObjectRewrite {
    sections: Vec<RewriteSection>,
    globals: Vec<RewriteGlobal>,
}

struct RewriteSection {
    input: String,
    output: String,
    start: Option<u64>,
    size: u64,
    align: u64,
    first_spec: usize,
}

struct RewriteGlobal {
    spec_index: usize,
    section_index: usize,
    offset: u64,
}

fn plan_object_rewrite(
    facts: &super::RelocatableDefinitions,
    resolved: &[ResolvedGlobal],
    globals: &[GlobalRequirement],
) -> ObjectRewrite {
    let mut taken_sections = facts
        .sections
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let mut rewrite = ObjectRewrite::default();
    for item in resolved {
        let detail = item
            .definition
            .section
            .as_ref()
            .expect("resolved data has a section");
        let section_index = if let Some(index) = rewrite
            .sections
            .iter()
            .position(|section| section.input == detail.name)
        {
            rewrite.sections[index].first_spec =
                rewrite.sections[index].first_spec.min(item.spec_index);
            index
        } else {
            let output = first_unused_spelling(
                &globals[item.spec_index].section_name_base,
                '.',
                |candidate| taken_sections.contains(candidate),
            );
            taken_sections.insert(output.clone());
            rewrite.sections.push(RewriteSection {
                input: detail.name.clone(),
                output,
                start: None,
                size: detail.size,
                align: detail.align,
                first_spec: item.spec_index,
            });
            rewrite.sections.len() - 1
        };
        rewrite.globals.push(RewriteGlobal {
            spec_index: item.spec_index,
            section_index,
            offset: detail.offset,
        });
    }
    rewrite
}

fn validate_archive_section_names<'a>(
    rewrites: impl IntoIterator<Item = &'a ObjectRewrite>,
    all_input_sections: &std::collections::HashSet<String>,
) -> Result<(), String> {
    let sections = rewrites
        .into_iter()
        .flat_map(|rewrite| &rewrite.sections)
        .collect::<Vec<_>>();
    if let Some(section) = sections
        .iter()
        .find(|section| all_input_sections.contains(&section.output))
    {
        return Err(format!(
            "prepared section name {:?} collides with an existing section in another archive member",
            section.output
        ));
    }
    let mut outputs = std::collections::HashSet::new();
    if let Some(section) = sections
        .iter()
        .find(|section| !outputs.insert(section.output.as_str()))
    {
        return Err(format!(
            "two archive members require the same prepared section name {:?}",
            section.output
        ));
    }
    Ok(())
}

fn place_global_sections(
    rewrites: &mut [&mut ObjectRewrite],
    placement: GlobalPlacement,
) -> Result<(), String> {
    let mut sections = rewrites
        .iter()
        .enumerate()
        .flat_map(|(rewrite_index, rewrite)| {
            rewrite
                .sections
                .iter()
                .enumerate()
                .map(move |(section_index, section)| {
                    (section.first_spec, rewrite_index, section_index)
                })
        })
        .collect::<Vec<_>>();
    sections.sort_unstable();

    let mut cursor = placement.base;
    for (_, rewrite_index, section_index) in sections {
        let section = &mut rewrites[rewrite_index].sections[section_index];
        let alignment = section.align.max(placement.alignment);
        if !alignment.is_power_of_two() {
            return Err(format!(
                "section {:?} has unsupported non-power-of-two alignment {}",
                section.input, section.align
            ));
        }
        let start = align_up(cursor, alignment)
            .ok_or_else(|| "fixed global-section addresses overflow u64".to_string())?;
        let end = start
            .checked_add(section.size.max(1))
            .ok_or_else(|| "fixed global-section extent overflows u64".to_string())?;
        cursor = align_up(end, placement.alignment)
            .ok_or_else(|| "fixed global-section addresses overflow u64".to_string())?;
        section.start = Some(start);
    }
    Ok(())
}

fn materialize_object_rewrite(
    rewrite: &ObjectRewrite,
    globals: &[GlobalRequirement],
) -> Result<PreparedGlobals, String> {
    let sections = rewrite
        .sections
        .iter()
        .map(|section| PreparedSection {
            input: section.input.clone(),
            output: section.output.clone(),
            start: section.start.expect("placement precedes materialisation"),
            size: section.size,
            align: section.align,
        })
        .collect::<Vec<_>>();
    let mut args = Vec::new();
    for section in &sections {
        args.extend([
            "--rename-section".to_string(),
            format!("{}={}", section.input, section.output),
        ]);
    }

    let mut prepared = Vec::with_capacity(rewrite.globals.len());
    for item in &rewrite.globals {
        let spec = &globals[item.spec_index];
        let section = &sections[item.section_index];
        let link_name = spec.link_name.clone();
        let internal = matches!(&spec.linkage, GlobalLinkage::Internal { .. });
        if internal {
            args.extend([
                "--redefine-sym".to_string(),
                format!(
                    "{}={}",
                    super::decorated_c_symbol(&spec.source_name),
                    super::decorated_c_symbol(&link_name)
                ),
                "--globalize-symbol".to_string(),
                super::decorated_c_symbol(&link_name),
            ]);
        }
        let address = section
            .start
            .checked_add(item.offset)
            .ok_or_else(|| "fixed global address overflows u64".to_string())?;
        prepared.push(PreparedGlobal {
            original: spec.source_name.clone(),
            link_name,
            section: section.output.clone(),
            address,
            size: spec.size,
            internal,
        });
    }
    Ok((sections, prepared, args))
}

fn prepare_globals_from_inspection(
    inspection: &RelocatableInspection,
    globals: &[GlobalRequirement],
    placement: GlobalPlacement,
) -> Result<PreparedGlobals, String> {
    if globals.is_empty() {
        return Ok((Vec::new(), Vec::new(), Vec::new()));
    }
    validate_global_architecture(&inspection.facts)?;
    let resolved = globals
        .iter()
        .enumerate()
        .map(|(index, global)| resolve_global(&inspection.facts, index, global))
        .collect::<Result<Vec<_>, _>>()?;
    let mut rewrite = plan_object_rewrite(&inspection.facts, &resolved, globals);
    place_global_sections(&mut [&mut rewrite], placement)?;
    materialize_object_rewrite(&rewrite, globals)
}

fn align_up(value: u64, align: u64) -> Option<u64> {
    debug_assert!(align.is_power_of_two());
    value.checked_add(align - 1).map(|sum| sum & !(align - 1))
}

fn validate_prepared_target(
    output: &Path,
    prepare: &PreparedObject,
) -> Result<(), PreparationError> {
    let invalid = |message: String| PreparationError::PreparedTarget {
        path: output.to_path_buf(),
        message,
    };
    if prepare.main.is_none()
        && prepare.asan_options.is_none()
        && prepare.allocation_redirects.is_empty()
        && prepare.globals.is_empty()
    {
        return Ok(());
    }
    let data = std::fs::read(output).map_err(|source| PreparationError::Read {
        path: output.to_path_buf(),
        source,
    })?;
    for (original, prepared) in [
        ("main", &prepare.main),
        (ASAN_OPTIONS_SYMBOL, &prepare.asan_options),
    ] {
        let Some(prepared) = prepared else { continue };
        let replacement = match prepared {
            PreparedSymbol::Definition(replacement) | PreparedSymbol::Reference(replacement) => {
                replacement
            }
        };
        let unreadable = |error| invalid(format!("could not read the rewritten symbols: {error}"));
        let applied = match prepared {
            PreparedSymbol::Definition(_) => {
                super::rename_applied_from(&data, output, original, replacement)
                    .map_err(unreadable)?
            }
            PreparedSymbol::Reference(_) => {
                reference_rename_applied(&data, output, super::Side::Either, original, replacement)
                    .map_err(unreadable)?
            }
        };
        if !applied {
            let mapping = symbol_rename(original, replacement);
            return Err(invalid(format!(
                "the object-copy tool reported success, but `{mapping}` was not applied"
            )));
        }
    }
    for redirect in &prepare.allocation_redirects {
        if !reference_rename_applied(
            &data,
            output,
            super::Side::Undefined,
            redirect.original,
            redirect.replacement,
        )
        .map_err(|error| invalid(format!("could not read the rewritten symbols: {error}")))?
        {
            return Err(invalid(format!(
                "the object-copy tool reported success, but `{}` was not applied",
                allocation_rename(redirect)
            )));
        }
    }
    if prepare.globals.is_empty() {
        return Ok(());
    }
    let mut wanted = prepare
        .globals
        .iter()
        .map(|global| global.link_name.as_str())
        .collect::<std::collections::HashSet<_>>();
    wanted.extend(
        prepare
            .globals
            .iter()
            .filter(|global| global.internal)
            .map(|global| global.original.as_str()),
    );
    let facts = super::relocatable_definitions_from(&data, output, |name| wanted.contains(name))
        .map_err(|error| invalid(format!("could not read rewritten globals: {error}")))?;
    for global in &prepare.globals {
        let found = facts
            .definitions
            .iter()
            .filter(|definition| {
                strong_data_definition(definition, &global.link_name, global.size, true)
                    && definition.section.as_ref().is_some_and(|section| {
                        section.name == global.section
                            && section
                                .offset
                                .checked_add(global.size)
                                .is_some_and(|end| end <= section.size)
                    })
            })
            .count();
        if found != 1 {
            return Err(invalid(format!(
                "the object-copy tool reported success, but `{}` is not one strong global Data symbol with its declared {}-byte extent in section {:?}",
                global.link_name, global.size, global.section
            )));
        }
        if global.internal
            && facts
                .definitions
                .iter()
                .any(|definition| definition.name == global.original)
        {
            return Err(invalid(format!(
                "the object-copy tool left internal name `{}` behind after creating `{}`",
                global.original, global.link_name
            )));
        }
    }
    for section in &prepare.sections {
        if !facts.sections.iter().any(|name| name == &section.output) {
            return Err(invalid(format!(
                "the object-copy tool did not create renamed section {:?}",
                section.output
            )));
        }
    }
    Ok(())
}

fn prepared_path(req: &Context, extension: &str) -> PathBuf {
    let candidate = req.directory.join(format!("target.prepared.{extension}"));
    if same_file(req.target_path, &candidate) {
        return req.directory.join(format!("target.prepared.1.{extension}"));
    }
    candidate
}

fn same_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    if let (Ok(left), Ok(right)) = (left.canonicalize(), right.canonicalize())
        && left == right
    {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(left), Ok(right)) = (left.metadata(), right.metadata()) {
            return left.dev() == right.dev() && left.ino() == right.ino();
        }
    }
    false
}

pub struct CommandSpec {
    program: String,
    args: Vec<String>,
    output: PathBuf,
    validation: Option<PreparedObject>,
}

#[derive(Clone)]
struct PreparedObject {
    main: Option<PreparedSymbol>,
    asan_options: Option<PreparedSymbol>,
    allocation_redirects: Vec<AllocatorRedirect>,
    globals: Vec<PreparedGlobal>,
    sections: Vec<PreparedSection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PreparedSymbol {
    Definition(String),
    Reference(String),
}
