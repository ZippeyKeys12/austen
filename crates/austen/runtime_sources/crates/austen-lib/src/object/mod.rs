use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

pub mod prepare;

use object::read::archive::ArchiveFile;
use object::read::macho::{FatArch, FatArch32, FatArch64, MachOFatFile};
use object::{
    BinaryFormat, FileKind, Object, ObjectKind, ObjectSection, ObjectSegment, ObjectSymbol,
    SymbolKind,
};

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {message}")]
    Unreadable { path: PathBuf, message: String },
}

impl ProbeError {
    pub fn path(&self) -> &Path {
        match self {
            ProbeError::Io { path, .. } | ProbeError::Unreadable { path, .. } => path,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Undefined,
    Defined,
    Either,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelocatableFormat {
    Elf,
    MachO,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineArchitecture {
    Aarch64,
    Arm,
    I386,
    X86_64,
    LoongArch64,
    Mips,
    Mips64,
    PowerPc,
    PowerPc64,
    Riscv32,
    Riscv64,
    S390x,
    Sparc64,
    Other,
}

pub fn host_architecture() -> Option<MachineArchitecture> {
    Some(match std::env::consts::ARCH {
        "aarch64" => MachineArchitecture::Aarch64,
        "arm" => MachineArchitecture::Arm,
        "x86" => MachineArchitecture::I386,
        "x86_64" => MachineArchitecture::X86_64,
        "loongarch64" => MachineArchitecture::LoongArch64,
        "mips" | "mipsel" => MachineArchitecture::Mips,
        "mips64" | "mips64el" => MachineArchitecture::Mips64,
        "powerpc" => MachineArchitecture::PowerPc,
        "powerpc64" => MachineArchitecture::PowerPc64,
        "riscv32" => MachineArchitecture::Riscv32,
        "riscv64" => MachineArchitecture::Riscv64,
        "s390x" => MachineArchitecture::S390x,
        "sparc64" => MachineArchitecture::Sparc64,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionKind {
    Data,
    Text,
    Tls,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionSection {
    pub name: String,
    pub offset: u64,
    pub size: u64,
    pub align: u64,
    pub writable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelocatableDefinition {
    pub name: String,
    pub kind: DefinitionKind,
    pub global: bool,
    pub weak: bool,
    pub common: bool,
    pub size: u64,
    pub section: Option<DefinitionSection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveTextDefinition {
    pub name: String,
    pub global: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelocatableDefinitions {
    pub format: RelocatableFormat,
    pub architecture: MachineArchitecture,
    pub sections: Vec<String>,
    pub definitions: Vec<RelocatableDefinition>,
    pub imports: Vec<String>,
    pub imports_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedDefinition {
    pub name: String,
    pub kind: DefinitionKind,
    pub weak: bool,
    pub address: u64,
    pub size: u64,
    pub section: Option<LinkedSection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedSection {
    pub name: String,
    pub address: u64,
    pub size: u64,
    pub allocated: bool,
    pub writable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedLoadSegment {
    pub address: u64,
    pub size: u64,
    pub writable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedDefinitions {
    pub format: RelocatableFormat,
    pub architecture: MachineArchitecture,
    pub executable: bool,
    pub definitions: Vec<LinkedDefinition>,
    pub sections: Vec<LinkedSection>,
    pub load_segments: Vec<LinkedLoadSegment>,
}

pub fn relocatable_definitions(
    target: &Path,
    keep: impl Fn(&str) -> bool,
) -> Result<RelocatableDefinitions, ProbeError> {
    let data = std::fs::read(target).map_err(|source| ProbeError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    relocatable_definitions_bytes(&data, target, &keep)
}

fn relocatable_definitions_from(
    data: &[u8],
    source: &Path,
    keep: impl Fn(&str) -> bool,
) -> Result<RelocatableDefinitions, ProbeError> {
    relocatable_definitions_bytes(data, source, &keep)
}

pub fn linked_definitions(
    target: &Path,
    keep: impl Fn(&str) -> bool,
) -> Result<LinkedDefinitions, ProbeError> {
    let data = std::fs::read(target).map_err(|source| ProbeError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    let unreadable = unreadable_in(target);
    if is_archive(&data) {
        return Err(ProbeError::Unreadable {
            path: target.to_path_buf(),
            message: "file is an archive, not a linked image".to_string(),
        });
    }
    if matches!(
        FileKind::parse(&*data).map_err(unreadable)?,
        FileKind::MachOFat32 | FileKind::MachOFat64
    ) {
        return Err(ProbeError::Unreadable {
            path: target.to_path_buf(),
            message: "file is a universal binary, not one linked image".to_string(),
        });
    }
    let file = object::File::parse(&*data).map_err(unreadable)?;
    let format = relocatable_format(file.format());
    let architecture = machine_architecture(file.architecture());
    let mut sections = Vec::new();
    for section in file.sections() {
        let Ok(name) = section.name() else { continue };
        sections.push(linked_section(&section, name));
    }
    let load_segments = file
        .segments()
        .map(|segment| LinkedLoadSegment {
            address: segment.address(),
            size: segment.size(),
            writable: matches!(
                segment.flags(),
                object::SegmentFlags::Elf { p_flags }
                    if p_flags & object::elf::PF_W != 0
            ),
        })
        .collect();
    let mut out = Vec::new();
    for symbol in object_symbols(&file, Side::Defined) {
        if symbol.is_undefined() || !symbol.is_global() {
            continue;
        }
        let Ok(name) = symbol.name() else { continue };
        let name = undecorated_symbol(file.format(), name);
        if !keep(name) {
            continue;
        }
        let section = symbol
            .section_index()
            .and_then(|index| file.section_by_index(index).ok())
            .and_then(|section| {
                let name = section.name().ok()?;
                Some(linked_section(&section, name))
            });
        out.push(LinkedDefinition {
            name: name.to_string(),
            kind: definition_kind(symbol.kind()),
            weak: symbol.is_weak(),
            address: symbol.address(),
            size: symbol.size(),
            section,
        });
    }
    Ok(LinkedDefinitions {
        format,
        architecture,
        executable: file.kind() == ObjectKind::Executable,
        definitions: out,
        sections,
        load_segments,
    })
}

fn relocatable_format(format: BinaryFormat) -> RelocatableFormat {
    match format {
        BinaryFormat::Elf => RelocatableFormat::Elf,
        BinaryFormat::MachO => RelocatableFormat::MachO,
        _ => RelocatableFormat::Other,
    }
}

fn machine_architecture(architecture: object::Architecture) -> MachineArchitecture {
    match architecture {
        object::Architecture::Aarch64 => MachineArchitecture::Aarch64,
        object::Architecture::Arm => MachineArchitecture::Arm,
        object::Architecture::I386 => MachineArchitecture::I386,
        object::Architecture::X86_64 => MachineArchitecture::X86_64,
        object::Architecture::LoongArch64 => MachineArchitecture::LoongArch64,
        object::Architecture::Mips => MachineArchitecture::Mips,
        object::Architecture::Mips64 => MachineArchitecture::Mips64,
        object::Architecture::PowerPc => MachineArchitecture::PowerPc,
        object::Architecture::PowerPc64 => MachineArchitecture::PowerPc64,
        object::Architecture::Riscv32 => MachineArchitecture::Riscv32,
        object::Architecture::Riscv64 => MachineArchitecture::Riscv64,
        object::Architecture::S390x => MachineArchitecture::S390x,
        object::Architecture::Sparc64 => MachineArchitecture::Sparc64,
        _ => MachineArchitecture::Other,
    }
}

fn section_flags<'data, S: ObjectSection<'data>>(section: &S) -> (bool, bool) {
    match section.flags() {
        object::SectionFlags::Elf { sh_flags } => (
            sh_flags & u64::from(object::elf::SHF_ALLOC) != 0,
            sh_flags & u64::from(object::elf::SHF_WRITE) != 0,
        ),
        _ => (false, false),
    }
}

fn linked_section<'data, S: ObjectSection<'data>>(section: &S, name: &str) -> LinkedSection {
    let (allocated, writable) = section_flags(section);
    LinkedSection {
        name: name.to_string(),
        address: section.address(),
        size: section.size(),
        allocated,
        writable,
    }
}

fn definition_kind(kind: SymbolKind) -> DefinitionKind {
    match kind {
        SymbolKind::Data => DefinitionKind::Data,
        SymbolKind::Text => DefinitionKind::Text,
        SymbolKind::Tls => DefinitionKind::Tls,
        _ => DefinitionKind::Other,
    }
}

fn relocatable_definitions_bytes(
    data: &[u8],
    source: &Path,
    keep: &dyn Fn(&str) -> bool,
) -> Result<RelocatableDefinitions, ProbeError> {
    let unreadable = unreadable_in(source);
    if is_archive(data) {
        return Err(ProbeError::Unreadable {
            path: source.to_path_buf(),
            message: "file is an archive, not one relocatable object".to_string(),
        });
    }
    if matches!(
        FileKind::parse(data).map_err(unreadable)?,
        FileKind::MachOFat32 | FileKind::MachOFat64
    ) {
        return Err(ProbeError::Unreadable {
            path: source.to_path_buf(),
            message: "file is a universal binary, not one relocatable object".to_string(),
        });
    }

    let file = object::File::parse(data).map_err(unreadable)?;
    if file.kind() != ObjectKind::Relocatable {
        return Err(ProbeError::Unreadable {
            path: source.to_path_buf(),
            message: "file is not a relocatable object".to_string(),
        });
    }
    let format = relocatable_format(file.format());
    let mut sections = Vec::new();
    for section in file.sections() {
        if let Ok(name) = section.name() {
            sections.push(name.to_string());
        }
    }

    let mut definitions = Vec::new();
    let mut imports = Vec::new();
    let mut imports_complete = !matches!(format, RelocatableFormat::Other);
    for symbol in file.symbols() {
        if symbol.is_undefined() {
            match symbol.name() {
                Ok(name) if !name.is_empty() => {
                    imports.push(undecorated_symbol(file.format(), name).to_string());
                }
                Ok(_) => {}
                Err(_) => imports_complete = false,
            }
            continue;
        }
        let Ok(name) = symbol.name() else { continue };
        let name = undecorated_symbol(file.format(), name);
        if !keep(name) {
            continue;
        }
        let section = symbol
            .section_index()
            .map(|index| {
                let section = file.section_by_index(index).map_err(unreadable)?;
                let name = section.name().map_err(unreadable)?.to_string();
                let offset = symbol
                    .address()
                    .checked_sub(section.address())
                    .ok_or_else(|| ProbeError::Unreadable {
                        path: source.to_path_buf(),
                        message: format!(
                            "symbol {name:?} precedes the address of its containing section"
                        ),
                    })?;
                Ok(DefinitionSection {
                    name,
                    offset,
                    size: section.size(),
                    align: section.align(),
                    writable: section_flags(&section).1,
                })
            })
            .transpose()?;
        let kind = definition_kind(symbol.kind());
        definitions.push(RelocatableDefinition {
            name: name.to_string(),
            kind,
            global: symbol.is_global(),
            weak: symbol.is_weak(),
            common: symbol.is_common(),
            size: symbol.size(),
            section,
        });
    }
    Ok(RelocatableDefinitions {
        format,
        architecture: machine_architecture(file.architecture()),
        sections,
        definitions,
        imports,
        imports_complete,
    })
}

pub fn defined<'a, 's>(
    link_line: impl IntoIterator<Item = &'a Path>,
    wanted: &[&'s str],
) -> Result<Vec<&'s str>, ProbeError> {
    let mut found = Vec::new();
    for entry in link_line {
        for name in matching(entry, Side::Defined, |name| wanted.contains(&name))? {
            found.extend(wanted.iter().find(|w| **w == name));
        }
    }
    Ok(found)
}

pub fn defines_symbol(target: &Path, symbol: &str) -> Result<bool, ProbeError> {
    Ok(!defined([target], &[symbol])?.is_empty())
}

pub fn matching(
    target: &Path,
    side: Side,
    keep: impl Fn(&str) -> bool,
) -> Result<Vec<String>, ProbeError> {
    let data = std::fs::read(target).map_err(|source| ProbeError::Io {
        path: target.to_path_buf(),
        source,
    })?;

    matching_bytes(&data, target, side, &keep, 0)
}

pub fn mentions_symbol(target: &Path, symbol: &str) -> Result<bool, ProbeError> {
    Ok(!matching(target, Side::Either, |name| name == symbol)?.is_empty())
}

pub fn rename_applied(target: &Path, old: &str, new: &str) -> Result<bool, ProbeError> {
    let data = std::fs::read(target).map_err(|source| ProbeError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    rename_applied_from(&data, target, old, new)
}

fn rename_applied_from(
    data: &[u8],
    source: &Path,
    old: &str,
    new: &str,
) -> Result<bool, ProbeError> {
    if !matching_bytes(data, source, Side::Either, &|name| name == old, 0)?.is_empty() {
        return Ok(false);
    }
    Ok(!matching_bytes(data, source, Side::Defined, &|name| name == new, 0)?.is_empty())
}

pub fn matching_from(
    data: &[u8],
    source: &Path,
    side: Side,
    keep: impl Fn(&str) -> bool,
) -> Result<Vec<String>, ProbeError> {
    matching_bytes(data, source, side, &keep, 0)
}

const SANCOV_GUARDS_SECTION: &str = "__sancov_guards";

const SANCOV_GUARD_BYTES: u64 = 4;

pub fn sancov_guard_count(target: &Path) -> Result<u64, ProbeError> {
    let data = std::fs::read(target).map_err(|source| ProbeError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    sancov_guard_count_from(&data, target)
}

pub fn sancov_guard_count_from(data: &[u8], source: &Path) -> Result<u64, ProbeError> {
    Ok(sancov_guard_bytes(data, source, 0)?.div_ceil(SANCOV_GUARD_BYTES))
}

fn sancov_guard_bytes(data: &[u8], source: &Path, depth: usize) -> Result<u64, ProbeError> {
    if depth > MAX_CONTAINER_DEPTH {
        return Err(too_deep(source));
    }

    if is_archive(data) {
        let members = each_member(data, source, |bytes, member_source| {
            sancov_guard_bytes(bytes, member_source, depth + 1)
        })?;
        return Ok(members.into_iter().fold(0, u64::saturating_add));
    }

    match fat_slices(data, source)? {
        Some(slices) => {
            let mut total = 0_u64;
            for bytes in slices {
                total = total.saturating_add(sancov_guard_bytes(bytes, source, depth + 1)?);
            }
            Ok(total)
        }
        None => {
            let file = object::File::parse(data).map_err(unreadable_in(source))?;
            Ok(file
                .sections()
                .filter(|section| section.name() == Ok(SANCOV_GUARDS_SECTION))
                .map(|section| section.size())
                .fold(0, u64::saturating_add))
        }
    }
}

pub fn archive_text_definitions(
    target: &Path,
    keep: impl Fn(&str) -> bool,
) -> Result<Vec<ArchiveTextDefinition>, ProbeError> {
    let data = std::fs::read(target).map_err(|source| ProbeError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    archive_text_definitions_from(&data, target, keep)
}

pub fn archive_text_definitions_from(
    data: &[u8],
    source: &Path,
    keep: impl Fn(&str) -> bool,
) -> Result<Vec<ArchiveTextDefinition>, ProbeError> {
    archive_text_definitions_bytes(data, source, &keep, 0, false)
}

pub fn matching_selected_archive(
    target: &Path,
    roots: &[&str],
    side: Side,
    keep: impl Fn(&str) -> bool,
) -> Result<Vec<String>, ProbeError> {
    let data = std::fs::read(target).map_err(|source| ProbeError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    matching_selected_archive_bytes(&data, target, roots, side, keep)
}

fn matching_selected_archive_bytes(
    data: &[u8],
    source: &Path,
    roots: &[&str],
    side: Side,
    keep: impl Fn(&str) -> bool,
) -> Result<Vec<String>, ProbeError> {
    Ok(selected_member_symbols(data, source, roots, 0)?
        .iter()
        .flat_map(|member| member.matching(side, &keep))
        .collect())
}

pub fn archive_extracts_symbol(
    target: &Path,
    symbol: &str,
    roots: &[&str],
) -> Result<bool, ProbeError> {
    let data = std::fs::read(target).map_err(|source| ProbeError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    archive_extracts_symbol_bytes(&data, target, symbol, roots)
}

fn archive_extracts_symbol_bytes(
    data: &[u8],
    source: &Path,
    symbol: &str,
    roots: &[&str],
) -> Result<bool, ProbeError> {
    Ok(selected_member_symbols(data, source, roots, 0)?
        .iter()
        .any(|member| member.is_hazard(symbol)))
}

#[derive(Debug, Clone)]
pub struct SelectedArchiveMember {
    pub index: usize,
    source: PathBuf,
    bytes: Vec<u8>,
    symbols: MemberSymbols,
}

impl SelectedArchiveMember {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub fn source(&self) -> &Path {
        &self.source
    }

    pub fn relocatable_definitions(
        &self,
        keep: impl Fn(&str) -> bool,
    ) -> Result<RelocatableDefinitions, ProbeError> {
        relocatable_definitions_bytes(&self.bytes, &self.source, &keep)
    }

    pub fn matching(
        &self,
        side: Side,
        keep: impl Fn(&str) -> bool,
    ) -> Result<Vec<String>, ProbeError> {
        Ok(self.symbols.matching(side, keep))
    }

    pub fn mentions_symbol(&self, symbol: &str) -> Result<bool, ProbeError> {
        Ok(!self
            .matching(Side::Either, |name| name == symbol)?
            .is_empty())
    }
}

pub fn selected_archive_members(
    target: &Path,
    roots: &[&str],
) -> Result<Vec<SelectedArchiveMember>, ProbeError> {
    let data = std::fs::read(target).map_err(|source| ProbeError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    selected_archive_members_bytes(&data, target, roots)
}

fn selected_archive_members_bytes(
    data: &[u8],
    source: &Path,
    roots: &[&str],
) -> Result<Vec<SelectedArchiveMember>, ProbeError> {
    if !is_archive(data) {
        return Err(ProbeError::Unreadable {
            path: source.to_path_buf(),
            message: "file is not a static archive".to_string(),
        });
    }
    let unreadable = unreadable_in(source);
    let archive = ArchiveFile::parse(data).map_err(unreadable)?;
    let mut members = Vec::new();
    let mut symbols = Vec::new();
    for (index, member) in archive.members().enumerate() {
        let member = member.map_err(unreadable)?;
        let member_source;
        let bytes = if member.is_thin() {
            member_source = thin_member_path(source, member.name());
            std::borrow::Cow::Owned(std::fs::read(&member_source).map_err(|source| {
                ProbeError::Io {
                    path: member_source.clone(),
                    source,
                }
            })?)
        } else {
            member_source = source.to_path_buf();
            std::borrow::Cow::Borrowed(member.data(data).map_err(unreadable)?)
        };
        symbols.push(member_symbols_bytes(bytes.as_ref(), &member_source, 1)?);
        members.push((index, member_source, bytes));
    }
    let selected = selected_member_indices(&symbols, roots);
    Ok(members
        .into_iter()
        .zip(symbols)
        .enumerate()
        .filter_map(|(position, ((index, source, bytes), symbols))| {
            selected
                .contains(&position)
                .then_some(SelectedArchiveMember {
                    index,
                    source,
                    bytes: bytes.into_owned(),
                    symbols,
                })
        })
        .collect())
}

#[derive(Debug, Clone, Default)]
struct MemberSymbols {
    defined: HashMap<String, bool>,
    undefined: HashMap<String, UndefinedFacts>,
}

#[derive(Debug, Clone, Copy, Default)]
struct UndefinedFacts {
    strong: bool,
    ordinary: bool,
}

impl MemberSymbols {
    fn extend(&mut self, other: Self) {
        for (name, hazard) in other.defined {
            *self.defined.entry(name).or_default() |= hazard;
        }
        for (name, facts) in other.undefined {
            let retained = self.undefined.entry(name).or_default();
            retained.strong |= facts.strong;
            retained.ordinary |= facts.ordinary;
        }
    }

    fn is_hazard(&self, symbol: &str) -> bool {
        self.defined.get(symbol).copied().unwrap_or(false)
    }

    fn matching(&self, side: Side, keep: impl Fn(&str) -> bool) -> Vec<String> {
        let mut matching = BTreeSet::new();
        if side != Side::Undefined {
            matching.extend(self.defined.keys().filter(|name| keep(name)).cloned());
        }
        if side != Side::Defined {
            matching.extend(
                self.undefined
                    .iter()
                    .filter(|(_, facts)| side == Side::Either || facts.ordinary)
                    .map(|(name, _)| name)
                    .filter(|name| keep(name))
                    .cloned(),
            );
        }
        matching.into_iter().collect()
    }
}

const MAX_CONTAINER_DEPTH: usize = 16;

fn too_deep(source: &Path) -> ProbeError {
    ProbeError::Unreadable {
        path: source.to_path_buf(),
        message: "object container nesting is too deep".to_string(),
    }
}

fn selected_member_symbols(
    data: &[u8],
    source: &Path,
    roots: &[&str],
    depth: usize,
) -> Result<Vec<MemberSymbols>, ProbeError> {
    if depth > MAX_CONTAINER_DEPTH {
        return Err(too_deep(source));
    }

    if is_archive(data) {
        let members = each_member(data, source, |bytes, member_source| {
            member_symbols_bytes(bytes, member_source, depth + 1)
        })?;
        let selected = selected_member_indices(&members, roots);
        return Ok(members
            .into_iter()
            .enumerate()
            .filter(|(index, _)| selected.contains(index))
            .map(|(_, member)| member)
            .collect());
    }

    match fat_slices(data, source)? {
        Some(slices) => {
            let mut selected = Vec::new();
            for bytes in slices {
                selected.extend(selected_member_symbols(bytes, source, roots, depth + 1)?);
            }
            Ok(selected)
        }
        None => Err(ProbeError::Unreadable {
            path: source.to_path_buf(),
            message: "file is not a static archive".to_string(),
        }),
    }
}

fn member_symbols_bytes(
    data: &[u8],
    source: &Path,
    depth: usize,
) -> Result<MemberSymbols, ProbeError> {
    if depth > MAX_CONTAINER_DEPTH {
        return Err(too_deep(source));
    }

    match fat_slices(data, source)? {
        Some(slices) => {
            let mut symbols = MemberSymbols::default();
            for bytes in slices {
                symbols.extend(member_symbols_bytes(bytes, source, depth + 1)?);
            }
            Ok(symbols)
        }
        None => member_symbols_in_object(data).map_err(unreadable_in(source)),
    }
}

fn selected_member_indices<'a>(members: &'a [MemberSymbols], roots: &[&'a str]) -> BTreeSet<usize> {
    let mut scan = Scan::new(members);
    for name in roots.iter().copied() {
        scan.require(name);
    }

    let mut selected = BTreeSet::new();

    while let Some(index) = scan.next_member() {
        let member = &members[index];
        if !scan.wants(member) {
            continue;
        }
        selected.insert(index);
        scan.resolve(member);
        for (name, facts) in &member.undefined {
            if facts.strong {
                scan.require(name);
            }
        }
    }
    selected
}

struct Scan<'a> {
    providers: HashMap<&'a str, Vec<usize>>,
    pending: HashSet<&'a str>,
    provided: HashSet<&'a str>,
    candidates: BTreeSet<usize>,
    cursor: usize,
}

impl<'a> Scan<'a> {
    fn new(members: &'a [MemberSymbols]) -> Self {
        let mut providers = HashMap::<&'a str, Vec<usize>>::new();
        for (index, member) in members.iter().enumerate() {
            for name in member.defined.keys() {
                providers.entry(name.as_str()).or_default().push(index);
            }
        }
        Self {
            providers,
            pending: HashSet::new(),
            provided: HashSet::new(),
            candidates: BTreeSet::new(),
            cursor: 0,
        }
    }

    fn require(&mut self, name: &'a str) {
        if self.provided.contains(name) || !self.pending.insert(name) {
            return;
        }
        if let Some(members) = self.providers.get(name) {
            self.candidates.extend(members.iter().copied());
        }
    }

    fn next_member(&mut self) -> Option<usize> {
        let index = *self
            .candidates
            .range(self.cursor..)
            .next()
            .or(self.candidates.first())?;
        self.candidates.remove(&index);
        self.cursor = index + 1;
        Some(index)
    }

    fn wants(&self, member: &MemberSymbols) -> bool {
        if member.defined.len() < self.pending.len() {
            member
                .defined
                .keys()
                .any(|name| self.pending.contains(name.as_str()))
        } else {
            self.pending
                .iter()
                .any(|name| member.defined.contains_key(*name))
        }
    }

    fn resolve(&mut self, member: &'a MemberSymbols) {
        for name in member.defined.keys().map(String::as_str) {
            self.provided.insert(name);
            self.pending.remove(name);
        }
    }
}

fn unreadable_in(source: &Path) -> impl Fn(object::Error) -> ProbeError + Copy + '_ {
    move |e| ProbeError::Unreadable {
        path: source.to_path_buf(),
        message: e.to_string(),
    }
}

fn is_archive(data: &[u8]) -> bool {
    data.starts_with(AR_MAGIC) || data.starts_with(THIN_AR_MAGIC)
}

fn each_member<T>(
    data: &[u8],
    source: &Path,
    mut visit: impl FnMut(&[u8], &Path) -> Result<T, ProbeError>,
) -> Result<Vec<T>, ProbeError> {
    let unreadable = unreadable_in(source);
    let archive = ArchiveFile::parse(data).map_err(unreadable)?;
    let mut out = Vec::new();
    for member in archive.members() {
        let member = member.map_err(unreadable)?;
        let owned;
        let member_source;
        let bytes = if member.is_thin() {
            member_source = thin_member_path(source, member.name());
            owned = std::fs::read(&member_source).map_err(|source| ProbeError::Io {
                path: member_source.clone(),
                source,
            })?;
            &owned[..]
        } else {
            member_source = source.to_path_buf();
            member.data(data).map_err(unreadable)?
        };
        out.push(visit(bytes, &member_source)?);
    }
    Ok(out)
}

fn fat_slices<'data>(
    data: &'data [u8],
    source: &Path,
) -> Result<Option<Vec<&'data [u8]>>, ProbeError> {
    let unreadable = unreadable_in(source);
    let slices = match FileKind::parse(data).map_err(unreadable)? {
        FileKind::MachOFat32 => arch_slices::<FatArch32>(data, source)?,
        FileKind::MachOFat64 => arch_slices::<FatArch64>(data, source)?,
        _ => return Ok(None),
    };
    Ok(Some(slices))
}

fn arch_slices<'data, Fat: FatArch>(
    data: &'data [u8],
    source: &Path,
) -> Result<Vec<&'data [u8]>, ProbeError> {
    let unreadable = unreadable_in(source);
    let fat = MachOFatFile::<Fat>::parse(data).map_err(unreadable)?;
    fat.arches()
        .iter()
        .map(|arch| arch.data(data).map_err(unreadable))
        .collect()
}

fn matching_bytes(
    data: &[u8],
    source: &Path,
    side: Side,
    keep: &dyn Fn(&str) -> bool,
    depth: usize,
) -> Result<Vec<String>, ProbeError> {
    if depth > MAX_CONTAINER_DEPTH {
        return Err(too_deep(source));
    }

    if is_archive(data) {
        let members = each_member(data, source, |bytes, member_source| {
            matching_bytes(bytes, member_source, side, keep, depth + 1)
        })?;
        return Ok(members.into_iter().flatten().collect());
    }

    match fat_slices(data, source)? {
        Some(slices) => {
            let mut all = Vec::new();
            for bytes in slices {
                all.extend(matching_bytes(bytes, source, side, keep, depth + 1)?);
            }
            Ok(all)
        }
        None => in_object(data, side, keep).map_err(unreadable_in(source)),
    }
}

fn archive_text_definitions_bytes(
    data: &[u8],
    source: &Path,
    keep: &dyn Fn(&str) -> bool,
    depth: usize,
    in_member: bool,
) -> Result<Vec<ArchiveTextDefinition>, ProbeError> {
    if depth > MAX_CONTAINER_DEPTH {
        return Err(too_deep(source));
    }

    if is_archive(data) {
        let members = each_member(data, source, |bytes, member_source| {
            archive_text_definitions_bytes(bytes, member_source, keep, depth + 1, true)
        })?;
        return Ok(members.into_iter().flatten().collect());
    }

    match fat_slices(data, source)? {
        Some(slices) => {
            let mut definitions = Vec::new();
            for bytes in slices {
                definitions.extend(archive_text_definitions_bytes(
                    bytes,
                    source,
                    keep,
                    depth + 1,
                    in_member,
                )?);
            }
            Ok(definitions)
        }
        None if in_member => Ok(relocatable_definitions_bytes(data, source, keep)?
            .definitions
            .into_iter()
            .filter(|definition| definition.kind == DefinitionKind::Text)
            .map(|definition| ArchiveTextDefinition {
                name: definition.name,
                global: definition.global,
            })
            .collect()),
        None => Err(ProbeError::Unreadable {
            path: source.to_path_buf(),
            message: "file is not a static archive".to_string(),
        }),
    }
}

const AR_MAGIC: &[u8] = b"!<arch>\n";
const THIN_AR_MAGIC: &[u8] = b"!<thin>\n";

pub fn is_thin_archive(target: &Path) -> Result<bool, ProbeError> {
    use std::io::Read;

    let mut file = std::fs::File::open(target).map_err(|source| ProbeError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    let mut magic = [0_u8; THIN_AR_MAGIC.len()];
    let mut read = 0;
    while read < magic.len() {
        match file.read(&mut magic[read..]) {
            Ok(0) => return Ok(false),
            Ok(n) => read += n,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(source) => {
                return Err(ProbeError::Io {
                    path: target.to_path_buf(),
                    source,
                });
            }
        }
    }
    Ok(magic == THIN_AR_MAGIC)
}

pub fn is_archive_bytes(data: &[u8], source: &Path) -> Result<bool, ProbeError> {
    archive_bytes_at(data, source, 0)
}

fn archive_bytes_at(data: &[u8], source: &Path, depth: usize) -> Result<bool, ProbeError> {
    if depth > MAX_CONTAINER_DEPTH {
        return Err(too_deep(source));
    }
    if is_archive(data) {
        return Ok(true);
    }
    match fat_slices(data, source)? {
        Some(slices) => {
            for bytes in slices {
                if archive_bytes_at(bytes, source, depth + 1)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        None => Ok(false),
    }
}

fn thin_member_path(archive: &Path, name: &[u8]) -> PathBuf {
    let dir = archive.parent().unwrap_or_else(|| Path::new("."));
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        dir.join(OsStr::from_bytes(name))
    }
    #[cfg(not(unix))]
    {
        dir.join(String::from_utf8_lossy(name).into_owned())
    }
}

const MACHO_C_PREFIX: &str = "_";

fn undecorated_symbol(format: BinaryFormat, name: &str) -> &str {
    if format == BinaryFormat::MachO {
        name.strip_prefix(MACHO_C_PREFIX).unwrap_or(name)
    } else {
        name
    }
}

pub fn decorated_c_symbol(symbol: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("{MACHO_C_PREFIX}{symbol}")
    } else {
        symbol.to_string()
    }
}

fn member_symbols_in_object(bytes: &[u8]) -> Result<MemberSymbols, object::Error> {
    let file = object::File::parse(bytes)?;
    let mut out = MemberSymbols::default();
    for (symbol, ordinary) in object_symbols_with_origin(&file, true) {
        let Ok(name) = symbol.name() else { continue };
        let name = undecorated_symbol(file.format(), name);
        if symbol.is_undefined() {
            let facts = out.undefined.entry(name.to_string()).or_default();
            facts.strong |= !symbol.is_weak();
            facts.ordinary |= ordinary;
        } else if symbol.is_global() {
            let hazard = !symbol.is_weak() && !symbol.is_common();
            *out.defined.entry(name.to_string()).or_default() |= hazard;
        }
    }
    Ok(out)
}

fn object_symbols_with_origin<'file, 'data>(
    file: &'file object::File<'data>,
    include_dynamic: bool,
) -> impl Iterator<Item = (object::read::Symbol<'data, 'file>, bool)> + 'file {
    let dynamic = include_dynamic.then(|| file.dynamic_symbols().map(|symbol| (symbol, false)));
    file.symbols()
        .map(|symbol| (symbol, true))
        .chain(dynamic.into_iter().flatten())
}

fn object_symbols<'file, 'data>(
    file: &'file object::File<'data>,
    side: Side,
) -> impl Iterator<Item = object::read::Symbol<'data, 'file>> + 'file {
    object_symbols_with_origin(file, side != Side::Undefined).map(|(symbol, _)| symbol)
}

fn in_object(
    bytes: &[u8],
    side: Side,
    keep: &dyn Fn(&str) -> bool,
) -> Result<Vec<String>, object::Error> {
    let file = object::File::parse(bytes)?;
    let mut out = Vec::new();
    for symbol in object_symbols(&file, side) {
        let wanted = match side {
            Side::Undefined => symbol.is_undefined(),
            Side::Defined => !symbol.is_undefined() && symbol.is_global(),
            Side::Either => symbol.is_undefined() || symbol.is_global(),
        };
        if !wanted {
            continue;
        }
        let Ok(name) = symbol.name() else { continue };
        let name = undecorated_symbol(file.format(), name);
        if keep(name) {
            out.push(name.to_string());
        }
    }
    Ok(out)
}
