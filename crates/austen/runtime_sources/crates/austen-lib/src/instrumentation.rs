mod registry;
pub use registry::{consumers, steered_by};

use std::path::Path;

use crate::external::{NotFound, TargetCompiler, discover_tool};
use crate::object::{ProbeError, Side};
use strum::{EnumCount, EnumIter, IntoEnumIterator};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, EnumCount, EnumIter)]
pub enum Coverage {
    TracePcGuard,
    SancovCounters,
    TraceCmp,
    LegacyTraceCmp,
    TraceDiv,
    TraceGep,
    IndirectCalls,
    LegacyIndirectCalls,
    StackDepth,
    TracePc,
    FunctionTrace,
    AflInlineMap,
}

impl Coverage {
    pub fn marks(self, symbol: &str) -> bool {
        match self {
            Coverage::TracePcGuard => {
                symbol == "__sanitizer_cov_trace_pc_guard"
                    || symbol == "__sanitizer_cov_trace_pc_guard_init"
            }
            Coverage::SancovCounters => symbol == "__sanitizer_cov_8bit_counters_init",
            Coverage::TraceCmp => matches!(
                symbol,
                "__sanitizer_cov_trace_cmp1"
                    | "__sanitizer_cov_trace_cmp2"
                    | "__sanitizer_cov_trace_cmp4"
                    | "__sanitizer_cov_trace_cmp8"
                    | "__sanitizer_cov_trace_const_cmp1"
                    | "__sanitizer_cov_trace_const_cmp2"
                    | "__sanitizer_cov_trace_const_cmp4"
                    | "__sanitizer_cov_trace_const_cmp8"
                    | "__sanitizer_cov_trace_switch"
            ),
            Coverage::LegacyTraceCmp => matches!(
                symbol,
                "__sanitizer_cov_trace_cmp"
                    | "__sanitizer_cov_trace_cmpf"
                    | "__sanitizer_cov_trace_cmpd"
            ),
            Coverage::TraceDiv => symbol.starts_with("__sanitizer_cov_trace_div"),
            Coverage::TraceGep => symbol == "__sanitizer_cov_trace_gep",
            Coverage::IndirectCalls => symbol == "__sanitizer_cov_trace_pc_indir",
            Coverage::LegacyIndirectCalls => symbol == "__sanitizer_cov_indir_call16",
            Coverage::StackDepth => symbol == "__sancov_lowest_stack",
            Coverage::TracePc => symbol == "__sanitizer_cov_trace_pc",
            Coverage::FunctionTrace => symbol == "__cyg_profile_func_enter",
            Coverage::AflInlineMap => symbol == "__afl_area_ptr" || symbol == "__afl_prev_loc",
        }
    }

    pub fn of(symbol: &str) -> Option<Coverage> {
        Coverage::iter().find(|f| f.marks(symbol))
    }

    pub fn built_with(self) -> &'static str {
        match self {
            Coverage::TracePcGuard => {
                "-fsanitize-coverage=trace-pc-guard (or afl-cc in \
                                       AFL_LLVM_INSTRUMENT=NATIVE mode)"
            }
            Coverage::SancovCounters => {
                "-fsanitize=fuzzer-no-link (or -fsanitize-coverage=inline-8bit-counters)"
            }
            Coverage::TraceCmp => "-fsanitize=fuzzer-no-link (or -fsanitize-coverage=trace-cmp)",
            Coverage::LegacyTraceCmp => "legacy/GCC sanitizer comparison coverage",
            Coverage::TraceDiv => "-fsanitize-coverage=trace-div",
            Coverage::TraceGep => "-fsanitize-coverage=trace-gep",
            Coverage::IndirectCalls => {
                "-fsanitize=fuzzer-no-link (or -fsanitize-coverage=indirect-calls)"
            }
            Coverage::LegacyIndirectCalls => "legacy LLVM indirect-call coverage",
            Coverage::StackDepth => {
                "-fsanitize=fuzzer-no-link (or -fsanitize-coverage=stack-depth)"
            }
            Coverage::TracePc => "-fsanitize-coverage=trace-pc",
            Coverage::FunctionTrace => {
                "-finstrument-functions (or -finstrument-functions-after-inlining)"
            }
            Coverage::AflInlineMap => "afl-cc in its default inlined mode",
        }
    }

    pub const SPELLINGS: &'static [(Spelling, &'static [Coverage])] = &[
        (
            Spelling::Cflag("-fsanitize=fuzzer-no-link"),
            &[
                Coverage::SancovCounters,
                Coverage::TraceCmp,
                Coverage::IndirectCalls,
                Coverage::StackDepth,
            ],
        ),
        (
            Spelling::SancovGroup("trace-pc-guard"),
            &[Coverage::TracePcGuard],
        ),
        (
            Spelling::SancovGroup("inline-8bit-counters"),
            &[Coverage::SancovCounters],
        ),
        (Spelling::SancovGroup("trace-cmp"), &[Coverage::TraceCmp]),
        (Spelling::SancovGroup("trace-div"), &[Coverage::TraceDiv]),
        (Spelling::SancovGroup("trace-gep"), &[Coverage::TraceGep]),
        (
            Spelling::SancovGroup("indirect-calls"),
            &[Coverage::IndirectCalls],
        ),
        (
            Spelling::SancovGroup("stack-depth"),
            &[Coverage::StackDepth],
        ),
        (Spelling::SancovGroup("trace-pc"), &[Coverage::TracePc]),
        (Spelling::SancovUnsupported("trace-bb"), &[]),
        (Spelling::SancovUnsupported("8bit-counters"), &[]),
        (Spelling::SancovUnsupported("trace-pc-entry-exit"), &[]),
        (Spelling::SancovUnsupported("inline-bool-flag"), &[]),
        (Spelling::SancovUnsupported("trace-loads"), &[]),
        (Spelling::SancovUnsupported("trace-stores"), &[]),
        (Spelling::SancovUnsupported("control-flow"), &[]),
        (
            Spelling::UnsupportedCflagPrefix("-fsanitize-coverage-stack-depth-callback-min="),
            &[],
        ),
        (Spelling::SancovModifier("edge"), &[]),
        (Spelling::SancovModifier("bb"), &[]),
        (Spelling::SancovModifier("func"), &[]),
        (Spelling::SancovModifier("no-prune"), &[]),
        (Spelling::SancovMetadata(PC_TABLE), &[]),
        (
            Spelling::Env("AFL_LLVM_INSTRUMENT", "NATIVE"),
            &[Coverage::TracePcGuard],
        ),
        (
            Spelling::Cflag("-finstrument-functions"),
            &[Coverage::FunctionTrace],
        ),
        (
            Spelling::Cflag("-finstrument-functions-after-inlining"),
            &[Coverage::FunctionTrace],
        ),
    ];

    pub fn token(self) -> &'static str {
        match self {
            Coverage::TracePcGuard => "trace-pc-guard",
            Coverage::SancovCounters => "sancov-counters",
            Coverage::TraceCmp => "trace-cmp",
            Coverage::LegacyTraceCmp => "legacy-trace-cmp",
            Coverage::TraceDiv => "trace-div",
            Coverage::TraceGep => "trace-gep",
            Coverage::IndirectCalls => "indirect-calls",
            Coverage::LegacyIndirectCalls => "legacy-indirect-calls",
            Coverage::StackDepth => "stack-depth",
            Coverage::TracePc => "trace-pc",
            Coverage::FunctionTrace => "function-trace",
            Coverage::AflInlineMap => "afl-inline-map",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spelling {
    Cflag(&'static str),
    SancovGroup(&'static str),
    SancovMetadata(&'static str),
    SancovModifier(&'static str),
    SancovUnsupported(&'static str),
    UnsupportedCflagPrefix(&'static str),
    Env(&'static str, &'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompileRequest {
    pub cflags: &'static [&'static str],
    pub env: &'static [(&'static str, &'static str)],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Implied {
    pub spellings: Vec<Spelling>,
    pub flavors: Vec<Coverage>,
    pub metadata: Vec<String>,
    pub unsupported: Vec<String>,
    pub unrecognised: Vec<String>,
    pub unrecognised_disabled: Vec<String>,
    pub invalid: Vec<String>,
}

impl CompileRequest {
    pub const NONE: CompileRequest = CompileRequest {
        cflags: &[],
        env: &[],
    };

    pub fn implies(&self) -> Implied {
        implied_by(self.cflags.iter().copied(), self.env.iter().copied())
    }
}

const SANCOV: &str = "-fsanitize-coverage=";
const NO_SANCOV: &str = "-fno-sanitize-coverage=";
const SANITIZE: &str = "-fsanitize=";
const NO_SANITIZE: &str = "-fno-sanitize=";
const FUZZER_NO_LINK: &str = "-fsanitize=fuzzer-no-link";

pub const PC_TABLE: &str = "pc-table";

pub fn is_control_word(flag: &str) -> bool {
    if flag.starts_with(SANCOV)
        || flag.starts_with(NO_SANCOV)
        || row_for_cflag(flag).is_some()
        || row_for_unsupported_cflag(flag).is_some()
        || matches!(
            flag,
            "-fno-instrument-functions" | "-fno-instrument-functions-after-inlining"
        )
    {
        return true;
    }
    if let Some(values) = flag.strip_prefix(SANITIZE) {
        return values
            .split(',')
            .any(|value| matches!(value, "fuzzer" | "fuzzer-no-link"));
    }
    flag.strip_prefix(NO_SANITIZE).is_some_and(|values| {
        values
            .split(',')
            .any(|value| matches!(value, "all" | "fuzzer" | "fuzzer-no-link"))
    })
}

pub fn implied_by<'c, 'e>(
    cflags: impl IntoIterator<Item = &'c str>,
    env: impl IntoIterator<Item = (&'e str, &'e str)>,
) -> Implied {
    let cflags: Vec<_> = cflags.into_iter().collect();
    let env: Vec<_> = env.into_iter().collect();

    let (mut fuzzer, mut fuzzer_no_link) = (false, false);
    for flag in &cflags {
        if let Some(values) = flag.strip_prefix(SANITIZE) {
            for value in values.split(',') {
                match value {
                    "fuzzer" => fuzzer = true,
                    "fuzzer-no-link" => fuzzer_no_link = true,
                    _ => {}
                }
            }
        } else if let Some(values) = flag.strip_prefix(NO_SANITIZE) {
            for value in values.split(',') {
                match value {
                    "fuzzer" => fuzzer = false,
                    "fuzzer-no-link" => fuzzer_no_link = false,
                    "all" => {
                        fuzzer = false;
                        fuzzer_no_link = false;
                    }
                    _ => {}
                }
            }
        }
    }

    let mut enabled = Vec::new();
    let mut active_modifiers = Vec::new();
    let mut active_metadata = Vec::new();
    let mut active_unsupported = Vec::new();
    let mut spellings = Vec::new();
    let mut unrecognised = Vec::new();
    let mut unrecognised_disabled = Vec::new();
    let mut invalid = Vec::new();

    let env_row = env
        .iter()
        .rev()
        .find(|(key, _)| *key == "AFL_LLVM_INSTRUMENT")
        .copied()
        .and_then(|(key, value)| row_for_env(key, value));
    if let Some((_, flavors)) = env_row {
        set_flavors(&mut enabled, flavors, true);
    }

    if fuzzer || fuzzer_no_link {
        let (_, flavors) = row_for_cflag(FUZZER_NO_LINK)
            .expect("Coverage::SPELLINGS names the libFuzzer umbrella");
        set_flavors(&mut enabled, flavors, true);
        let (spelling, _) = row_for_group(PC_TABLE)
            .expect("Coverage::SPELLINGS names libFuzzer's pc-table metadata");
        set_metadata(&mut active_metadata, spelling, true);
    }

    let mut function_trace = false;
    let mut function_trace_after_inlining = false;
    let mut umbrella_recorded = false;
    for flag in &cflags {
        if let Some(groups) = flag.strip_prefix(SANCOV) {
            if groups.parse::<u64>().is_ok() {
                enabled.clear();
                active_modifiers.clear();
                active_metadata.clear();
                active_unsupported
                    .retain(|control| matches!(control, UnsupportedControl::Cflag(_)));
                continue;
            }
            for group in groups.split(',').filter(|group| !group.is_empty()) {
                match row_for_group(group) {
                    Some((spelling, flavors)) => {
                        spellings.push(spelling);
                        set_modifier(&mut active_modifiers, spelling, true);
                        set_metadata(&mut active_metadata, spelling, true);
                        set_unsupported(&mut active_unsupported, spelling, true);
                        set_flavors(&mut enabled, flavors, true);
                    }
                    None => unrecognised.push(group.to_string()),
                }
            }
        } else if let Some(groups) = flag.strip_prefix(NO_SANCOV) {
            for group in groups.split(',').filter(|group| !group.is_empty()) {
                match row_for_group(group) {
                    Some((spelling, flavors)) => {
                        set_modifier(&mut active_modifiers, spelling, false);
                        set_metadata(&mut active_metadata, spelling, false);
                        set_unsupported(&mut active_unsupported, spelling, false);
                        set_flavors(&mut enabled, flavors, false);
                    }
                    None => unrecognised_disabled.push(group.to_string()),
                }
            }
        } else if let Some(spelling) = row_for_unsupported_cflag(flag) {
            spellings.push(spelling);
            set_unsupported_cflag(&mut active_unsupported, flag);
        } else if *flag == "-finstrument-functions" {
            let (spelling, _) =
                row_for_cflag(flag).expect("Coverage::SPELLINGS names -finstrument-functions");
            spellings.push(spelling);
            function_trace = true;
        } else if *flag == "-fno-instrument-functions" {
            invalid.push((*flag).to_string());
        } else if *flag == "-finstrument-functions-after-inlining" {
            let (spelling, _) = row_for_cflag(flag)
                .expect("Coverage::SPELLINGS names -finstrument-functions-after-inlining");
            spellings.push(spelling);
            function_trace_after_inlining = true;
        } else if *flag == "-fno-instrument-functions-after-inlining" {
            invalid.push((*flag).to_string());
        } else if let Some(values) = flag.strip_prefix(SANITIZE) {
            if !umbrella_recorded
                && values
                    .split(',')
                    .any(|value| matches!(value, "fuzzer" | "fuzzer-no-link"))
            {
                let (spelling, _) = row_for_cflag(FUZZER_NO_LINK)
                    .expect("Coverage::SPELLINGS names the libFuzzer umbrella");
                spellings.push(spelling);
                umbrella_recorded = true;
            }
        } else if let Some((spelling, flavors)) = row_for_cflag(flag) {
            spellings.push(spelling);
            set_flavors(&mut enabled, flavors, true);
        }
    }

    set_flavors(
        &mut enabled,
        &[Coverage::FunctionTrace],
        function_trace || function_trace_after_inlining,
    );

    let active_shapes: Vec<_> = active_modifiers
        .iter()
        .copied()
        .filter(|modifier| matches!(*modifier, "edge" | "bb" | "func"))
        .collect();
    let insertion_shape_is_active = !active_shapes.is_empty();
    let base_is_active = [
        Coverage::TracePcGuard,
        Coverage::SancovCounters,
        Coverage::TracePc,
    ]
    .iter()
    .any(|flavor| enabled.contains(flavor));
    if insertion_shape_is_active && !base_is_active {
        set_flavors(&mut enabled, &[Coverage::TracePcGuard], true);
    }
    if active_shapes.len() > 1 {
        invalid.push(format!("-fsanitize-coverage={}", active_shapes.join(",")));
    }
    if let Some((spelling, _)) = env_row {
        spellings.push(spelling);
    }

    let flavors = Coverage::iter()
        .filter(|flavor| enabled.contains(flavor))
        .collect();
    let metadata = active_metadata.into_iter().map(str::to_string).collect();
    let unsupported = active_unsupported
        .into_iter()
        .map(UnsupportedControl::into_string)
        .collect();
    Implied {
        spellings,
        flavors,
        metadata,
        unsupported,
        unrecognised,
        unrecognised_disabled,
        invalid,
    }
}

fn row_for_cflag(want: &str) -> Option<(Spelling, &'static [Coverage])> {
    Coverage::SPELLINGS.iter().find_map(|row| match row.0 {
        Spelling::Cflag(flag) if flag == want => Some(*row),
        _ => None,
    })
}

fn row_for_unsupported_cflag(want: &str) -> Option<Spelling> {
    Coverage::SPELLINGS.iter().find_map(|row| match row.0 {
        Spelling::UnsupportedCflagPrefix(prefix) if want.starts_with(prefix) => Some(row.0),
        _ => None,
    })
}

fn row_for_group(want: &str) -> Option<(Spelling, &'static [Coverage])> {
    Coverage::SPELLINGS.iter().find_map(|row| match row.0 {
        Spelling::SancovGroup(group)
        | Spelling::SancovMetadata(group)
        | Spelling::SancovModifier(group)
        | Spelling::SancovUnsupported(group)
            if group == want =>
        {
            Some(*row)
        }
        _ => None,
    })
}

fn row_for_env(want_key: &str, want_value: &str) -> Option<(Spelling, &'static [Coverage])> {
    Coverage::SPELLINGS.iter().find_map(|row| match row.0 {
        Spelling::Env(key, value) if key == want_key && value == want_value => Some(*row),
        _ => None,
    })
}

fn toggle<T: PartialEq>(active: &mut Vec<T>, item: T, on: bool) {
    if on {
        if !active.contains(&item) {
            active.push(item);
        }
    } else {
        active.retain(|existing| *existing != item);
    }
}

fn set_modifier(active: &mut Vec<&'static str>, spelling: Spelling, on: bool) {
    if let Spelling::SancovModifier(modifier) = spelling {
        toggle(active, modifier, on);
    }
}

fn set_metadata(active: &mut Vec<&'static str>, spelling: Spelling, on: bool) {
    if let Spelling::SancovMetadata(metadata) = spelling {
        toggle(active, metadata, on);
    }
}

#[derive(Debug, PartialEq, Eq)]
enum UnsupportedControl {
    Sancov(&'static str),
    Cflag(String),
}

impl UnsupportedControl {
    fn into_string(self) -> String {
        match self {
            UnsupportedControl::Sancov(group) => group.to_string(),
            UnsupportedControl::Cflag(flag) => flag,
        }
    }
}

fn set_unsupported(active: &mut Vec<UnsupportedControl>, spelling: Spelling, on: bool) {
    if let Spelling::SancovUnsupported(group) = spelling {
        toggle(active, UnsupportedControl::Sancov(group), on);
    }
}

fn set_unsupported_cflag(active: &mut Vec<UnsupportedControl>, flag: &str) {
    toggle(active, UnsupportedControl::Cflag(flag.to_string()), true);
}

fn set_flavors(enabled: &mut Vec<Coverage>, flavors: &[Coverage], on: bool) {
    for flavor in flavors {
        toggle(enabled, *flavor, on);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub coverage: Coverage,
    pub symbol: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    Clean,
    Consumable(Vec<Evidence>),
    Inert(Vec<Evidence>),
    Rejected(Vec<Evidence>),
    Unlinkable(Vec<Evidence>),
    Defines(Vec<Evidence>),
}

pub fn classify<'a, 'd>(
    consumes: &[Coverage],
    rejects: &[Coverage],
    resolved: &[Coverage],
    undefined: impl IntoIterator<Item = &'a str>,
    defined: impl IntoIterator<Item = &'d str>,
) -> Found {
    let defines = witnesses(defined);
    if !defines.is_empty() {
        return Found::Defines(defines);
    }

    let (mut consumable, mut inert, mut rejected, mut unlinkable) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for evidence in witnesses(undefined) {
        let flavor = evidence.coverage;
        if rejects.contains(&flavor) {
            rejected.push(evidence);
        } else if consumes.contains(&flavor) {
            consumable.push(evidence);
        } else if resolved.contains(&flavor) {
            inert.push(evidence);
        } else {
            unlinkable.push(evidence);
        }
    }

    if !rejected.is_empty() {
        Found::Rejected(rejected)
    } else if !unlinkable.is_empty() {
        Found::Unlinkable(unlinkable)
    } else if !consumable.is_empty() {
        Found::Consumable(consumable)
    } else if !inert.is_empty() {
        Found::Inert(inert)
    } else {
        Found::Clean
    }
}

fn witnesses<'a>(symbols: impl IntoIterator<Item = &'a str>) -> Vec<Evidence> {
    let mut witness: [Option<String>; Coverage::COUNT] = Default::default();
    for symbol in symbols {
        let Some(slot) = Coverage::iter().position(|f| f.marks(symbol)) else {
            continue;
        };
        witness[slot].get_or_insert_with(|| symbol.to_string());
    }
    Coverage::iter()
        .zip(witness)
        .filter_map(|(coverage, symbol)| symbol.map(|symbol| Evidence { coverage, symbol }))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instrumentation {
    Compiled,
    Detected,
    Absent,
    Unknown,
}

pub fn probe(
    consumes: &[Coverage],
    rejects: &[Coverage],
    resolved: &[Coverage],
    target: &Path,
) -> Result<Found, ProbeError> {
    let data = std::fs::read(target).map_err(|source| ProbeError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    if crate::object::is_archive_bytes(&data, target)? {
        return Err(ProbeError::Unreadable {
            path: target.to_path_buf(),
            message: "file is a static archive; its instrumentation is a property of the \
                      members the link selects, not of every member it contains"
                .to_string(),
        });
    }
    let undefined = crate::object::matching_from(&data, target, Side::Undefined, is_marker)?;
    let defined = crate::object::matching_from(&data, target, Side::Defined, is_marker)?;
    Ok(classify(
        consumes,
        rejects,
        resolved,
        undefined.iter().map(String::as_str),
        defined.iter().map(String::as_str),
    ))
}

pub fn probe_archive(
    consumes: &[Coverage],
    rejects: &[Coverage],
    resolved: &[Coverage],
    target: &Path,
    roots: &[&str],
) -> Result<Found, ProbeError> {
    let undefined = selected_markers(target, roots, Side::Undefined)?;
    let defined = selected_markers(target, roots, Side::Defined)?;
    Ok(classify(
        consumes,
        rejects,
        resolved,
        undefined.iter().map(String::as_str),
        defined.iter().map(String::as_str),
    ))
}

pub fn resolved_by<'a>(
    link_line: impl IntoIterator<Item = &'a Path>,
) -> (Vec<Coverage>, Vec<ProbeError>) {
    let (mut defined, mut errors) = (Vec::new(), Vec::new());
    for entry in link_line {
        match markers(entry, Side::Defined) {
            Ok(symbols) => defined.extend(symbols),
            Err(e) => errors.push(e),
        }
    }
    let flavors = Coverage::iter()
        .filter(|f| defined.iter().any(|s| Coverage::of(s) == Some(*f)))
        .collect();
    (flavors, errors)
}

fn is_marker(name: &str) -> bool {
    Coverage::of(name).is_some()
}

fn markers(target: &Path, side: Side) -> Result<Vec<String>, ProbeError> {
    crate::object::matching(target, side, is_marker)
}

fn selected_markers(target: &Path, roots: &[&str], side: Side) -> Result<Vec<String>, ProbeError> {
    crate::object::matching_selected_archive(target, roots, side, is_marker)
}

pub const EVALUATION: CompileRequest = CompileRequest {
    cflags: &["-fsanitize-coverage=trace-pc-guard,pc-table"],
    env: &[],
};

pub const EVALUATION_FLAVORS: &[Coverage] = &[Coverage::TracePcGuard];

pub const EVALUATION_CLANG_ENV: &str = "AUSTEN_COVERAGE_CLANG";

pub fn coverage_compiler(request: CompileRequest) -> Result<TargetCompiler, NotFound> {
    Ok(TargetCompiler {
        program: discover_tool(
            EVALUATION_CLANG_ENV,
            "clang",
            &[],
            "install clang, put it on PATH, or set AUSTEN_COVERAGE_CLANG to a \
             clang supporting -fsanitize-coverage",
        )?,
        request,
    })
}

pub fn evaluation_compiler() -> Result<TargetCompiler, NotFound> {
    coverage_compiler(EVALUATION)
}
