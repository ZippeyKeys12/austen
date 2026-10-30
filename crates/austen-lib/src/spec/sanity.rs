use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::spec::{
    BaseType, BinOp, CType, ClauseShapeError, Const, IndexTerm, Init, Lat, LogicalConstraint,
    Pattern, PatternKind, PredicateDef, Request, RequestName, Sctype, SpecModule, Sym, Term, UnOp,
};

pub const DEFAULT_PROJECTION_DEPTH: usize = 6;
const MAX_FLOW_FACTS: usize = 512;
const MAX_CONDITION_RELATIONS: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiagnosticCode {
    TrivialPostcondition,
    ReturnUnmentioned,
    OutputPartiallyConstrained,
    ResourceContentsUnconstrained,
    NoInputOutputRelation,
    InputNotUsedInPost,
}

impl DiagnosticCode {
    pub const ALL: [DiagnosticCode; 6] = [
        DiagnosticCode::TrivialPostcondition,
        DiagnosticCode::ReturnUnmentioned,
        DiagnosticCode::OutputPartiallyConstrained,
        DiagnosticCode::ResourceContentsUnconstrained,
        DiagnosticCode::NoInputOutputRelation,
        DiagnosticCode::InputNotUsedInPost,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            DiagnosticCode::TrivialPostcondition => "ASC_TRIVIAL_POSTCONDITION",
            DiagnosticCode::ReturnUnmentioned => "ASC_RETURN_UNMENTIONED",
            DiagnosticCode::OutputPartiallyConstrained => "ASC_OUTPUT_PARTIALLY_CONSTRAINED",
            DiagnosticCode::ResourceContentsUnconstrained => "ASC_RESOURCE_CONTENTS_UNCONSTRAINED",
            DiagnosticCode::NoInputOutputRelation => "ASC_NO_INPUT_OUTPUT_RELATION",
            DiagnosticCode::InputNotUsedInPost => "ASC_INPUT_NOT_USED_IN_POST",
        }
    }

    pub const fn severity(self) -> Severity {
        match self {
            DiagnosticCode::ReturnUnmentioned | DiagnosticCode::OutputPartiallyConstrained => {
                Severity::Warning
            }
            DiagnosticCode::TrivialPostcondition
            | DiagnosticCode::ResourceContentsUnconstrained
            | DiagnosticCode::NoInputOutputRelation
            | DiagnosticCode::InputNotUsedInPost => Severity::Informational,
        }
    }
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for DiagnosticCode {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for DiagnosticCode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let token = <std::borrow::Cow<'_, str> as Deserialize>::deserialize(d)?;
        token.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown sanity diagnostic code `{0}`")]
pub struct UnknownDiagnosticCode(String);

impl FromStr for DiagnosticCode {
    type Err = UnknownDiagnosticCode;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|code| code.as_str() == value)
            .ok_or_else(|| UnknownDiagnosticCode(value.to_owned()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LimitationCode {
    DuplicateLogicalFunction,
    DuplicatePredicate,
    OpaquePredicate,
    PartialSubject,
    PredicateSummary,
    UnboundCArguments,
    UnknownExpression,
    UnknownFunction,
}

impl LimitationCode {
    pub const ALL: [LimitationCode; 8] = [
        LimitationCode::DuplicateLogicalFunction,
        LimitationCode::DuplicatePredicate,
        LimitationCode::OpaquePredicate,
        LimitationCode::PartialSubject,
        LimitationCode::PredicateSummary,
        LimitationCode::UnboundCArguments,
        LimitationCode::UnknownExpression,
        LimitationCode::UnknownFunction,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            LimitationCode::DuplicateLogicalFunction => "ASC_LIMIT_DUPLICATE_LOGICAL_FUNCTION",
            LimitationCode::DuplicatePredicate => "ASC_LIMIT_DUPLICATE_PREDICATE",
            LimitationCode::OpaquePredicate => "ASC_LIMIT_OPAQUE_PREDICATE",
            LimitationCode::PartialSubject => "ASC_LIMIT_PARTIAL_SUBJECT",
            LimitationCode::PredicateSummary => "ASC_LIMIT_PREDICATE_SUMMARY",
            LimitationCode::UnboundCArguments => "ASC_LIMIT_UNBOUND_C_ARGUMENTS",
            LimitationCode::UnknownExpression => "ASC_LIMIT_UNKNOWN_EXPRESSION",
            LimitationCode::UnknownFunction => "ASC_LIMIT_UNKNOWN_FUNCTION",
        }
    }
}

impl fmt::Display for LimitationCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LimitationCode {
    type Err = UnknownDiagnosticCode;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|code| code.as_str() == value)
            .ok_or_else(|| UnknownDiagnosticCode(value.to_owned()))
    }
}

impl Serialize for LimitationCode {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for LimitationCode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let token = <std::borrow::Cow<'_, str> as Deserialize>::deserialize(d)?;
        token.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Informational,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    Observed,
    NoEvidence,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchCoverage {
    EveryPath,
    SomeBranch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SanityPhase {
    Signature,
    Precondition,
    Postcondition,
    Predicate,
    LogicalFunction,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct IrLocation {
    pub function: Option<String>,
    pub phase: SanityPhase,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    Argument,
    GhostArgument,
    Global,
    PreStateResource,
    ReturnValue,
    PostStateResource,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Subject {
    pub kind: SubjectKind,
    pub name: String,
    pub symbol: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projection: Vec<String>,
}

impl Subject {
    pub fn display_name(&self) -> String {
        let mut out = self.name.clone();
        for part in &self.projection {
            if part.starts_with('[') || part.starts_with('<') {
                out.push_str(part);
            } else {
                out.push('.');
                out.push_str(part);
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Value,
    Control,
    AddressOnly,
}

fn compose_relation_kind(left: RelationKind, right: RelationKind) -> RelationKind {
    if left == RelationKind::AddressOnly || right == RelationKind::AddressOnly {
        RelationKind::AddressOnly
    } else if left == RelationKind::Control || right == RelationKind::Control {
        RelationKind::Control
    } else {
        RelationKind::Value
    }
}

fn relation_link_neighbor(
    node: &Atom,
    from: &Atom,
    to: &Atom,
    kind: RelationKind,
    projectable: bool,
    limit: usize,
) -> Option<Atom> {
    if node.origin != from.origin || !node.projection.starts_with(&from.projection) {
        return None;
    }
    let suffix = &node.projection[from.projection.len()..];
    if !suffix.is_empty() && (!projectable || kind != RelationKind::Value) {
        return None;
    }
    to.project_path(suffix, limit).ok()
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RelationSummary {
    pub output: Subject,
    pub input: Subject,
    pub kind: RelationKind,
    pub coverage: BranchCoverage,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<IrLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectSummary {
    pub subject: Subject,
    pub ownership_or_shape_use: Evidence,
    pub value_constraint: Evidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_contents_constraint: Option<Evidence>,
    pub input_output_relation: Evidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constraint_coverage: Option<BranchCoverage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<IrLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionSummary {
    pub function: String,
    pub trivial_postcondition: bool,
    pub subjects: Vec<SubjectSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<RelationSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SanityDiagnostic {
    pub code: DiagnosticCode,
    pub severity: Severity,
    pub function: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<Subject>,
    pub message: String,
    pub explanation: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<IrLocation>,
    pub suppressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AnalysisLimitation {
    pub code: LimitationCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<Subject>,
    pub message: String,
    pub location: IrLocation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanityOptions {
    pub function: Option<String>,
    pub pedantic: bool,
    pub suppress: BTreeSet<DiagnosticCode>,
    pub projection_depth: usize,
}

impl Default for SanityOptions {
    fn default() -> Self {
        Self {
            function: None,
            pedantic: false,
            suppress: BTreeSet::new(),
            projection_depth: DEFAULT_PROJECTION_DEPTH,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SanityReport {
    pub schema_version: u32,
    pub diagnostics: Vec<SanityDiagnostic>,
    pub limitations: Vec<AnalysisLimitation>,
    pub functions: Vec<FunctionSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suppressed_codes: Vec<DiagnosticCode>,
}

impl SanityReport {
    pub const SCHEMA_VERSION: u32 = 1;

    pub fn visible_diagnostics(&self) -> impl Iterator<Item = &SanityDiagnostic> {
        self.diagnostics.iter().filter(|d| !d.suppressed)
    }

    pub fn has_warnings(&self) -> bool {
        self.visible_diagnostics()
            .any(|d| d.severity == Severity::Warning)
    }
}

pub fn analyze_module(module: &SpecModule, options: &SanityOptions) -> SanityReport {
    Analyzer::new(module, options).run()
}

type Projection = Vec<String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SubjectKey(usize);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Origin {
    Subject(SubjectKey),
    Formal(usize),
    InternalResource(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Atom {
    origin: Origin,
    projection: Projection,
}

impl Atom {
    fn subject(key: SubjectKey) -> Self {
        Self {
            origin: Origin::Subject(key),
            projection: Vec::new(),
        }
    }

    fn formal(index: usize) -> Self {
        Self {
            origin: Origin::Formal(index),
            projection: Vec::new(),
        }
    }

    fn internal(index: usize) -> Self {
        Self {
            origin: Origin::InternalResource(index),
            projection: Vec::new(),
        }
    }

    fn projected(&self, part: &str, limit: usize) -> Result<Self, Self> {
        if self.projection.len() >= limit {
            return Err(self.clone());
        }
        let mut atom = self.clone();
        atom.projection.push(part.to_owned());
        Ok(atom)
    }

    fn project_path(&self, path: &[String], limit: usize) -> Result<Self, Self> {
        let mut atom = self.clone();
        for part in path {
            atom = atom.projected(part, limit)?;
        }
        Ok(atom)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Flow {
    direct: BTreeSet<Atom>,
    fields: BTreeMap<String, Flow>,
    authoritative_fields: BTreeSet<String>,
    control: BTreeSet<Atom>,
    shape_control: BTreeSet<Atom>,
    conditional: BTreeSet<Atom>,
    exclusive: BTreeSet<(Atom, Atom)>,
    unknown: BTreeSet<Atom>,
    incomplete: bool,
    projection_lost: bool,
    condition: Option<Box<ConditionEvidence>>,
}

impl Flow {
    fn atom(atom: Atom) -> Self {
        Self {
            direct: BTreeSet::from([atom]),
            ..Self::default()
        }
    }

    fn field(part: impl Into<String>, flow: Flow) -> Self {
        let part = part.into();
        Self {
            fields: BTreeMap::from([(part.clone(), flow)]),
            authoritative_fields: BTreeSet::from([part]),
            ..Self::default()
        }
    }

    fn merge(&mut self, other: &Flow) {
        self.direct.extend(other.direct.iter().cloned());
        self.control.extend(other.control.iter().cloned());
        self.shape_control
            .extend(other.shape_control.iter().cloned());
        self.conditional.extend(other.conditional.iter().cloned());
        self.exclusive.extend(other.exclusive.iter().cloned());
        self.unknown.extend(other.unknown.iter().cloned());
        self.incomplete |= other.incomplete;
        self.projection_lost |= other.projection_lost;
        self.authoritative_fields
            .extend(other.authoritative_fields.iter().cloned());
        for (field, flow) in &other.fields {
            self.fields.entry(field.clone()).or_default().merge(flow);
        }
        self.widen_if_large();
    }

    fn union(flows: impl IntoIterator<Item = Flow>) -> Flow {
        let mut out = Flow::default();
        for flow in flows {
            out.merge(&flow);
        }
        out
    }

    fn alternatives(flows: impl IntoIterator<Item = Flow>) -> Flow {
        let flows: Vec<_> = flows.into_iter().collect();
        if flows.is_empty() {
            return Flow::default();
        }
        let mut out = Flow::union(flows.iter().cloned());
        out.projection_lost |= flows.len() > 1;
        out.authoritative_fields =
            flows
                .iter()
                .skip(1)
                .fold(flows[0].authoritative_fields.clone(), |common, flow| {
                    common
                        .intersection(&flow.authoritative_fields)
                        .cloned()
                        .collect()
                });

        let branch_atoms: Vec<_> = flows.iter().map(Flow::atoms).collect();
        let common_atoms = branch_atoms
            .iter()
            .skip(1)
            .fold(branch_atoms[0].clone(), |common, atoms| {
                common.intersection(atoms).cloned().collect()
            });
        let all_atoms: Vec<_> = branch_atoms
            .iter()
            .flat_map(|atoms| atoms.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        'pairs: for (index, left) in all_atoms.iter().enumerate() {
            for right in all_atoms.iter().skip(index + 1) {
                if branch_atoms
                    .iter()
                    .any(|atoms| atoms.contains(left) && atoms.contains(right))
                {
                    continue;
                }
                if out.exclusive.len() >= MAX_CONDITION_RELATIONS {
                    out.incomplete = true;
                    break 'pairs;
                }
                out.exclusive.insert((left.clone(), right.clone()));
            }
        }
        out.conditional.extend(
            all_atoms
                .into_iter()
                .filter(|atom| !common_atoms.contains(atom)),
        );
        out
    }

    fn atoms(&self) -> BTreeSet<Atom> {
        let mut out = self.direct.clone();
        out.extend(self.control.iter().cloned());
        out.extend(self.shape_control.iter().cloned());
        for flow in self.fields.values() {
            out.extend(flow.atoms());
        }
        out
    }

    fn data_atoms(&self) -> BTreeSet<Atom> {
        let mut out = self.direct.clone();
        for flow in self.fields.values() {
            out.extend(flow.data_atoms());
        }
        out
    }

    fn unknown_atoms(&self) -> BTreeSet<Atom> {
        let mut out = self.unknown.clone();
        for flow in self.fields.values() {
            out.extend(flow.unknown_atoms());
        }
        out
    }

    fn conditional_atoms(&self) -> BTreeSet<Atom> {
        let mut out = self.conditional.clone();
        for flow in self.fields.values() {
            out.extend(flow.conditional_atoms());
        }
        out
    }

    fn mark_unknown_from(&mut self, flow: &Flow) {
        self.unknown.extend(flow.atoms());
        self.unknown.extend(flow.unknown_atoms());
        self.incomplete |= flow.incomplete;
    }

    fn project(&self, part: &str, limit: usize) -> Flow {
        let mut fallback = Flow::default();
        for atom in &self.direct {
            let conditional = self.conditional.contains(atom);
            match atom.projected(part, limit) {
                Ok(atom) => {
                    if conditional {
                        fallback.conditional.insert(atom.clone());
                    }
                    fallback.direct.insert(atom);
                }
                Err(atom) => {
                    fallback.unknown.insert(atom);
                }
            }
        }
        for atom in &self.unknown {
            match atom.projected(part, limit) {
                Ok(atom) | Err(atom) => {
                    fallback.unknown.insert(atom);
                }
            }
        }
        fallback.control.extend(self.control.iter().cloned());
        fallback
            .shape_control
            .extend(self.shape_control.iter().cloned());
        fallback.conditional.extend(
            self.control
                .iter()
                .filter(|atom| self.conditional.contains(*atom))
                .cloned(),
        );
        fallback.conditional.extend(
            self.shape_control
                .iter()
                .filter(|atom| self.conditional.contains(*atom))
                .cloned(),
        );
        fallback.exclusive.extend(self.exclusive.iter().cloned());
        fallback.incomplete = self.incomplete;
        fallback.projection_lost = self.projection_lost;

        if let Some(field) = self.fields.get(part) {
            let mut out = field.clone();
            let child_atoms = out.atoms();
            out.conditional.extend(
                self.conditional
                    .iter()
                    .filter(|atom| child_atoms.contains(*atom))
                    .cloned(),
            );
            out.control.extend(self.control.iter().cloned());
            out.shape_control.extend(self.shape_control.iter().cloned());
            out.conditional.extend(
                self.control
                    .iter()
                    .filter(|atom| self.conditional.contains(*atom))
                    .cloned(),
            );
            out.conditional.extend(
                self.shape_control
                    .iter()
                    .filter(|atom| self.conditional.contains(*atom))
                    .cloned(),
            );
            out.unknown.extend(self.unknown.iter().cloned());
            out.exclusive.extend(self.exclusive.iter().cloned());
            out.incomplete |= self.incomplete;
            out.projection_lost |= self.projection_lost;
            if !self.authoritative_fields.contains(part) {
                out.merge(&fallback);
            }
            return out;
        }
        fallback
    }

    fn project_path(&self, path: &[String], limit: usize) -> Flow {
        path.iter()
            .fold(self.clone(), |flow, part| flow.project(part, limit))
    }

    fn with_control(mut self, flow: &Flow) -> Self {
        self.control.extend(flow.atoms());
        self.conditional.extend(flow.conditional_atoms());
        self.unknown.extend(flow.unknown_atoms());
        self.incomplete |= flow.incomplete;
        self.projection_lost = true;
        self
    }

    fn with_shape_control(mut self, flow: &Flow) -> Self {
        self.shape_control.extend(flow.atoms());
        self.conditional.extend(flow.conditional_atoms());
        self.unknown.extend(flow.unknown_atoms());
        self.incomplete |= flow.incomplete;
        self.projection_lost = true;
        self
    }

    fn discriminator_flow(&self) -> Flow {
        let mut out = Flow {
            direct: self.direct.clone(),
            control: self.control.clone(),
            shape_control: self.shape_control.clone(),
            unknown: self.unknown.clone(),
            incomplete: self.incomplete,
            projection_lost: true,
            ..Flow::default()
        };
        let atoms = out.atoms();
        out.conditional.extend(
            self.conditional
                .iter()
                .filter(|atom| atoms.contains(*atom))
                .cloned(),
        );
        out
    }

    fn known_constructor_projection(&self) -> Option<String> {
        if !self.direct.is_empty() || self.fields.len() != 1 {
            return None;
        }
        self.fields
            .keys()
            .next()
            .filter(|field| field.starts_with('<'))
            .cloned()
    }

    fn transformed(mut self) -> Self {
        self.projection_lost = true;
        self
    }

    fn opaque_from(flows: &[Flow]) -> Self {
        let mut out = Flow {
            incomplete: true,
            projection_lost: true,
            ..Flow::default()
        };
        for flow in flows {
            out.mark_unknown_from(flow);
        }
        out
    }

    fn fact_count_over(&self, limit: usize) -> bool {
        fn count(flow: &Flow, total: &mut usize, limit: usize) -> bool {
            *total = total
                .saturating_add(flow.direct.len())
                .saturating_add(flow.control.len())
                .saturating_add(flow.shape_control.len())
                .saturating_add(flow.conditional.len())
                .saturating_add(flow.unknown.len());
            if *total > limit {
                return true;
            }
            flow.fields.values().any(|child| count(child, total, limit))
        }
        count(self, &mut 0, limit)
    }

    fn collect_origins(&self, origins: &mut BTreeSet<Origin>) {
        origins.extend(
            self.direct
                .iter()
                .chain(&self.control)
                .chain(&self.shape_control)
                .chain(&self.conditional)
                .chain(&self.unknown)
                .map(|atom| atom.origin.clone()),
        );
        for flow in self.fields.values() {
            flow.collect_origins(origins);
        }
    }

    fn atoms_are_exclusive(&self, left: &Atom, right: &Atom) -> bool {
        self.exclusive.iter().any(|(a, b)| {
            (atom_prefix_matches(a, left) && atom_prefix_matches(b, right))
                || (atom_prefix_matches(a, right) && atom_prefix_matches(b, left))
        })
    }

    fn projection_preserving(&self) -> bool {
        !self.projection_lost
            && !self.incomplete
            && self.fields.is_empty()
            && self.control.is_empty()
            && self.shape_control.is_empty()
            && self.conditional.is_empty()
            && self.unknown.is_empty()
            && self.direct.len() == 1
    }

    fn widen_if_large(&mut self) {
        if !self.fact_count_over(MAX_FLOW_FACTS) {
            return;
        }
        let mut origins = BTreeSet::new();
        self.collect_origins(&mut origins);
        *self = Flow {
            unknown: origins
                .into_iter()
                .map(|origin| Atom {
                    origin,
                    projection: Vec::new(),
                })
                .collect(),
            incomplete: true,
            projection_lost: true,
            ..Flow::default()
        };
    }

    fn mappings(&self, prefix: &mut Projection, out: &mut Vec<(Projection, Atom)>) {
        out.extend(
            self.direct
                .iter()
                .cloned()
                .map(|atom| (prefix.clone(), atom)),
        );
        for (field, flow) in &self.fields {
            prefix.push(field.clone());
            flow.mappings(prefix, out);
            prefix.pop();
        }
    }

    fn unknown_mappings(&self, prefix: &mut Projection, out: &mut Vec<(Projection, Atom)>) {
        out.extend(
            self.unknown
                .iter()
                .cloned()
                .map(|atom| (prefix.clone(), atom)),
        );
        for (field, flow) in &self.fields {
            prefix.push(field.clone());
            flow.unknown_mappings(prefix, out);
            prefix.pop();
        }
    }
}

type Env = BTreeMap<Sym, Flow>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct LogicalSummary {
    result: Flow,
    truth: Truth,
    total: bool,
    opaque: bool,
    parameter_count: usize,
    global_symbols: Vec<Sym>,
    condition: Option<ConditionEvidence>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Truth {
    AlwaysTrue,
    AlwaysFalse,
    Nontrivial,
    #[default]
    Unknown,
}

impl Truth {
    fn not(self) -> Self {
        match self {
            Truth::AlwaysTrue => Truth::AlwaysFalse,
            Truth::AlwaysFalse => Truth::AlwaysTrue,
            Truth::Nontrivial => Truth::Nontrivial,
            Truth::Unknown => Truth::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Coverage {
    some: bool,
    every: bool,
}

impl Coverage {
    const EVERY: Coverage = Coverage {
        some: true,
        every: true,
    };

    const SOME: Coverage = Coverage {
        some: true,
        every: false,
    };

    fn branch(conditional: bool) -> Self {
        if conditional { Self::SOME } else { Self::EVERY }
    }

    fn narrow(self, conditional: bool) -> Self {
        if conditional {
            scale_coverage(self, Self::SOME)
        } else {
            self
        }
    }

    fn merge_alternative(self, other: Self) -> Self {
        Self {
            some: self.some || other.some,
            every: self.every && other.every,
        }
    }

    fn public(self) -> Option<BranchCoverage> {
        if self.every {
            Some(BranchCoverage::EveryPath)
        } else if self.some {
            Some(BranchCoverage::SomeBranch)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Default)]
struct PathEvidence {
    observed: BTreeMap<Projection, Coverage>,
    unknown: BTreeSet<Projection>,
    locations: BTreeSet<IrLocation>,
}

impl PathEvidence {
    fn observe(&mut self, projection: Projection, coverage: Coverage, location: &IrLocation) {
        self.observed
            .entry(projection)
            .and_modify(|old| {
                old.some |= coverage.some;
                old.every |= coverage.every;
            })
            .or_insert(coverage);
        self.locations.insert(location.clone());
    }

    fn mark_unknown(&mut self, projection: Projection, location: &IrLocation) {
        self.unknown.insert(projection);
        self.locations.insert(location.clone());
    }

    fn evidence_at(&self, projection: &[String]) -> (Evidence, Option<BranchCoverage>) {
        let mut coverage = Coverage::default();
        for (path, found) in &self.observed {
            if projection_matches(path, projection) {
                coverage.some |= found.some;
                coverage.every |= found.every;
            }
        }
        if coverage.some {
            return (Evidence::Observed, coverage.public());
        }
        if self
            .unknown
            .iter()
            .any(|path| projection_matches(path, projection))
        {
            return (Evidence::Unknown, None);
        }
        (Evidence::NoEvidence, None)
    }

    fn any_observed(&self) -> bool {
        !self.observed.is_empty()
    }

    fn any_unknown(&self) -> bool {
        !self.unknown.is_empty()
    }
}

fn projection_matches(mark: &[String], leaf: &[String]) -> bool {
    if mark.len() > leaf.len() {
        return false;
    }
    mark.iter().zip(leaf).all(|(a, b)| a == b || a == "[*]")
}

fn atom_prefix_matches(mark: &Atom, value: &Atom) -> bool {
    mark.origin == value.origin && projection_matches(&mark.projection, &value.projection)
}

#[derive(Debug, Clone)]
struct SubjectData {
    public: Subject,
    bt: BaseType,
    input: bool,
    output: bool,
    relevant: bool,
    content_relevant: bool,
    ownership: PathEvidence,
    value: PathEvidence,
    resource_contents: PathEvidence,
    post_value_use: PathEvidence,
    post_shape_use: PathEvidence,
    relation: PathEvidence,
    address_relation: PathEvidence,
}

impl SubjectData {
    fn new(public: Subject, bt: BaseType, input: bool, output: bool) -> Self {
        let content_relevant = bt != BaseType::Unit;
        Self {
            public,
            bt,
            input,
            output,
            relevant: input || output,
            content_relevant,
            ownership: PathEvidence::default(),
            value: PathEvidence::default(),
            resource_contents: PathEvidence::default(),
            post_value_use: PathEvidence::default(),
            post_shape_use: PathEvidence::default(),
            relation: PathEvidence::default(),
            address_relation: PathEvidence::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RelationKey {
    output: Atom,
    input: Atom,
    kind: RelationKind,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RelationLinkKey {
    left: Atom,
    right: Atom,
    kind: RelationKind,
    projectable: bool,
}

#[derive(Debug, Clone, Default)]
struct RelationEvidence {
    coverage: Coverage,
    locations: BTreeSet<IrLocation>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PredicateSummary {
    constrained: BTreeMap<Projection, Coverage>,
    unknown: BTreeSet<Projection>,
    intrinsic_content: Coverage,
    intrinsic_unknown: bool,
    value_uses: BTreeMap<(usize, Projection), Coverage>,
    shape_uses: BTreeMap<(usize, Projection), Coverage>,
    unknown_inputs: BTreeSet<(usize, Projection)>,
    unknown_shape_inputs: BTreeSet<(usize, Projection)>,
    relations: BTreeMap<(Projection, usize, Projection, RelationKind), Coverage>,
    projection_relations: BTreeMap<(Projection, usize, Projection), Coverage>,
    relation_unknown: BTreeSet<BTreeSet<PredicateEndpoint>>,
    address_relation_unknown: BTreeSet<BTreeSet<PredicateEndpoint>>,
    partial: bool,
    opaque: bool,
    parameter_count: usize,
    global_symbols: Vec<Sym>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PredicateEndpoint {
    Bound(Projection),
    Formal(usize, Projection),
}

const TERM_NODE_BUDGET: usize = 8_192;
const MAX_FIXPOINT_ROUNDS: usize = 16;

fn symbol_label(sym: &Sym) -> String {
    sym.name
        .clone()
        .unwrap_or_else(|| format!("symbol#{}", sym.num))
}

fn constructor_projection(sym: &Sym) -> String {
    match &sym.name {
        Some(name) => format!("<{name}#{}>", sym.num),
        None => format!("<symbol#{}>", sym.num),
    }
}

fn unknown_from_term(term: &IndexTerm, env: &Env) -> Flow {
    let mut out = Flow {
        incomplete: true,
        projection_lost: true,
        ..Flow::default()
    };
    for sym in term.free_vars() {
        if let Some(flow) = env.get(&sym) {
            out.mark_unknown_from(flow);
        }
    }
    out
}

fn substitute_flow(flow: &Flow, actuals: &[Flow], limit: usize) -> Flow {
    fn substitute_atom(atom: &Atom, actuals: &[Flow], limit: usize, unknown: bool) -> Flow {
        match atom.origin {
            Origin::Formal(index) => {
                let Some(actual) = actuals.get(index) else {
                    return Flow {
                        incomplete: true,
                        projection_lost: true,
                        ..Flow::default()
                    };
                };
                let mut projected = actual.project_path(&atom.projection, limit);
                if unknown {
                    let mut out = Flow::default();
                    out.mark_unknown_from(&projected);
                    projected = out;
                }
                projected
            }
            Origin::Subject(_) | Origin::InternalResource(_) => {
                let mut out = Flow::default();
                if unknown {
                    out.unknown.insert(atom.clone());
                } else {
                    out.direct.insert(atom.clone());
                }
                out
            }
        }
    }

    let mut out = Flow::default();
    for atom in &flow.direct {
        out.merge(&substitute_atom(atom, actuals, limit, false));
    }
    for atom in &flow.unknown {
        out.merge(&substitute_atom(atom, actuals, limit, true));
    }
    for atom in &flow.control {
        let replacement = substitute_atom(atom, actuals, limit, false);
        out.control.extend(replacement.atoms());
        out.conditional.extend(replacement.conditional_atoms());
        out.unknown.extend(replacement.unknown_atoms());
        out.incomplete |= replacement.incomplete;
    }
    for atom in &flow.shape_control {
        let replacement = substitute_atom(atom, actuals, limit, false);
        out.shape_control.extend(replacement.atoms());
        out.conditional.extend(replacement.conditional_atoms());
        out.unknown.extend(replacement.unknown_atoms());
        out.incomplete |= replacement.incomplete;
    }
    for atom in &flow.conditional {
        let replacement = substitute_atom(atom, actuals, limit, false);
        out.conditional.extend(replacement.atoms());
        out.conditional.extend(replacement.conditional_atoms());
        out.unknown.extend(replacement.unknown_atoms());
        out.incomplete |= replacement.incomplete;
    }
    for (field, child) in &flow.fields {
        out.fields
            .entry(field.clone())
            .or_default()
            .merge(&substitute_flow(child, actuals, limit));
    }
    out.authoritative_fields
        .extend(flow.authoritative_fields.iter().cloned());
    for (left, right) in &flow.exclusive {
        let left = substitute_atom(left, actuals, limit, false);
        let right = substitute_atom(right, actuals, limit, false);
        'pairs: for left_atom in left.atoms() {
            for right_atom in right.atoms() {
                if out.exclusive.len() >= MAX_CONDITION_RELATIONS {
                    out.incomplete = true;
                    break 'pairs;
                }
                out.exclusive.insert((left_atom.clone(), right_atom));
            }
        }
        out.unknown.extend(left.unknown_atoms());
        out.unknown.extend(right.unknown_atoms());
        out.incomplete |= left.incomplete || right.incomplete;
    }
    out.incomplete |= flow.incomplete;
    out.projection_lost |= flow.projection_lost;
    if let Some(condition) = &flow.condition {
        out.condition = Some(Box::new(substitute_condition(condition, actuals, limit)));
    }
    out
}

fn eval_index_term(
    term: &IndexTerm,
    env: &Env,
    logical: &BTreeMap<Sym, LogicalSummary>,
    limit: usize,
) -> Flow {
    let mut budget = TERM_NODE_BUDGET;
    eval_index_term_with_budget(term, env, logical, limit, &mut budget)
}

fn eval_index_term_with_budget(
    it: &IndexTerm,
    env: &Env,
    logical: &BTreeMap<Sym, LogicalSummary>,
    limit: usize,
    budget: &mut usize,
) -> Flow {
    if *budget == 0 {
        return unknown_from_term(it, env);
    }
    *budget -= 1;

    let child = |term: &IndexTerm, env: &Env, budget: &mut usize| {
        eval_index_term_with_budget(term, env, logical, limit, budget)
    };
    match &it.term {
        Term::Const(_)
        | Term::SizeOf(_)
        | Term::OffsetOf(_, _)
        | Term::Nil(_)
        | Term::CnNone(_) => Flow::default(),
        Term::Sym(sym) => env.get(sym).cloned().unwrap_or_else(|| Flow {
            incomplete: true,
            projection_lost: true,
            ..Flow::default()
        }),
        Term::Unop(_, value)
        | Term::HasAllocId(value)
        | Term::Representable(_, value)
        | Term::Good(_, value)
        | Term::WrapI(_, value)
        | Term::IsSome(value)
        | Term::Cast(_, value) => child(value, env, budget).transformed(),
        Term::NthTuple(index, value) => {
            child(value, env, budget).project(&format!("[{index}]"), limit)
        }
        Term::StructMember(value, field) | Term::RecordMember(value, field) => {
            child(value, env, budget).project(&field.0, limit)
        }
        Term::MemberShift(value, _, field) => child(value, env, budget).project(&field.0, limit),
        Term::Head(value) => child(value, env, budget).project("head", limit),
        Term::Tail(value) => child(value, env, budget).project("tail", limit),
        Term::MapConst(_, value) => Flow::field("[*]", child(value, env, budget)),
        Term::CnSome(value) => Flow::field("some", child(value, env, budget)),
        Term::GetOpt(value) => child(value, env, budget).project("some", limit),
        Term::Binop(_, left, right)
        | Term::CopyAllocId {
            addr: left,
            loc: right,
        }
        | Term::Aligned {
            t: left,
            align: right,
        } => Flow::union([child(left, env, budget), child(right, env, budget)]).transformed(),
        Term::Cons(head, tail) => {
            let mut out = Flow::default();
            out.fields.insert("head".into(), child(head, env, budget));
            out.fields.insert("tail".into(), child(tail, env, budget));
            out.authoritative_fields
                .extend(["head".into(), "tail".into()]);
            out
        }
        Term::ArrayShift { base, index, .. } => Flow::union([
            child(base, env, budget).project("[*]", limit),
            child(index, env, budget),
        ])
        .transformed(),
        Term::MapGet(map, key) => child(map, env, budget)
            .project("[*]", limit)
            .with_control(&child(key, env, budget)),
        Term::Ite(cond, then_, else_) => {
            let cond = child(cond, env, budget);
            let branches =
                Flow::alternatives([child(then_, env, budget), child(else_, env, budget)]);
            branches.with_control(&cond)
        }
        Term::Tuple(values) => {
            let mut out = Flow::default();
            for (index, value) in values.iter().enumerate() {
                let field = format!("[{index}]");
                out.fields.insert(field.clone(), child(value, env, budget));
                out.authoritative_fields.insert(field);
            }
            out
        }
        Term::Struct(_, fields) | Term::Record(fields) => {
            let mut out = Flow::default();
            for (field, value) in fields {
                out.fields
                    .insert(field.0.clone(), child(value, env, budget));
                out.authoritative_fields.insert(field.0.clone());
            }
            out
        }
        Term::Constructor(constructor, fields) => {
            let mut variant = Flow::default();
            for (field, value) in fields {
                variant
                    .fields
                    .insert(field.0.clone(), child(value, env, budget));
                variant.authoritative_fields.insert(field.0.clone());
            }
            Flow::field(constructor_projection(constructor), variant)
        }
        Term::StructUpdate {
            target,
            member,
            value,
        }
        | Term::RecordUpdate {
            target,
            member,
            value,
        } => {
            let mut out = child(target, env, budget);
            out.fields
                .insert(member.0.clone(), child(value, env, budget));
            out.authoritative_fields.insert(member.0.clone());
            out
        }
        Term::MapSet(map, key, value) => {
            let mut out = child(map, env, budget);
            let existing = out.project("[*]", limit);
            let value = child(value, env, budget);
            let wildcard =
                Flow::alternatives([existing, value]).with_control(&child(key, env, budget));
            out.fields.insert("[*]".into(), wildcard);
            out.authoritative_fields.insert("[*]".into());
            out
        }
        Term::EachI { var, body, .. } | Term::MapDef(var, body) => {
            let mut inner = env.clone();
            inner.insert(var.0.clone(), Flow::default());
            Flow::field("[*]", child(body, &inner, budget))
        }
        Term::Apply(name, args) => {
            let mut actuals: Vec<_> = args.iter().map(|arg| child(arg, env, budget)).collect();
            match logical.get(name) {
                Some(summary) => {
                    let globals = summary.global_symbols.iter().map(|symbol| {
                        env.get(symbol).cloned().unwrap_or_else(|| Flow {
                            incomplete: true,
                            ..Flow::default()
                        })
                    });
                    actuals.extend(globals);
                    if args.len() != summary.parameter_count {
                        return Flow::opaque_from(&actuals);
                    }
                    if summary.opaque {
                        Flow::opaque_from(&actuals)
                    } else {
                        let mut result = substitute_flow(&summary.result, &actuals, limit);
                        if let Some(condition) = &summary.condition {
                            result.condition =
                                Some(Box::new(substitute_condition(condition, &actuals, limit)));
                        }
                        result
                    }
                }
                None => Flow::opaque_from(&actuals),
            }
        }
        Term::Let((binder, value), body) => {
            let mut evaluated = child(value, env, budget);
            if value.bt == BaseType::Bool {
                evaluated.condition =
                    Some(Box::new(inspect_condition_term(value, env, logical, limit)));
            }
            let mut inner = env.clone();
            inner.insert(binder.clone(), evaluated);
            child(body, &inner, budget)
        }
        Term::Match(scrutinee, arms) => {
            let syntactically_known = matches!(&scrutinee.term, Term::Constructor(_, _));
            let scrutinee = child(scrutinee, env, budget);
            let known_scrutinee =
                syntactically_known || scrutinee.known_constructor_projection().is_some();
            if known_scrutinee {
                match select_known_match_arm(arms, &scrutinee, limit) {
                    KnownMatchArm::Selected(pattern, arm) => {
                        let mut inner = env.clone();
                        bind_pattern(pattern, &scrutinee, &mut inner, limit);
                        return child(arm, &inner, budget);
                    }
                    KnownMatchArm::NoMatch => {
                        return Flow {
                            incomplete: true,
                            ..Flow::default()
                        };
                    }
                    KnownMatchArm::Unknown => {}
                }
            }
            let mut branches = Vec::new();
            let mut needs_discriminator = false;
            for (pattern, arm) in arms {
                let pattern_match = known_scrutinee
                    .then(|| match_pattern_against_known_flow(pattern, &scrutinee, limit));
                if pattern_match == Some(PatternMatch::Mismatches) {
                    continue;
                }
                let mut inner = env.clone();
                bind_pattern(pattern, &scrutinee, &mut inner, limit);
                branches.push(child(arm, &inner, budget));
                if pattern_is_irrefutable(pattern) || pattern_match == Some(PatternMatch::Matches) {
                    break;
                }
                needs_discriminator = true;
            }
            let branches = Flow::alternatives(branches);
            if needs_discriminator {
                branches.with_shape_control(&scrutinee.discriminator_flow())
            } else {
                branches
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatternMatch {
    Matches,
    Mismatches,
    Unknown,
}

fn match_pattern_against_known_flow(pattern: &Pattern, value: &Flow, limit: usize) -> PatternMatch {
    match &pattern.pattern {
        PatternKind::Sym(_) | PatternKind::Wild => PatternMatch::Matches,
        PatternKind::Constructor(expected, fields) => {
            let expected = constructor_projection(expected);
            let Some(actual) = value.known_constructor_projection() else {
                return PatternMatch::Unknown;
            };
            if actual != expected {
                return PatternMatch::Mismatches;
            }

            let variant = value.project(&expected, limit);
            let mut result = PatternMatch::Matches;
            for (field, pattern) in fields {
                match match_pattern_against_known_flow(
                    pattern,
                    &variant.project(&field.0, limit),
                    limit,
                ) {
                    PatternMatch::Mismatches => return PatternMatch::Mismatches,
                    PatternMatch::Unknown => result = PatternMatch::Unknown,
                    PatternMatch::Matches => {}
                }
            }
            result
        }
    }
}

enum KnownMatchArm<'a> {
    Selected(&'a Pattern, &'a IndexTerm),
    NoMatch,
    Unknown,
}

fn select_known_match_arm<'a>(
    arms: &'a [(Pattern, IndexTerm)],
    value: &Flow,
    limit: usize,
) -> KnownMatchArm<'a> {
    for (pattern, arm) in arms {
        match match_pattern_against_known_flow(pattern, value, limit) {
            PatternMatch::Matches => return KnownMatchArm::Selected(pattern, arm),
            PatternMatch::Mismatches => {}
            PatternMatch::Unknown => return KnownMatchArm::Unknown,
        }
    }
    KnownMatchArm::NoMatch
}

fn pattern_is_irrefutable(pattern: &Pattern) -> bool {
    matches!(pattern.pattern, PatternKind::Sym(_) | PatternKind::Wild)
}

fn bind_pattern(pattern: &Pattern, value: &Flow, env: &mut Env, limit: usize) {
    match &pattern.pattern {
        PatternKind::Sym(sym) => {
            env.insert(sym.clone(), value.clone());
        }
        PatternKind::Wild => {}
        PatternKind::Constructor(constructor, fields) => {
            let variant = value.project(&constructor_projection(constructor), limit);
            for (field, pattern) in fields {
                bind_pattern(pattern, &variant.project(&field.0, limit), env, limit);
            }
        }
    }
}

fn logical_summaries(module: &SpecModule, limit: usize) -> BTreeMap<Sym, LogicalSummary> {
    let global_symbols: Vec<_> = module
        .globals
        .iter()
        .map(|global| global.address.clone())
        .collect();
    let mut summaries = BTreeMap::new();
    let recursive = recursive_logical_names(module);
    for def in &module.logical_functions {
        summaries
            .entry(def.name.clone())
            .or_insert_with(|| LogicalSummary {
                result: Flow::default(),
                truth: Truth::Unknown,
                total: false,
                opaque: def.body.is_none(),
                parameter_count: def.args.len(),
                global_symbols: global_symbols.clone(),
                condition: None,
            });
    }

    for _ in 0..MAX_FIXPOINT_ROUNDS {
        let previous = summaries.clone();
        let mut visited = BTreeSet::new();
        for def in &module.logical_functions {
            if !visited.insert(def.name.clone()) {
                continue;
            }
            let Some(current) = summaries.get(&def.name) else {
                continue;
            };
            if current.opaque {
                continue;
            }
            let Some(body) = &def.body else {
                continue;
            };
            let mut env: Env = global_symbols
                .iter()
                .enumerate()
                .map(|(index, sym)| {
                    (
                        sym.clone(),
                        Flow::atom(Atom::formal(def.args.len() + index)),
                    )
                })
                .collect();
            env.extend(
                def.args
                    .iter()
                    .enumerate()
                    .map(|(index, (sym, _))| (sym.clone(), Flow::atom(Atom::formal(index)))),
            );
            let mut result = eval_index_term(body.term(), &env, &previous, limit);
            let truth = truth_of(body.term(), &previous);
            let total = definitely_total(body.term(), &previous);
            let mut condition = (def.return_bt == BaseType::Bool)
                .then(|| inspect_condition_term(body.term(), &env, &previous, limit));
            if recursive.contains(&def.name) {
                let mut relation_group = Vec::new();
                for index in 0..def.args.len() + global_symbols.len() {
                    let atom = Atom::formal(index);
                    result.unknown.insert(atom.clone());
                    if let Some(condition) = &mut condition {
                        condition.unknown.insert(atom.clone());
                    }
                    relation_group.push(atom);
                }
                if let Some(condition) = &mut condition {
                    condition.mark_relation_unknown(relation_group, false);
                }
            }
            summaries.insert(
                def.name.clone(),
                LogicalSummary {
                    result,
                    truth,
                    total,
                    opaque: false,
                    parameter_count: def.args.len(),
                    global_symbols: global_symbols.clone(),
                    condition,
                },
            );
        }
        if summaries == previous {
            return summaries;
        }
    }

    let mut visited = BTreeSet::new();
    for def in &module.logical_functions {
        if !visited.insert(def.name.clone()) {
            continue;
        }
        let Some(summary) = summaries.get_mut(&def.name) else {
            continue;
        };
        let mut relation_group = Vec::new();
        for index in 0..def.args.len() + summary.global_symbols.len() {
            let atom = Atom::formal(index);
            summary.result.unknown.insert(atom.clone());
            if let Some(condition) = &mut summary.condition {
                condition.unknown.insert(atom.clone());
            }
            relation_group.push(atom);
        }
        if let Some(condition) = &mut summary.condition {
            condition.mark_relation_unknown(relation_group, false);
        }
    }
    summaries
}

fn logical_callees(term: &IndexTerm) -> Vec<Sym> {
    let mut callees = BTreeSet::new();
    crate::spec::convert::logical_functions::collect_applies(term, &mut callees);
    callees.into_iter().collect()
}

fn recursive_logical_names(module: &SpecModule) -> BTreeSet<Sym> {
    let mut graph = BTreeMap::<Sym, Vec<Sym>>::new();
    for definition in &module.logical_functions {
        graph.entry(definition.name.clone()).or_insert_with(|| {
            definition
                .body
                .as_ref()
                .map_or_else(Vec::new, |body| logical_callees(body.term()))
        });
    }
    crate::spec::convert::def::self_reaching(&graph)
}

fn truth_of(term: &IndexTerm, logical: &BTreeMap<Sym, LogicalSummary>) -> Truth {
    let mut budget = TERM_NODE_BUDGET;
    truth_of_with_budget(term, logical, &mut budget)
}

fn definitely_total(term: &IndexTerm, logical: &BTreeMap<Sym, LogicalSummary>) -> bool {
    let mut total = true;
    drop(term.clone().map_post(&mut |node| {
        match &node.term {
            Term::Binop(
                BinOp::Div
                | BinOp::Exp
                | BinOp::Rem
                | BinOp::Mod
                | BinOp::ShiftLeft
                | BinOp::ShiftRight,
                _,
                _,
            )
            | Term::Head(_)
            | Term::Tail(_)
            | Term::GetOpt(_)
            | Term::MapGet(_, _)
            | Term::Match(_, _) => total = false,
            Term::Apply(name, args)
                if !logical.get(name).is_some_and(|summary| {
                    !summary.opaque && summary.total && args.len() == summary.parameter_count
                }) =>
            {
                total = false;
            }
            _ => {}
        }
        node
    }));
    total
}

fn truth_of_with_budget(
    term: &IndexTerm,
    logical: &BTreeMap<Sym, LogicalSummary>,
    budget: &mut usize,
) -> Truth {
    if *budget == 0 {
        return Truth::Unknown;
    }
    *budget -= 1;
    let recurse =
        |term: &IndexTerm, budget: &mut usize| truth_of_with_budget(term, logical, budget);

    match &term.term {
        Term::Const(Const::Bool(true)) => Truth::AlwaysTrue,
        Term::Const(Const::Bool(false)) => Truth::AlwaysFalse,
        Term::Representable(Sctype::Void | Sctype::Byte | Sctype::Pointer(_), _)
        | Term::Good(Sctype::Void | Sctype::Byte, _) => Truth::AlwaysTrue,
        Term::Good(Sctype::Pointer(pointee), about)
            if matches!(pointee.as_ref(), Sctype::Void | Sctype::Function { .. })
                && definitely_total(about, logical) =>
        {
            Truth::AlwaysTrue
        }
        Term::Unop(UnOp::Not, value) => match recurse(value, budget) {
            Truth::AlwaysTrue | Truth::AlwaysFalse if !definitely_total(value, logical) => {
                Truth::Unknown
            }
            truth => truth.not(),
        },
        Term::Binop(BinOp::Eq | BinOp::Le | BinOp::LePointer, left, right)
            if left == right && definitely_total(left, logical) =>
        {
            Truth::AlwaysTrue
        }
        Term::Binop(BinOp::Implies, left, right)
            if left == right && definitely_total(left, logical) =>
        {
            Truth::AlwaysTrue
        }
        Term::Binop(BinOp::And, left, right) => {
            let left_truth = recurse(left, budget);
            let right_truth = recurse(right, budget);
            let left_total = definitely_total(left, logical);
            let right_total = definitely_total(right, logical);
            match (left_truth, right_truth) {
                (Truth::AlwaysFalse, _) if left_total => Truth::AlwaysFalse,
                (_, Truth::AlwaysFalse) if left_total && right_total => Truth::AlwaysFalse,
                (Truth::AlwaysTrue, Truth::AlwaysTrue) if left_total && right_total => {
                    Truth::AlwaysTrue
                }
                (Truth::Unknown, _) | (_, Truth::Unknown) => Truth::Unknown,
                (Truth::AlwaysFalse, _) | (_, Truth::AlwaysFalse) => Truth::Unknown,
                _ => Truth::Nontrivial,
            }
        }
        Term::Binop(BinOp::Or, left, right) => {
            if (matches!(&right.term, Term::Unop(UnOp::Not, inner) if inner.as_ref() == left.as_ref())
                || matches!(&left.term, Term::Unop(UnOp::Not, inner) if inner.as_ref() == right.as_ref()))
                && definitely_total(left, logical)
                && definitely_total(right, logical)
            {
                return Truth::AlwaysTrue;
            }
            let left_truth = recurse(left, budget);
            let right_truth = recurse(right, budget);
            let left_total = definitely_total(left, logical);
            let right_total = definitely_total(right, logical);
            match (left_truth, right_truth) {
                (Truth::AlwaysTrue, _) if left_total => Truth::AlwaysTrue,
                (_, Truth::AlwaysTrue) if left_total && right_total => Truth::AlwaysTrue,
                (Truth::AlwaysFalse, Truth::AlwaysFalse) if left_total && right_total => {
                    Truth::AlwaysFalse
                }
                (Truth::Unknown, _) | (_, Truth::Unknown) => Truth::Unknown,
                (Truth::AlwaysTrue, _) | (_, Truth::AlwaysTrue) => Truth::Unknown,
                _ => Truth::Nontrivial,
            }
        }
        Term::Binop(BinOp::Implies, left, right) => {
            let left_truth = recurse(left, budget);
            let right_truth = recurse(right, budget);
            let left_total = definitely_total(left, logical);
            let right_total = definitely_total(right, logical);
            match (left_truth, right_truth) {
                (Truth::AlwaysFalse, _) if left_total => Truth::AlwaysTrue,
                (_, Truth::AlwaysTrue) if left_total && right_total => Truth::AlwaysTrue,
                (Truth::AlwaysTrue, value) if left_total => value,
                (value, Truth::AlwaysFalse) if left_total && right_total => value.not(),
                (Truth::Unknown, _) | (_, Truth::Unknown) => Truth::Unknown,
                (Truth::AlwaysFalse, _)
                | (_, Truth::AlwaysTrue)
                | (Truth::AlwaysTrue, _)
                | (_, Truth::AlwaysFalse) => Truth::Unknown,
                _ => Truth::Nontrivial,
            }
        }
        Term::Ite(cond, then_term, else_term) => match recurse(cond, budget) {
            Truth::AlwaysTrue if definitely_total(cond, logical) => recurse(then_term, budget),
            Truth::AlwaysFalse if definitely_total(cond, logical) => recurse(else_term, budget),
            _ => {
                let then_truth = recurse(then_term, budget);
                let else_truth = recurse(else_term, budget);
                if then_truth == else_truth
                    && definitely_total(cond, logical)
                    && definitely_total(then_term, logical)
                    && definitely_total(else_term, logical)
                {
                    then_truth
                } else if matches!(then_truth, Truth::Unknown)
                    || matches!(else_truth, Truth::Unknown)
                {
                    Truth::Unknown
                } else {
                    Truth::Nontrivial
                }
            }
        },
        Term::Apply(name, args) => logical
            .get(name)
            .filter(|summary| {
                !summary.opaque
                    && args.len() == summary.parameter_count
                    && args.iter().all(|arg| definitely_total(arg, logical))
            })
            .map_or(Truth::Unknown, |summary| match summary.truth {
                Truth::AlwaysTrue | Truth::AlwaysFalse if !summary.total => Truth::Unknown,
                truth => truth,
            }),
        Term::Let((_, value), body) if definitely_total(value, logical) => recurse(body, budget),
        Term::Cast(BaseType::Bool, value) if definitely_total(value, logical) => {
            recurse(value, budget)
        }
        _ if term.bt == BaseType::Bool => Truth::Nontrivial,
        _ => Truth::Unknown,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ConditionEvidence {
    uses: BTreeMap<Atom, Coverage>,
    shape_uses: BTreeMap<Atom, Coverage>,
    content_shape_uses: BTreeMap<Atom, Coverage>,
    content_uses: BTreeMap<Atom, Coverage>,
    unknown: BTreeSet<Atom>,
    shape_unknown: BTreeSet<Atom>,
    content_shape_unknown: BTreeSet<Atom>,
    content_unknown: BTreeSet<Atom>,
    relation_unknown: BTreeSet<BTreeSet<Atom>>,
    address_relation_unknown: BTreeSet<BTreeSet<Atom>>,
    relations: BTreeMap<(Atom, Atom, RelationKind), Coverage>,
    projection_relations: BTreeMap<(Atom, Atom), Coverage>,
    incomplete: bool,
}

impl ConditionEvidence {
    fn observe(&mut self, atom: Atom, coverage: Coverage) {
        merge_coverage(self.uses.entry(atom).or_default(), coverage);
    }

    fn observe_shape(&mut self, atom: Atom, coverage: Coverage) {
        merge_coverage(self.shape_uses.entry(atom).or_default(), coverage);
    }

    fn observe_content_shape(&mut self, atom: Atom, coverage: Coverage) {
        self.observe_shape(atom.clone(), coverage);
        merge_coverage(self.content_shape_uses.entry(atom).or_default(), coverage);
    }

    fn observe_content(&mut self, atom: Atom, coverage: Coverage) {
        merge_coverage(self.content_uses.entry(atom).or_default(), coverage);
    }

    fn mark_relation_unknown(&mut self, atoms: impl IntoIterator<Item = Atom>, address_only: bool) {
        let group: BTreeSet<_> = atoms.into_iter().collect();
        if group.is_empty() {
            return;
        }
        if address_only {
            self.address_relation_unknown.insert(group);
        } else {
            self.relation_unknown.insert(group);
        }
    }

    fn relate(&mut self, left: Atom, right: Atom, kind: RelationKind, coverage: Coverage) {
        if left == right {
            return;
        }
        if self.relations.len() >= MAX_CONDITION_RELATIONS
            && !self
                .relations
                .contains_key(&(left.clone(), right.clone(), kind))
        {
            self.unknown.insert(Atom {
                origin: left.origin.clone(),
                projection: Vec::new(),
            });
            self.unknown.insert(Atom {
                origin: right.origin.clone(),
                projection: Vec::new(),
            });
            self.mark_relation_unknown([left, right], kind == RelationKind::AddressOnly);
            self.incomplete = true;
            return;
        }
        merge_coverage(
            self.relations
                .entry((left.clone(), right.clone(), kind))
                .or_default(),
            coverage,
        );
        merge_coverage(
            self.relations.entry((right, left, kind)).or_default(),
            coverage,
        );
    }

    fn relate_projection(&mut self, left: Atom, right: Atom, coverage: Coverage) {
        if left == right {
            return;
        }
        if self.projection_relations.len() >= MAX_CONDITION_RELATIONS
            && !self
                .projection_relations
                .contains_key(&(left.clone(), right.clone()))
        {
            self.mark_relation_unknown([left, right], false);
            self.incomplete = true;
            return;
        }
        merge_coverage(
            self.projection_relations
                .entry((left.clone(), right.clone()))
                .or_default(),
            coverage,
        );
        merge_coverage(
            self.projection_relations.entry((right, left)).or_default(),
            coverage,
        );
    }

    fn merge(&mut self, other: &Self) {
        for (atom, coverage) in &other.uses {
            merge_coverage(self.uses.entry(atom.clone()).or_default(), *coverage);
        }
        for (atom, coverage) in &other.shape_uses {
            merge_coverage(self.shape_uses.entry(atom.clone()).or_default(), *coverage);
        }
        for (atom, coverage) in &other.content_shape_uses {
            merge_coverage(
                self.content_shape_uses.entry(atom.clone()).or_default(),
                *coverage,
            );
        }
        for (atom, coverage) in &other.content_uses {
            merge_coverage(
                self.content_uses.entry(atom.clone()).or_default(),
                *coverage,
            );
        }
        self.unknown.extend(other.unknown.iter().cloned());
        self.shape_unknown
            .extend(other.shape_unknown.iter().cloned());
        self.content_shape_unknown
            .extend(other.content_shape_unknown.iter().cloned());
        self.content_unknown
            .extend(other.content_unknown.iter().cloned());
        self.relation_unknown
            .extend(other.relation_unknown.iter().cloned());
        self.address_relation_unknown
            .extend(other.address_relation_unknown.iter().cloned());
        self.incomplete |= other.incomplete;
        for (edge, coverage) in &other.relations {
            merge_coverage(self.relations.entry(edge.clone()).or_default(), *coverage);
        }
        for (edge, coverage) in &other.projection_relations {
            merge_coverage(
                self.projection_relations.entry(edge.clone()).or_default(),
                *coverage,
            );
        }
    }

    fn scaled(mut self, outer: Coverage) -> Self {
        for coverage in self.uses.values_mut() {
            *coverage = scale_coverage(*coverage, outer);
        }
        for coverage in self.shape_uses.values_mut() {
            *coverage = scale_coverage(*coverage, outer);
        }
        for coverage in self.content_shape_uses.values_mut() {
            *coverage = scale_coverage(*coverage, outer);
        }
        for coverage in self.content_uses.values_mut() {
            *coverage = scale_coverage(*coverage, outer);
        }
        for coverage in self.relations.values_mut() {
            *coverage = scale_coverage(*coverage, outer);
        }
        for coverage in self.projection_relations.values_mut() {
            *coverage = scale_coverage(*coverage, outer);
        }
        self
    }

    fn alternatives(left: Self, right: Self) -> Self {
        fn join_map<K: Ord + Clone>(
            left: &BTreeMap<K, Coverage>,
            right: &BTreeMap<K, Coverage>,
        ) -> BTreeMap<K, Coverage> {
            let keys: BTreeSet<_> = left.keys().chain(right.keys()).cloned().collect();
            keys.into_iter()
                .filter_map(|key| {
                    let a = left.get(&key).copied().unwrap_or_default();
                    let b = right.get(&key).copied().unwrap_or_default();
                    let joined = a.merge_alternative(b);
                    joined.some.then_some((key, joined))
                })
                .collect()
        }

        Self {
            uses: join_map(&left.uses, &right.uses),
            shape_uses: join_map(&left.shape_uses, &right.shape_uses),
            content_shape_uses: join_map(&left.content_shape_uses, &right.content_shape_uses),
            content_uses: join_map(&left.content_uses, &right.content_uses),
            unknown: left.unknown.union(&right.unknown).cloned().collect(),
            shape_unknown: left
                .shape_unknown
                .union(&right.shape_unknown)
                .cloned()
                .collect(),
            content_shape_unknown: left
                .content_shape_unknown
                .union(&right.content_shape_unknown)
                .cloned()
                .collect(),
            content_unknown: left
                .content_unknown
                .union(&right.content_unknown)
                .cloned()
                .collect(),
            relation_unknown: left
                .relation_unknown
                .union(&right.relation_unknown)
                .cloned()
                .collect(),
            address_relation_unknown: left
                .address_relation_unknown
                .union(&right.address_relation_unknown)
                .cloned()
                .collect(),
            relations: join_map(&left.relations, &right.relations),
            projection_relations: join_map(&left.projection_relations, &right.projection_relations),
            incomplete: left.incomplete || right.incomplete,
        }
    }
}

fn merge_coverage(target: &mut Coverage, source: Coverage) {
    target.some |= source.some;
    target.every |= source.every;
}

fn scale_coverage(inner: Coverage, outer: Coverage) -> Coverage {
    Coverage {
        some: inner.some && outer.some,
        every: inner.every && outer.every,
    }
}

fn inspect_constraint(
    constraint: &LogicalConstraint,
    env: &Env,
    logical: &BTreeMap<Sym, LogicalSummary>,
    limit: usize,
) -> ConditionEvidence {
    match constraint {
        LogicalConstraint::T(term) => inspect_condition_term(term, env, logical, limit),
        LogicalConstraint::Forall {
            var,
            permission,
            body,
        } => {
            let mut inner = env.clone();
            inner.insert(var.0.clone(), Flow::default());
            let permission = eval_index_term(permission, &inner, logical, limit);
            let mut evidence =
                inspect_condition_term(body, &inner, logical, limit).scaled(Coverage::SOME);
            for atom in permission.atoms() {
                evidence.observe_shape(atom, Coverage::EVERY);
            }
            evidence.shape_unknown.extend(permission.unknown_atoms());
            evidence.incomplete |= permission.incomplete;
            evidence
        }
    }
}

fn logical_call_actuals(
    args: &[IndexTerm],
    summary: &LogicalSummary,
    env: &Env,
    logical: &BTreeMap<Sym, LogicalSummary>,
    limit: usize,
) -> Option<Vec<Flow>> {
    if args.len() != summary.parameter_count {
        return None;
    }
    let mut actuals: Vec<_> = args
        .iter()
        .map(|arg| eval_index_term(arg, env, logical, limit))
        .collect();
    actuals.extend(summary.global_symbols.iter().map(|symbol| {
        env.get(symbol).cloned().unwrap_or_else(|| Flow {
            incomplete: true,
            ..Flow::default()
        })
    }));
    Some(actuals)
}

fn substitute_condition(
    condition: &ConditionEvidence,
    actuals: &[Flow],
    limit: usize,
) -> ConditionEvidence {
    let substitute = |atom: &Atom| substitute_flow(&Flow::atom(atom.clone()), actuals, limit);
    let mut out = ConditionEvidence {
        incomplete: condition.incomplete,
        ..ConditionEvidence::default()
    };
    for (atom, coverage) in &condition.uses {
        let flow = substitute(atom);
        let conditional = flow.conditional_atoms();
        let data = flow.data_atoms();
        for atom in &data {
            out.observe(atom.clone(), coverage.narrow(conditional.contains(atom)));
        }
        for control in &flow.control {
            out.observe(control.clone(), scale_coverage(*coverage, Coverage::SOME));
            for value in &data {
                if flow.atoms_are_exclusive(control, value) {
                    continue;
                }
                out.relate(
                    control.clone(),
                    value.clone(),
                    RelationKind::Control,
                    scale_coverage(*coverage, Coverage::SOME),
                );
            }
        }
        for control in &flow.shape_control {
            out.observe(control.clone(), scale_coverage(*coverage, Coverage::SOME));
            out.observe_shape(control.clone(), scale_coverage(*coverage, Coverage::SOME));
            for value in &data {
                if flow.atoms_are_exclusive(control, value) {
                    continue;
                }
                out.relate(
                    control.clone(),
                    value.clone(),
                    RelationKind::Control,
                    scale_coverage(*coverage, Coverage::SOME),
                );
            }
        }
        out.unknown.extend(flow.unknown_atoms());
        out.incomplete |= flow.incomplete;
    }
    for (atom, coverage) in &condition.shape_uses {
        let flow = substitute(atom);
        let conditional = flow.conditional_atoms();
        let data = flow.data_atoms();
        for atom in &data {
            out.observe_shape(atom.clone(), coverage.narrow(conditional.contains(atom)));
        }
        for control in &flow.control {
            out.observe(control.clone(), scale_coverage(*coverage, Coverage::SOME));
            for value in &data {
                if flow.atoms_are_exclusive(control, value) {
                    continue;
                }
                out.relate(
                    control.clone(),
                    value.clone(),
                    RelationKind::AddressOnly,
                    scale_coverage(*coverage, Coverage::SOME),
                );
            }
        }
        for control in &flow.shape_control {
            out.observe_shape(control.clone(), scale_coverage(*coverage, Coverage::SOME));
            for value in &data {
                if flow.atoms_are_exclusive(control, value) {
                    continue;
                }
                out.relate(
                    control.clone(),
                    value.clone(),
                    RelationKind::AddressOnly,
                    scale_coverage(*coverage, Coverage::SOME),
                );
            }
        }
        out.shape_unknown.extend(flow.unknown_atoms());
        if flow.incomplete || !flow.unknown_atoms().is_empty() {
            out.mark_relation_unknown(data.into_iter().chain(flow.unknown_atoms()), true);
        }
        out.incomplete |= flow.incomplete;
    }
    for (atom, coverage) in &condition.content_shape_uses {
        let flow = substitute(atom);
        let conditional = flow.conditional_atoms();
        for atom in flow.atoms() {
            out.observe_content_shape(atom.clone(), coverage.narrow(conditional.contains(&atom)));
        }
        out.content_shape_unknown.extend(flow.unknown_atoms());
        out.incomplete |= flow.incomplete;
    }
    for (atom, coverage) in &condition.content_uses {
        let flow = substitute(atom);
        for atom in flow.data_atoms() {
            out.observe_content(atom, *coverage);
        }
        out.content_unknown.extend(flow.unknown_atoms());
        out.incomplete |= flow.incomplete;
    }
    for atom in &condition.unknown {
        let flow = substitute(atom);
        out.unknown.extend(flow.atoms());
        out.unknown.extend(flow.unknown_atoms());
        out.incomplete |= flow.incomplete;
    }
    for atom in &condition.shape_unknown {
        let flow = substitute(atom);
        out.shape_unknown.extend(flow.atoms());
        out.shape_unknown.extend(flow.unknown_atoms());
        out.incomplete |= flow.incomplete;
    }
    for atom in &condition.content_shape_unknown {
        let flow = substitute(atom);
        out.content_shape_unknown.extend(flow.atoms());
        out.content_shape_unknown.extend(flow.unknown_atoms());
        out.incomplete |= flow.incomplete;
    }
    for atom in &condition.content_unknown {
        let flow = substitute(atom);
        out.content_unknown.extend(flow.atoms());
        out.content_unknown.extend(flow.unknown_atoms());
        out.incomplete |= flow.incomplete;
    }
    for group in &condition.relation_unknown {
        let mut substituted = BTreeSet::new();
        for atom in group {
            let flow = substitute(atom);
            substituted.extend(flow.atoms());
            substituted.extend(flow.unknown_atoms());
            out.incomplete |= flow.incomplete;
        }
        out.mark_relation_unknown(substituted, false);
    }
    for group in &condition.address_relation_unknown {
        let mut substituted = BTreeSet::new();
        for atom in group {
            let flow = substitute(atom);
            substituted.extend(flow.atoms());
            substituted.extend(flow.unknown_atoms());
            out.incomplete |= flow.incomplete;
        }
        out.mark_relation_unknown(substituted, true);
    }
    for ((left, right, kind), coverage) in &condition.relations {
        if *kind == RelationKind::Value
            && (condition
                .projection_relations
                .contains_key(&(left.clone(), right.clone()))
                || condition
                    .projection_relations
                    .contains_key(&(right.clone(), left.clone())))
        {
            continue;
        }
        let left = substitute(left);
        let right = substitute(right);
        let left_conditional = left.conditional_atoms();
        let right_conditional = right.conditional_atoms();
        let left_data = left.data_atoms();
        let right_data = right.data_atoms();
        for left_atom in &left_data {
            for right_atom in &right_data {
                if left.atoms_are_exclusive(left_atom, right_atom)
                    || right.atoms_are_exclusive(left_atom, right_atom)
                {
                    continue;
                }
                let coverage = coverage.narrow(
                    left_conditional.contains(left_atom) || right_conditional.contains(right_atom),
                );
                out.relate(left_atom.clone(), right_atom.clone(), *kind, coverage);
            }
        }
        for control in &left.control {
            for value in &right_data {
                if left.atoms_are_exclusive(control, value)
                    || right.atoms_are_exclusive(control, value)
                {
                    continue;
                }
                out.relate(
                    control.clone(),
                    value.clone(),
                    compose_relation_kind(*kind, RelationKind::Control),
                    scale_coverage(*coverage, Coverage::SOME),
                );
            }
        }
        for control in &left.shape_control {
            for value in &right_data {
                if left.atoms_are_exclusive(control, value)
                    || right.atoms_are_exclusive(control, value)
                {
                    continue;
                }
                out.relate(
                    control.clone(),
                    value.clone(),
                    compose_relation_kind(*kind, RelationKind::Control),
                    scale_coverage(*coverage, Coverage::SOME),
                );
            }
        }
        for control in &right.control {
            for value in &left_data {
                if left.atoms_are_exclusive(control, value)
                    || right.atoms_are_exclusive(control, value)
                {
                    continue;
                }
                out.relate(
                    control.clone(),
                    value.clone(),
                    compose_relation_kind(*kind, RelationKind::Control),
                    scale_coverage(*coverage, Coverage::SOME),
                );
            }
        }
        for control in &right.shape_control {
            for value in &left_data {
                if left.atoms_are_exclusive(control, value)
                    || right.atoms_are_exclusive(control, value)
                {
                    continue;
                }
                out.relate(
                    control.clone(),
                    value.clone(),
                    compose_relation_kind(*kind, RelationKind::Control),
                    scale_coverage(*coverage, Coverage::SOME),
                );
            }
        }
        out.unknown.extend(left.unknown_atoms());
        out.unknown.extend(right.unknown_atoms());
        if left.incomplete || right.incomplete {
            out.mark_relation_unknown(
                left_data.into_iter().chain(right_data),
                *kind == RelationKind::AddressOnly,
            );
        }
        out.incomplete |= left.incomplete || right.incomplete;
    }
    for ((left, right), coverage) in &condition.projection_relations {
        let left = substitute(left);
        let right = substitute(right);
        if !left.projection_preserving() || !right.projection_preserving() {
            if left.fields.is_empty() && right.fields.is_empty() {
                let left_conditional = left.conditional_atoms();
                let right_conditional = right.conditional_atoms();
                for left_atom in left.data_atoms() {
                    for right_atom in right.data_atoms() {
                        if left.atoms_are_exclusive(&left_atom, &right_atom)
                            || right.atoms_are_exclusive(&left_atom, &right_atom)
                        {
                            continue;
                        }
                        out.relate(
                            left_atom.clone(),
                            right_atom.clone(),
                            RelationKind::Value,
                            coverage.narrow(
                                left_conditional.contains(&left_atom)
                                    || right_conditional.contains(&right_atom),
                            ),
                        );
                    }
                }
                out.unknown.extend(left.unknown_atoms());
                out.unknown.extend(right.unknown_atoms());
                out.incomplete |= left.incomplete || right.incomplete;
                continue;
            }
            out.mark_relation_unknown(
                left.atoms()
                    .into_iter()
                    .chain(right.atoms())
                    .chain(left.unknown_atoms())
                    .chain(right.unknown_atoms()),
                false,
            );
            out.incomplete = true;
            continue;
        }
        let left_conditional = left.conditional_atoms();
        let right_conditional = right.conditional_atoms();
        for left_atom in left.data_atoms() {
            for right_atom in right.data_atoms() {
                if left.atoms_are_exclusive(&left_atom, &right_atom)
                    || right.atoms_are_exclusive(&left_atom, &right_atom)
                {
                    continue;
                }
                out.relate_projection(
                    left_atom.clone(),
                    right_atom.clone(),
                    coverage.narrow(
                        left_conditional.contains(&left_atom)
                            || right_conditional.contains(&right_atom),
                    ),
                );
            }
        }
    }
    out
}

fn inspect_condition_term(
    term: &IndexTerm,
    env: &Env,
    logical: &BTreeMap<Sym, LogicalSummary>,
    limit: usize,
) -> ConditionEvidence {
    if truth_of(term, logical) == Truth::AlwaysTrue {
        return ConditionEvidence::default();
    }
    if let Term::Sym(sym) = &term.term
        && let Some(condition) = env.get(sym).and_then(|flow| flow.condition.as_deref())
    {
        return condition.clone();
    }
    if let Term::Apply(name, args) = &term.term
        && let Some(summary) = logical.get(name).filter(|summary| !summary.opaque)
        && let Some(condition) = &summary.condition
    {
        let Some(actuals) = logical_call_actuals(args, summary, env, logical, limit) else {
            let mut evidence = atomic_condition(term, env, logical, limit);
            evidence.incomplete = true;
            return evidence;
        };
        return substitute_condition(condition, &actuals, limit);
    }
    match &term.term {
        Term::Binop(BinOp::And, left, right) => {
            let mut out = inspect_condition_term(left, env, logical, limit);
            out.merge(&inspect_condition_term(right, env, logical, limit));
            out
        }
        Term::Binop(BinOp::Or, left, right) => ConditionEvidence::alternatives(
            inspect_condition_term(left, env, logical, limit),
            inspect_condition_term(right, env, logical, limit),
        ),
        Term::Binop(BinOp::Eq, left, right)
            if matches!(left.term, Term::Const(Const::Bool(_))) && right.bt == BaseType::Bool =>
        {
            inspect_condition_term(right, env, logical, limit)
        }
        Term::Binop(BinOp::Eq, left, right)
            if matches!(right.term, Term::Const(Const::Bool(_))) && left.bt == BaseType::Bool =>
        {
            inspect_condition_term(left, env, logical, limit)
        }
        Term::Binop(BinOp::Implies, guard, body) => {
            let guard_flow = eval_index_term(guard, env, logical, limit);
            let mut guard_evidence = inspect_condition_term(guard, env, logical, limit);
            let guard_kind = condition_control_kind(&guard_evidence);
            let mut body_evidence =
                inspect_condition_term(body, env, logical, limit).scaled(Coverage::SOME);
            add_control_relations(&guard_flow, &mut body_evidence, Coverage::SOME, guard_kind);
            guard_evidence.merge(&body_evidence);
            guard_evidence
        }
        Term::Ite(guard, then_, else_) => {
            let guard_flow = eval_index_term(guard, env, logical, limit);
            let guard_evidence = inspect_condition_term(guard, env, logical, limit);
            let guard_kind = condition_control_kind(&guard_evidence);
            let mut then_evidence = inspect_condition_term(then_, env, logical, limit);
            let mut else_evidence = inspect_condition_term(else_, env, logical, limit);
            add_control_relations(&guard_flow, &mut then_evidence, Coverage::SOME, guard_kind);
            add_control_relations(&guard_flow, &mut else_evidence, Coverage::SOME, guard_kind);
            let mut out = guard_evidence;
            out.merge(&ConditionEvidence::alternatives(
                then_evidence,
                else_evidence,
            ));
            out
        }
        Term::Let((binder, value), body) => {
            let mut evaluated = eval_index_term(value, env, logical, limit);
            if value.bt == BaseType::Bool {
                evaluated.condition =
                    Some(Box::new(inspect_condition_term(value, env, logical, limit)));
            }
            let mut inner = env.clone();
            inner.insert(binder.clone(), evaluated);
            inspect_condition_term(body, &inner, logical, limit)
        }
        Term::Match(scrutinee_term, arms) => {
            let syntactically_known = matches!(&scrutinee_term.term, Term::Constructor(_, _));
            let scrutinee = eval_index_term(scrutinee_term, env, logical, limit);
            let known_scrutinee =
                syntactically_known || scrutinee.known_constructor_projection().is_some();
            if known_scrutinee {
                match select_known_match_arm(arms, &scrutinee, limit) {
                    KnownMatchArm::Selected(pattern, arm) => {
                        let mut inner = env.clone();
                        bind_pattern(pattern, &scrutinee, &mut inner, limit);
                        return inspect_condition_term(arm, &inner, logical, limit);
                    }
                    KnownMatchArm::NoMatch => {
                        return ConditionEvidence {
                            incomplete: true,
                            ..ConditionEvidence::default()
                        };
                    }
                    KnownMatchArm::Unknown => {}
                }
            }

            let discriminator = scrutinee.discriminator_flow();
            let mut joined = None;
            let needs_discriminator = arms
                .iter()
                .find(|(pattern, _)| {
                    !known_scrutinee
                        || match_pattern_against_known_flow(pattern, &scrutinee, limit)
                            != PatternMatch::Mismatches
                })
                .is_some_and(|(pattern, _)| !pattern_is_irrefutable(pattern));
            for (pattern, arm) in arms {
                let pattern_match = known_scrutinee
                    .then(|| match_pattern_against_known_flow(pattern, &scrutinee, limit));
                if pattern_match == Some(PatternMatch::Mismatches) {
                    continue;
                }
                let mut inner = env.clone();
                bind_pattern(pattern, &scrutinee, &mut inner, limit);
                let mut arm_evidence = inspect_condition_term(arm, &inner, logical, limit);
                if needs_discriminator {
                    add_control_relations(
                        &discriminator,
                        &mut arm_evidence,
                        Coverage::SOME,
                        RelationKind::Control,
                    );
                }
                joined = Some(match joined {
                    None => arm_evidence,
                    Some(previous) => ConditionEvidence::alternatives(previous, arm_evidence),
                });
                if pattern_is_irrefutable(pattern) || pattern_match == Some(PatternMatch::Matches) {
                    break;
                }
            }
            let mut out = joined.unwrap_or_else(|| ConditionEvidence {
                incomplete: true,
                ..ConditionEvidence::default()
            });
            if needs_discriminator {
                let conditional = discriminator.conditional_atoms();
                for atom in discriminator.atoms() {
                    let coverage = Coverage::branch(conditional.contains(&atom));
                    out.observe(atom.clone(), coverage);
                    out.observe_shape(atom, coverage);
                }
                out.unknown.extend(discriminator.unknown_atoms());
                out.incomplete |= discriminator.incomplete;
            }
            out
        }
        Term::Unop(UnOp::Not, inner) => inspect_condition_term(inner, env, logical, limit),
        _ => atomic_condition(term, env, logical, limit),
    }
}

fn atomic_condition(
    term: &IndexTerm,
    env: &Env,
    logical: &BTreeMap<Sym, LogicalSummary>,
    limit: usize,
) -> ConditionEvidence {
    let flow = eval_index_term(term, env, logical, limit);
    let mut out = ConditionEvidence {
        incomplete: flow.incomplete,
        ..ConditionEvidence::default()
    };
    let address_only = is_address_condition(term);
    let structural_value = is_structural_value_condition(term);
    match truth_of(term, logical) {
        Truth::AlwaysTrue => return out,
        Truth::Unknown => {
            if address_only {
                out.shape_unknown.extend(flow.unknown_atoms());
                out.content_shape_unknown.extend(flow.unknown_atoms());
            } else {
                out.unknown.extend(flow.unknown_atoms());
            }
            if term.bt != BaseType::Bool || flow.data_atoms().is_empty() {
                if address_only {
                    out.shape_unknown.extend(flow.atoms());
                    out.content_shape_unknown.extend(flow.atoms());
                } else {
                    out.unknown.extend(flow.atoms());
                }
                out.mark_relation_unknown(
                    flow.atoms().into_iter().chain(flow.unknown_atoms()),
                    address_only,
                );
                return out;
            }
        }
        Truth::AlwaysFalse | Truth::Nontrivial => {}
    }

    let aligned_equality = aggregate_equality_flows(term, env, logical, limit);
    let (data, conditional): (Vec<_>, BTreeSet<_>) = if let Some(aligned) = &aligned_equality {
        let mut data = BTreeSet::new();
        let mut conditional = BTreeSet::new();
        for (left, right) in &aligned.pairs {
            data.extend(left.data_atoms());
            data.extend(right.data_atoms());
            conditional.extend(left.conditional_atoms());
            conditional.extend(right.conditional_atoms());
        }
        out.unknown.extend(aligned.unknown.iter().cloned());
        (data.into_iter().collect(), conditional)
    } else {
        (
            flow.data_atoms().into_iter().collect(),
            flow.conditional_atoms(),
        )
    };
    for atom in &data {
        let coverage = Coverage::branch(conditional.contains(atom));
        if address_only {
            out.observe_content_shape(atom.clone(), coverage);
        } else {
            out.observe(atom.clone(), coverage);
            if structural_value {
                out.observe_shape(atom.clone(), coverage);
            }
        }
    }
    if address_only {
        out.shape_unknown.extend(flow.unknown_atoms());
        out.content_shape_unknown.extend(flow.unknown_atoms());
    } else {
        out.unknown.extend(flow.unknown_atoms());
    }
    if let Some(aligned) = &aligned_equality {
        for (left_flow, right_flow) in &aligned.pairs {
            let left_conditional = left_flow.conditional_atoms();
            let right_conditional = right_flow.conditional_atoms();
            for left in left_flow.data_atoms() {
                for right in right_flow.data_atoms() {
                    if left_flow.atoms_are_exclusive(&left, &right)
                        || right_flow.atoms_are_exclusive(&left, &right)
                    {
                        continue;
                    }
                    let coverage = Coverage::branch(
                        left_conditional.contains(&left) || right_conditional.contains(&right),
                    );
                    out.relate(
                        left.clone(),
                        right.clone(),
                        if address_only {
                            RelationKind::AddressOnly
                        } else {
                            RelationKind::Value
                        },
                        coverage,
                    );
                    if !address_only
                        && left_flow.projection_preserving()
                        && right_flow.projection_preserving()
                    {
                        out.relate_projection(left.clone(), right, coverage);
                    }
                }
            }
        }
        if !aligned.unknown.is_empty() {
            out.mark_relation_unknown(aligned.unknown.iter().cloned(), address_only);
            out.incomplete = true;
        }
    } else {
        for (index, left) in data.iter().enumerate() {
            for right in data.iter().skip(index + 1) {
                if flow.atoms_are_exclusive(left, right) {
                    continue;
                }
                let coverage =
                    Coverage::branch(conditional.contains(left) || conditional.contains(right));
                out.relate(
                    left.clone(),
                    right.clone(),
                    if address_only {
                        RelationKind::AddressOnly
                    } else {
                        RelationKind::Value
                    },
                    coverage,
                );
            }
        }
        if !address_only && let Term::Binop(BinOp::Eq, left_term, right_term) = &term.term {
            let left_flow = eval_index_term(left_term, env, logical, limit);
            let right_flow = eval_index_term(right_term, env, logical, limit);
            if left_flow.projection_preserving() && right_flow.projection_preserving() {
                let left = left_flow
                    .direct
                    .iter()
                    .next()
                    .expect("projection-preserving flow has one origin")
                    .clone();
                let right = right_flow
                    .direct
                    .iter()
                    .next()
                    .expect("projection-preserving flow has one origin")
                    .clone();
                out.relate_projection(left, right, Coverage::EVERY);
            }
        }
    }
    for control in &flow.control {
        let coverage = Coverage::branch(conditional.contains(control));
        if address_only {
            out.observe_content_shape(control.clone(), coverage);
        } else {
            out.observe(control.clone(), coverage);
            if structural_value {
                out.observe_shape(control.clone(), coverage);
            }
        }
        for value in &data {
            if flow.atoms_are_exclusive(control, value) {
                continue;
            }
            let coverage =
                Coverage::branch(conditional.contains(control) || conditional.contains(value));
            out.relate(
                control.clone(),
                value.clone(),
                if address_only {
                    RelationKind::AddressOnly
                } else {
                    RelationKind::Control
                },
                coverage,
            );
        }
    }
    for control in &flow.shape_control {
        let coverage = Coverage::branch(conditional.contains(control));
        if address_only {
            out.observe_content_shape(control.clone(), coverage);
        } else {
            out.observe(control.clone(), coverage);
            out.observe_shape(control.clone(), coverage);
        }
        for value in &data {
            if flow.atoms_are_exclusive(control, value) {
                continue;
            }
            let coverage =
                Coverage::branch(conditional.contains(control) || conditional.contains(value));
            out.relate(
                control.clone(),
                value.clone(),
                if address_only {
                    RelationKind::AddressOnly
                } else {
                    RelationKind::Control
                },
                coverage,
            );
        }
    }
    if !flow.unknown_atoms().is_empty() || flow.incomplete {
        out.mark_relation_unknown(data.into_iter().chain(flow.unknown_atoms()), address_only);
    }
    out
}

struct AlignedEquality {
    pairs: Vec<(Flow, Flow)>,
    unknown: BTreeSet<Atom>,
}

fn aggregate_equality_flows(
    term: &IndexTerm,
    env: &Env,
    logical: &BTreeMap<Sym, LogicalSummary>,
    limit: usize,
) -> Option<AlignedEquality> {
    let Term::Binop(BinOp::Eq, left, right) = &term.term else {
        return None;
    };
    if !is_aggregate_base_type(&left.bt) && !is_aggregate_base_type(&right.bt) {
        return None;
    }
    let mut static_paths = BTreeSet::new();
    let statically_complete = collect_static_value_paths(
        if is_aggregate_base_type(&left.bt) {
            &left.bt
        } else {
            &right.bt
        },
        &mut Vec::new(),
        &mut static_paths,
        limit,
    );
    let left = eval_index_term(left, env, logical, limit);
    let right = eval_index_term(right, env, logical, limit);
    let mut paths = BTreeSet::new();
    collect_carrier_paths(&left, &mut Vec::new(), &mut paths);
    collect_carrier_paths(&right, &mut Vec::new(), &mut paths);
    paths.extend(static_paths);
    if !left.fields.is_empty() || !right.fields.is_empty() {
        paths.remove(&Vec::new());
    }
    let mut unknown = BTreeSet::new();
    if !statically_complete
        && ((!left.direct.is_empty() && !left.fields.is_empty())
            || (!right.direct.is_empty() && !right.fields.is_empty()))
    {
        unknown.extend(left.direct.iter().cloned());
        unknown.extend(right.direct.iter().cloned());
    }
    Some(AlignedEquality {
        pairs: paths
            .into_iter()
            .map(|path| {
                (
                    left.project_path(&path, limit),
                    right.project_path(&path, limit),
                )
            })
            .collect(),
        unknown,
    })
}

fn collect_static_value_paths(
    bt: &BaseType,
    prefix: &mut Projection,
    out: &mut BTreeSet<Projection>,
    remaining: usize,
) -> bool {
    if remaining == 0 {
        return false;
    }
    match bt {
        BaseType::Record(fields) => {
            let mut complete = true;
            for (field, field_bt) in fields {
                prefix.push(field.0.clone());
                complete &= collect_static_value_paths(field_bt, prefix, out, remaining - 1);
                prefix.pop();
            }
            complete
        }
        BaseType::Tuple(fields) => {
            let mut complete = true;
            for (index, field_bt) in fields.iter().enumerate() {
                prefix.push(format!("[{index}]"));
                complete &= collect_static_value_paths(field_bt, prefix, out, remaining - 1);
                prefix.pop();
            }
            complete
        }
        BaseType::Struct(_)
        | BaseType::Datatype(_)
        | BaseType::Map(_, _)
        | BaseType::List(_)
        | BaseType::Set(_)
        | BaseType::Option(_) => false,
        _ => {
            out.insert(prefix.clone());
            true
        }
    }
}

fn collect_carrier_paths(flow: &Flow, prefix: &mut Projection, out: &mut BTreeSet<Projection>) {
    if flow.fields.is_empty() {
        if !prefix.is_empty() || !flow.data_atoms().is_empty() {
            out.insert(prefix.clone());
        }
        return;
    }
    for (field, child) in &flow.fields {
        prefix.push(field.clone());
        collect_carrier_paths(child, prefix, out);
        prefix.pop();
    }
}

fn is_aggregate_base_type(bt: &BaseType) -> bool {
    matches!(
        bt,
        BaseType::Struct(_)
            | BaseType::Datatype(_)
            | BaseType::Record(_)
            | BaseType::Map(_, _)
            | BaseType::List(_)
            | BaseType::Tuple(_)
            | BaseType::Set(_)
            | BaseType::Option(_)
    )
}

fn is_address_condition(term: &IndexTerm) -> bool {
    match &term.term {
        Term::HasAllocId(_) | Term::Aligned { .. } | Term::Binop(BinOp::LePointer, _, _) => true,
        Term::Representable(Sctype::Pointer(_), _) | Term::Good(Sctype::Pointer(_), _) => true,
        Term::Binop(BinOp::Eq | BinOp::Lt | BinOp::Le, left, right) => {
            matches!(left.bt, BaseType::Loc(_)) && matches!(right.bt, BaseType::Loc(_))
        }
        _ => false,
    }
}

fn is_structural_value_condition(term: &IndexTerm) -> bool {
    matches!(term.term, Term::IsSome(_))
}

fn condition_control_kind(evidence: &ConditionEvidence) -> RelationKind {
    if evidence.uses.is_empty() && !evidence.shape_uses.is_empty() {
        RelationKind::AddressOnly
    } else {
        RelationKind::Control
    }
}

fn add_control_relations(
    guard: &Flow,
    body: &mut ConditionEvidence,
    coverage: Coverage,
    guard_kind: RelationKind,
) {
    let guards = guard.atoms();
    let unknown_guards = guard.unknown_atoms();
    let values: Vec<_> = body.uses.keys().cloned().collect();
    let shapes: Vec<_> = body.shape_uses.keys().cloned().collect();
    for guard in guards {
        for value in &values {
            body.relate(guard.clone(), value.clone(), guard_kind, coverage);
        }
        for value in &shapes {
            body.relate(
                guard.clone(),
                value.clone(),
                compose_relation_kind(guard_kind, RelationKind::AddressOnly),
                coverage,
            );
        }
    }
    if !unknown_guards.is_empty() {
        body.mark_relation_unknown(
            unknown_guards.iter().cloned().chain(values),
            guard_kind == RelationKind::AddressOnly,
        );
        body.mark_relation_unknown(unknown_guards.iter().cloned().chain(shapes), true);
    }
    body.unknown.extend(unknown_guards);
    body.incomplete |= guard.incomplete;
}

fn predicate_summaries(
    module: &SpecModule,
    logical: &BTreeMap<Sym, LogicalSummary>,
    limit: usize,
) -> BTreeMap<Sym, PredicateSummary> {
    let global_symbols: Vec<_> = module
        .globals
        .iter()
        .map(|global| global.address.clone())
        .collect();
    let mut summaries = BTreeMap::new();
    let recursive = recursive_predicate_names(module);
    for predicate in &module.predicates {
        summaries
            .entry(predicate.name.clone())
            .or_insert_with(|| PredicateSummary {
                opaque: !matches!(predicate.catch_all_clauses(), Some(Ok(_))),
                parameter_count: predicate.generator_iargs().len(),
                global_symbols: global_symbols.clone(),
                ..PredicateSummary::default()
            });
    }

    for _ in 0..MAX_FIXPOINT_ROUNDS {
        let previous = summaries.clone();
        let mut visited = BTreeSet::new();
        for predicate in &module.predicates {
            if !visited.insert(predicate.name.clone()) {
                continue;
            }
            let Some(current) = summaries.get(&predicate.name) else {
                continue;
            };
            if current.opaque {
                continue;
            }
            let Some(Ok(clauses)) = predicate.catch_all_clauses() else {
                continue;
            };
            let mut summary = summarize_predicate(
                predicate,
                clauses,
                &previous,
                logical,
                &global_symbols,
                limit,
            );
            if recursive.contains(&predicate.name) && !summary.opaque {
                summary.unknown.insert(Vec::new());
                let mut relation_group = BTreeSet::from([PredicateEndpoint::Bound(Vec::new())]);
                for formal in 0..summary.parameter_count + summary.global_symbols.len() {
                    summary.unknown_inputs.insert((formal, Vec::new()));
                    summary.unknown_shape_inputs.insert((formal, Vec::new()));
                    relation_group.insert(PredicateEndpoint::Formal(formal, Vec::new()));
                }
                summary.relation_unknown.insert(relation_group);
            }
            summaries.insert(predicate.name.clone(), summary);
        }
        if summaries == previous {
            return summaries;
        }
    }

    for summary in summaries.values_mut() {
        summary.unknown.insert(Vec::new());
        let mut relation_group = BTreeSet::from([PredicateEndpoint::Bound(Vec::new())]);
        for formal in 0..summary.parameter_count + summary.global_symbols.len() {
            summary.unknown_inputs.insert((formal, Vec::new()));
            summary.unknown_shape_inputs.insert((formal, Vec::new()));
            relation_group.insert(PredicateEndpoint::Formal(formal, Vec::new()));
        }
        summary.relation_unknown.insert(relation_group);
    }
    summaries
}

fn recursive_predicate_names(module: &SpecModule) -> BTreeSet<Sym> {
    crate::spec::convert::def::recursive_predicates(module)
}

fn summarize_predicate(
    predicate: &PredicateDef,
    clauses: &[crate::spec::Clause],
    predicates: &BTreeMap<Sym, PredicateSummary>,
    logical: &BTreeMap<Sym, LogicalSummary>,
    global_symbols: &[Sym],
    limit: usize,
) -> PredicateSummary {
    let generator_iargs = predicate.generator_iargs();
    let mut base_env: Env = global_symbols
        .iter()
        .enumerate()
        .map(|(index, sym)| {
            (
                sym.clone(),
                Flow::atom(Atom::formal(generator_iargs.len() + index)),
            )
        })
        .collect();
    base_env.extend(
        generator_iargs
            .iter()
            .enumerate()
            .map(|(index, (sym, _))| (sym.clone(), Flow::atom(Atom::formal(index)))),
    );

    let mut joined: Option<PredicateSummary> = None;
    let mut preceding_guards = Flow::default();
    let mut preceding_guard_evidence = ConditionEvidence::default();
    for (clause_index, clause) in clauses.iter().enumerate() {
        let guard_truth = truth_of(&clause.guard, logical);
        if guard_truth == Truth::AlwaysFalse {
            continue;
        }
        let guard = eval_index_term(&clause.guard, &base_env, logical, limit);
        let all_guards = Flow::union([preceding_guards.clone(), guard.clone()]);
        let mut env = base_env.clone();
        let guard_evidence =
            inspect_condition_term(&clause.guard, &base_env, logical, limit).scaled(Coverage::SOME);
        let mut evidence = preceding_guard_evidence.clone();
        evidence.merge(&guard_evidence);

        let mut terminal = walk_predicate_lat(
            &clause.packing_ft,
            &mut env,
            &mut evidence,
            predicates,
            logical,
            global_symbols,
            limit,
            clause_index,
        );
        let clause_controls = all_guards.atoms();
        terminal.control.extend(clause_controls.iter().cloned());
        terminal.conditional.extend(clause_controls);
        terminal.unknown.extend(all_guards.unknown_atoms());
        terminal.incomplete |= all_guards.incomplete;
        let clause_summary = summarize_predicate_clause(&terminal, &evidence, limit);
        joined = Some(match joined {
            None => clause_summary,
            Some(previous) => join_predicate_alternatives(previous, clause_summary),
        });
        if guard_truth == Truth::AlwaysTrue {
            break;
        }
        preceding_guards.merge(&guard);
        preceding_guard_evidence.merge(&guard_evidence);
    }
    let mut summary = joined.unwrap_or_else(|| PredicateSummary {
        opaque: true,
        unknown: BTreeSet::from([Vec::new()]),
        ..PredicateSummary::default()
    });
    summary.parameter_count = generator_iargs.len();
    summary.global_symbols = global_symbols.to_vec();
    summary
}

#[allow(clippy::too_many_arguments)]
fn walk_predicate_lat(
    lat: &Lat<IndexTerm>,
    env: &mut Env,
    evidence: &mut ConditionEvidence,
    predicates: &BTreeMap<Sym, PredicateSummary>,
    logical: &BTreeMap<Sym, LogicalSummary>,
    global_symbols: &[Sym],
    limit: usize,
    clause_index: usize,
) -> Flow {
    let mut node = lat;
    let mut position = 0usize;
    loop {
        match node {
            Lat::Define((binder, value), rest) => {
                let mut evaluated = eval_index_term(value, env, logical, limit);
                if value.bt == BaseType::Bool {
                    evaluated.condition =
                        Some(Box::new(inspect_condition_term(value, env, logical, limit)));
                }
                env.insert(binder.clone(), evaluated);
                node = rest;
            }
            Lat::Resource((binder, (request, _)), rest) => {
                let internal = Flow::atom(Atom::internal(
                    clause_index.saturating_mul(1_000_000) + position,
                ));
                inspect_summary_request(
                    request,
                    &internal,
                    env,
                    evidence,
                    predicates,
                    logical,
                    global_symbols,
                    limit,
                );
                env.insert(binder.clone(), internal);
                node = rest;
            }
            Lat::Constraint(constraint, rest) => {
                evidence.merge(&inspect_constraint(constraint, env, logical, limit));
                node = rest;
            }
            Lat::I(terminal) => return eval_index_term(terminal, env, logical, limit),
        }
        position += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn inspect_summary_request(
    request: &Request,
    bound: &Flow,
    env: &Env,
    evidence: &mut ConditionEvidence,
    predicates: &BTreeMap<Sym, PredicateSummary>,
    logical: &BTreeMap<Sym, LogicalSummary>,
    global_symbols: &[Sym],
    limit: usize,
) {
    let (pointer, iargs, permission) = request_flows(request, env, logical, limit);
    for flow in std::iter::once(&pointer)
        .chain(iargs.iter())
        .chain(permission.iter())
    {
        let conditional = flow.conditional_atoms();
        for atom in flow.atoms() {
            evidence.observe_shape(atom.clone(), Coverage::branch(conditional.contains(&atom)));
        }
        evidence.incomplete |= flow.incomplete;
    }
    evidence.shape_unknown.extend(pointer.unknown_atoms());
    for flow in iargs.iter().chain(permission.iter()) {
        evidence.shape_unknown.extend(flow.unknown_atoms());
    }

    let RequestName::PName(name) = request.name() else {
        return;
    };
    let mut actuals = vec![pointer];
    actuals.extend(iargs);
    match predicates.get(name) {
        Some(summary) if !summary.opaque => {
            apply_predicate_summary(summary, bound, &actuals, env, evidence, limit)
        }
        Some(summary) => {
            evidence.unknown.extend(bound.atoms());
            let mut relation_group = bound.atoms();
            for formal in 0..summary.parameter_count + summary.global_symbols.len() {
                if let Some(actual) = predicate_actual(summary, formal, &actuals, env) {
                    evidence.unknown.extend(actual.atoms());
                    evidence.unknown.extend(actual.unknown_atoms());
                    relation_group.extend(actual.atoms());
                    relation_group.extend(actual.unknown_atoms());
                }
            }
            evidence.mark_relation_unknown(relation_group, false);
        }
        None => {
            evidence.unknown.extend(bound.atoms());
            let mut relation_group = bound.atoms();
            for actual in &actuals {
                evidence.unknown.extend(actual.atoms());
                evidence.unknown.extend(actual.unknown_atoms());
                relation_group.extend(actual.atoms());
                relation_group.extend(actual.unknown_atoms());
            }
            for symbol in global_symbols {
                if let Some(global) = env.get(symbol) {
                    evidence.unknown.extend(global.atoms());
                    evidence.unknown.extend(global.unknown_atoms());
                    relation_group.extend(global.atoms());
                    relation_group.extend(global.unknown_atoms());
                }
            }
            evidence.mark_relation_unknown(relation_group, false);
        }
    }
}

fn request_flows(
    request: &Request,
    env: &Env,
    logical: &BTreeMap<Sym, LogicalSummary>,
    limit: usize,
) -> (Flow, Vec<Flow>, Vec<Flow>) {
    match request {
        Request::P { pointer, iargs, .. } => (
            eval_index_term(pointer, env, logical, limit),
            iargs
                .iter()
                .map(|arg| eval_index_term(arg, env, logical, limit))
                .collect(),
            Vec::new(),
        ),
        Request::Q {
            pointer,
            q,
            permission,
            iargs,
            ..
        } => {
            let pointer = eval_index_term(pointer, env, logical, limit);
            let mut inner = env.clone();
            inner.insert(q.0.clone(), Flow::default());
            let iargs = iargs
                .iter()
                .map(|arg| eval_index_term(arg, &inner, logical, limit))
                .collect();
            let permission = vec![eval_index_term(permission, &inner, logical, limit)];
            (pointer, iargs, permission)
        }
    }
}

fn apply_predicate_summary(
    summary: &PredicateSummary,
    bound: &Flow,
    actuals: &[Flow],
    env: &Env,
    evidence: &mut ConditionEvidence,
    limit: usize,
) {
    let Some(bound_atom) = bound.direct.iter().next() else {
        evidence.incomplete = true;
        return;
    };
    evidence.incomplete |= summary.partial;
    if actuals.len() != summary.parameter_count {
        evidence.incomplete = true;
        evidence.unknown.insert(bound_atom.clone());
        for actual in actuals {
            evidence.unknown.extend(actual.atoms());
            evidence.unknown.extend(actual.unknown_atoms());
        }
        return;
    }
    if summary.intrinsic_content.some {
        evidence.observe_content(bound_atom.clone(), summary.intrinsic_content);
    }
    if summary.intrinsic_unknown {
        evidence.content_unknown.insert(bound_atom.clone());
    }
    for ((formal, path), coverage) in &summary.value_uses {
        let Some(actual) = predicate_actual(summary, *formal, actuals, env) else {
            evidence.incomplete = true;
            continue;
        };
        let actual = actual.project_path(path, limit);
        let conditional = actual.conditional_atoms();
        for atom in actual
            .data_atoms()
            .into_iter()
            .chain(actual.control.iter().cloned())
        {
            evidence.observe(atom.clone(), coverage.narrow(conditional.contains(&atom)));
        }
        for atom in &actual.shape_control {
            evidence.observe(atom.clone(), scale_coverage(*coverage, Coverage::SOME));
            evidence.observe_shape(atom.clone(), scale_coverage(*coverage, Coverage::SOME));
        }
        evidence.unknown.extend(actual.unknown_atoms());
    }
    for ((formal, path), coverage) in &summary.shape_uses {
        let Some(actual) = predicate_actual(summary, *formal, actuals, env) else {
            evidence.incomplete = true;
            continue;
        };
        let actual = actual.project_path(path, limit);
        let conditional = actual.conditional_atoms();
        for atom in actual
            .data_atoms()
            .into_iter()
            .chain(actual.shape_control.iter().cloned())
        {
            evidence.observe_shape(atom.clone(), coverage.narrow(conditional.contains(&atom)));
        }
        for atom in &actual.control {
            evidence.observe_shape(atom.clone(), scale_coverage(*coverage, Coverage::SOME));
        }
        evidence.shape_unknown.extend(actual.unknown_atoms());
    }
    for (formal, path) in &summary.unknown_inputs {
        let Some(actual) = predicate_actual(summary, *formal, actuals, env) else {
            evidence.incomplete = true;
            continue;
        };
        let actual = actual.project_path(path, limit);
        evidence.unknown.extend(actual.atoms());
        evidence.unknown.extend(actual.unknown_atoms());
    }
    for (formal, path) in &summary.unknown_shape_inputs {
        let Some(actual) = predicate_actual(summary, *formal, actuals, env) else {
            evidence.incomplete = true;
            continue;
        };
        let actual = actual.project_path(path, limit);
        evidence.shape_unknown.extend(actual.atoms());
        evidence.shape_unknown.extend(actual.unknown_atoms());
    }
    for (path, coverage) in &summary.constrained {
        match bound_atom.project_path(path, limit) {
            Ok(atom) => evidence.observe(atom, *coverage),
            Err(atom) => {
                evidence.unknown.insert(atom);
            }
        }
    }
    for path in &summary.unknown {
        match bound_atom.project_path(path, limit) {
            Ok(atom) | Err(atom) => {
                evidence.unknown.insert(atom);
            }
        }
    }
    for group in &summary.relation_unknown {
        let (atoms, incomplete) =
            substitute_predicate_unknown_group(summary, group, bound_atom, actuals, env, limit);
        evidence.mark_relation_unknown(atoms, false);
        evidence.incomplete |= incomplete;
    }
    for group in &summary.address_relation_unknown {
        let (atoms, incomplete) =
            substitute_predicate_unknown_group(summary, group, bound_atom, actuals, env, limit);
        evidence.mark_relation_unknown(atoms, true);
        evidence.incomplete |= incomplete;
    }
    for ((output_path, formal, formal_path, kind), coverage) in &summary.relations {
        if *kind == RelationKind::Value
            && summary.projection_relations.contains_key(&(
                output_path.clone(),
                *formal,
                formal_path.clone(),
            ))
        {
            continue;
        }
        let output = match bound_atom.project_path(output_path, limit) {
            Ok(atom) => atom,
            Err(atom) => {
                evidence.mark_relation_unknown([atom], *kind == RelationKind::AddressOnly);
                evidence.incomplete = true;
                continue;
            }
        };
        let Some(actual) = predicate_actual(summary, *formal, actuals, env) else {
            evidence.mark_relation_unknown([output], *kind == RelationKind::AddressOnly);
            evidence.incomplete = true;
            continue;
        };
        let actual = actual.project_path(formal_path, limit);
        let conditional = actual.conditional_atoms();
        for input in actual.data_atoms() {
            let relation_coverage = coverage.narrow(conditional.contains(&input));
            evidence.relate(output.clone(), input, *kind, relation_coverage);
        }
        for input in &actual.control {
            evidence.relate(
                output.clone(),
                input.clone(),
                compose_relation_kind(*kind, RelationKind::Control),
                scale_coverage(*coverage, Coverage::SOME),
            );
        }
        for input in &actual.shape_control {
            evidence.relate(
                output.clone(),
                input.clone(),
                compose_relation_kind(*kind, RelationKind::Control),
                scale_coverage(*coverage, Coverage::SOME),
            );
        }
        let unknown = actual.unknown_atoms();
        if !unknown.is_empty() || actual.incomplete {
            evidence.mark_relation_unknown(
                std::iter::once(output).chain(unknown),
                *kind == RelationKind::AddressOnly,
            );
            evidence.incomplete |= actual.incomplete;
        }
    }
    for ((output_path, formal, formal_path), coverage) in &summary.projection_relations {
        let output = match bound_atom.project_path(output_path, limit) {
            Ok(atom) => atom,
            Err(atom) => {
                evidence.mark_relation_unknown([atom], false);
                evidence.incomplete = true;
                continue;
            }
        };
        let Some(actual) = predicate_actual(summary, *formal, actuals, env) else {
            evidence.mark_relation_unknown([output], false);
            evidence.incomplete = true;
            continue;
        };
        let actual = actual.project_path(formal_path, limit);
        if !actual.projection_preserving() {
            if actual.fields.is_empty() {
                let conditional = actual.conditional_atoms();
                for input in actual.data_atoms() {
                    evidence.relate(
                        output.clone(),
                        input.clone(),
                        RelationKind::Value,
                        coverage.narrow(conditional.contains(&input)),
                    );
                }
                for input in actual.control.iter().chain(&actual.shape_control) {
                    evidence.relate(
                        output.clone(),
                        input.clone(),
                        RelationKind::Control,
                        scale_coverage(*coverage, Coverage::SOME),
                    );
                }
                evidence.unknown.extend(actual.unknown_atoms());
                evidence.incomplete |= actual.incomplete;
                continue;
            }
            evidence.mark_relation_unknown(
                std::iter::once(output)
                    .chain(actual.atoms())
                    .chain(actual.unknown_atoms()),
                false,
            );
            evidence.incomplete = true;
            continue;
        }
        let conditional = actual.conditional_atoms();
        for input in actual.data_atoms() {
            evidence.relate_projection(
                output.clone(),
                input.clone(),
                coverage.narrow(conditional.contains(&input)),
            );
        }
    }
}

fn predicate_actual(
    summary: &PredicateSummary,
    formal: usize,
    actuals: &[Flow],
    env: &Env,
) -> Option<Flow> {
    if formal < summary.parameter_count {
        return actuals.get(formal).cloned();
    }
    summary
        .global_symbols
        .get(formal - summary.parameter_count)
        .and_then(|symbol| env.get(symbol))
        .cloned()
}

fn substitute_predicate_unknown_group(
    summary: &PredicateSummary,
    group: &BTreeSet<PredicateEndpoint>,
    bound: &Atom,
    actuals: &[Flow],
    env: &Env,
    limit: usize,
) -> (BTreeSet<Atom>, bool) {
    let mut atoms = BTreeSet::new();
    let mut incomplete = false;
    for endpoint in group {
        match endpoint {
            PredicateEndpoint::Bound(path) => match bound.project_path(path, limit) {
                Ok(atom) => {
                    atoms.insert(atom);
                }
                Err(atom) => {
                    atoms.insert(atom);
                    incomplete = true;
                }
            },
            PredicateEndpoint::Formal(formal, path) => {
                let Some(actual) = predicate_actual(summary, *formal, actuals, env) else {
                    incomplete = true;
                    continue;
                };
                let projected = actual.project_path(path, limit);
                atoms.extend(projected.atoms());
                atoms.extend(projected.unknown_atoms());
                incomplete |= projected.incomplete;
            }
        }
    }
    (atoms, incomplete)
}

fn summarize_predicate_clause(
    terminal: &Flow,
    evidence: &ConditionEvidence,
    limit: usize,
) -> PredicateSummary {
    let mut mappings = Vec::new();
    terminal.mappings(&mut Vec::new(), &mut mappings);
    let mut unknown_mappings = Vec::new();
    terminal.unknown_mappings(&mut Vec::new(), &mut unknown_mappings);
    let mut summary = PredicateSummary::default();
    let terminal_conditional = terminal.conditional_atoms();
    if terminal.incomplete || evidence.incomplete {
        summary.partial = true;
    }

    for (constrained, coverage) in &evidence.uses {
        if let Origin::Formal(formal) = constrained.origin {
            merge_coverage(
                summary
                    .value_uses
                    .entry((formal, constrained.projection.clone()))
                    .or_default(),
                *coverage,
            );
        }
        if matches!(constrained.origin, Origin::InternalResource(_)) {
            merge_coverage(&mut summary.intrinsic_content, *coverage);
        }
        let paths = map_internal_to_output(constrained, &mappings, limit);
        for path in paths {
            if summary.constrained.len() >= MAX_CONDITION_RELATIONS
                && !summary.constrained.contains_key(&path)
            {
                summary.unknown.insert(Vec::new());
                break;
            }
            merge_coverage(summary.constrained.entry(path).or_default(), *coverage);
        }
    }
    for (content, coverage) in &evidence.content_uses {
        if matches!(content.origin, Origin::InternalResource(_)) {
            merge_coverage(&mut summary.intrinsic_content, *coverage);
        }
    }

    for (used, coverage) in &evidence.shape_uses {
        if let Origin::Formal(formal) = used.origin {
            merge_coverage(
                summary
                    .shape_uses
                    .entry((formal, used.projection.clone()))
                    .or_default(),
                *coverage,
            );
        }
    }
    for (used, coverage) in &evidence.content_shape_uses {
        if !matches!(used.origin, Origin::InternalResource(_)) {
            continue;
        }
        merge_coverage(&mut summary.intrinsic_content, *coverage);
        for path in map_internal_to_output(used, &mappings, limit) {
            if summary.constrained.len() >= MAX_CONDITION_RELATIONS
                && !summary.constrained.contains_key(&path)
            {
                summary.unknown.insert(Vec::new());
                break;
            }
            merge_coverage(summary.constrained.entry(path).or_default(), *coverage);
        }
    }
    for unknown in &evidence.shape_unknown {
        if let Origin::Formal(formal) = unknown.origin {
            summary
                .unknown_shape_inputs
                .insert((formal, unknown.projection.clone()));
        }
    }
    if evidence
        .content_shape_unknown
        .iter()
        .any(|atom| matches!(atom.origin, Origin::InternalResource(_)))
    {
        summary.intrinsic_unknown = true;
    }

    for unknown in evidence
        .unknown
        .iter()
        .chain(unknown_mappings.iter().map(|(_, atom)| atom))
    {
        if let Origin::Formal(formal) = unknown.origin {
            summary
                .unknown_inputs
                .insert((formal, unknown.projection.clone()));
        }
        if matches!(unknown.origin, Origin::InternalResource(_)) {
            summary.intrinsic_unknown = true;
        }
        let paths = map_internal_to_output(unknown, &mappings, limit);
        if paths.is_empty()
            && mappings
                .iter()
                .any(|(_, source)| source.origin == unknown.origin)
        {
            summary.unknown.insert(Vec::new());
        } else {
            summary.unknown.extend(paths);
        }
    }
    if evidence
        .content_unknown
        .iter()
        .any(|atom| matches!(atom.origin, Origin::InternalResource(_)))
    {
        summary.intrinsic_unknown = true;
    }
    for group in &evidence.relation_unknown {
        let translated = predicate_unknown_group(group, &mappings, limit);
        if !translated.is_empty() {
            summary.relation_unknown.insert(translated);
        }
    }
    for group in &evidence.address_relation_unknown {
        let translated = predicate_unknown_group(group, &mappings, limit);
        if !translated.is_empty() {
            let observes_loaded_content = group.iter().any(|atom| {
                matches!(atom.origin, Origin::InternalResource(_))
                    && (evidence.content_shape_uses.keys().any(|used| {
                        atom_prefix_matches(used, atom) || atom_prefix_matches(atom, used)
                    }) || evidence.content_shape_unknown.iter().any(|unknown| {
                        atom_prefix_matches(unknown, atom) || atom_prefix_matches(atom, unknown)
                    }))
            });
            if observes_loaded_content {
                summary.relation_unknown.insert(translated);
            } else {
                summary.address_relation_unknown.insert(translated);
            }
        }
    }

    for (output_path, source) in &mappings {
        let Origin::Formal(formal) = &source.origin else {
            continue;
        };
        let key = (
            output_path.clone(),
            *formal,
            source.projection.clone(),
            RelationKind::Value,
        );
        if summary.relations.len() >= MAX_CONDITION_RELATIONS
            && !summary.relations.contains_key(&key)
        {
            summary.unknown.insert(output_path.clone());
            continue;
        }
        merge_coverage(
            summary.relations.entry(key).or_default(),
            Coverage::branch(terminal_conditional.contains(source)),
        );
    }
    for control in &terminal.control {
        let Origin::Formal(formal) = &control.origin else {
            continue;
        };
        let output_paths: Vec<_> = if mappings.is_empty() {
            vec![Vec::new()]
        } else {
            mappings.iter().map(|(path, _)| path.clone()).collect()
        };
        for output_path in output_paths {
            let kind = if *formal == 0 {
                RelationKind::AddressOnly
            } else {
                RelationKind::Control
            };
            let key = (
                output_path.clone(),
                *formal,
                control.projection.clone(),
                kind,
            );
            if summary.relations.len() >= MAX_CONDITION_RELATIONS
                && !summary.relations.contains_key(&key)
            {
                summary.unknown.insert(output_path);
                continue;
            }
            merge_coverage(
                summary.relations.entry(key).or_default(),
                Coverage::branch(terminal_conditional.contains(control)),
            );
        }
    }
    for control in &terminal.shape_control {
        let Origin::Formal(formal) = &control.origin else {
            continue;
        };
        merge_coverage(
            summary
                .shape_uses
                .entry((*formal, control.projection.clone()))
                .or_default(),
            Coverage::SOME,
        );
        let output_paths: Vec<_> = if mappings.is_empty() {
            vec![Vec::new()]
        } else {
            mappings.iter().map(|(path, _)| path.clone()).collect()
        };
        for output_path in output_paths {
            let key = (
                output_path.clone(),
                *formal,
                control.projection.clone(),
                RelationKind::Control,
            );
            if summary.relations.len() >= MAX_CONDITION_RELATIONS
                && !summary.relations.contains_key(&key)
            {
                summary.unknown.insert(output_path);
                continue;
            }
            merge_coverage(summary.relations.entry(key).or_default(), Coverage::SOME);
        }
    }

    for ((left, right, kind), coverage) in &evidence.relations {
        let (packed, formal) = match (&left.origin, &right.origin) {
            (_, Origin::Formal(index)) => (left, *index),
            _ => continue,
        };
        let projectable = *kind == RelationKind::Value
            && (evidence
                .projection_relations
                .contains_key(&(left.clone(), right.clone()))
                || evidence
                    .projection_relations
                    .contains_key(&(right.clone(), left.clone())));
        let output_mappings = if projectable {
            map_internal_relation_to_output(packed, &mappings, limit)
        } else {
            map_internal_to_output(packed, &mappings, limit)
                .into_iter()
                .map(|path| (path, Vec::new()))
                .collect()
        };
        if output_mappings.is_empty() {
            if mappings
                .iter()
                .any(|(_, source)| source.origin == packed.origin)
            {
                summary.unknown.insert(Vec::new());
            }
            continue;
        }
        for (output_path, formal_suffix) in output_mappings {
            let observes_loaded_content = matches!(packed.origin, Origin::InternalResource(_))
                && evidence.content_shape_uses.keys().any(|used| {
                    atom_prefix_matches(used, packed) || atom_prefix_matches(packed, used)
                });
            let relation_kind = if *kind == RelationKind::AddressOnly && observes_loaded_content {
                RelationKind::Value
            } else if formal == 0 && *kind == RelationKind::Control {
                RelationKind::AddressOnly
            } else {
                *kind
            };
            let mut formal_path = right.projection.clone();
            if formal_path.len().saturating_add(formal_suffix.len()) > limit {
                summary.unknown.insert(output_path);
                continue;
            }
            formal_path.extend(formal_suffix);
            let key = (output_path, formal, formal_path, relation_kind);
            if summary.relations.len() >= MAX_CONDITION_RELATIONS
                && !summary.relations.contains_key(&key)
            {
                summary.unknown.insert(Vec::new());
                break;
            }
            merge_coverage(summary.relations.entry(key).or_default(), *coverage);
        }
    }
    for ((left, right), coverage) in &evidence.projection_relations {
        let (packed, formal) = match (&left.origin, &right.origin) {
            (_, Origin::Formal(index)) => (left, *index),
            _ => continue,
        };
        for (output_path, formal_suffix) in
            map_internal_relation_to_output(packed, &mappings, limit)
        {
            let mut formal_path = right.projection.clone();
            if formal_path.len().saturating_add(formal_suffix.len()) > limit {
                summary.partial = true;
                continue;
            }
            formal_path.extend(formal_suffix);
            let key = (output_path, formal, formal_path);
            if summary.projection_relations.len() >= MAX_CONDITION_RELATIONS
                && !summary.projection_relations.contains_key(&key)
            {
                summary.partial = true;
                break;
            }
            merge_coverage(
                summary.projection_relations.entry(key).or_default(),
                *coverage,
            );
        }
    }
    summary
}

fn map_internal_to_output(
    needle: &Atom,
    mappings: &[(Projection, Atom)],
    limit: usize,
) -> Vec<Projection> {
    let mut out = BTreeSet::new();
    for (output_path, source) in mappings {
        if source.origin != needle.origin {
            continue;
        }
        if needle.projection.starts_with(&source.projection) {
            let mut path = output_path.clone();
            let suffix = &needle.projection[source.projection.len()..];
            if path.len().saturating_add(suffix.len()) <= limit {
                path.extend_from_slice(suffix);
                out.insert(path);
            }
        } else if source.projection.starts_with(&needle.projection) {
            out.insert(output_path.clone());
        }
    }
    out.into_iter().collect()
}

fn map_internal_relation_to_output(
    needle: &Atom,
    mappings: &[(Projection, Atom)],
    limit: usize,
) -> Vec<(Projection, Projection)> {
    let mut out = BTreeSet::new();
    for (output_path, source) in mappings {
        if source.origin != needle.origin {
            continue;
        }
        if needle.projection.starts_with(&source.projection) {
            let mut path = output_path.clone();
            let suffix = &needle.projection[source.projection.len()..];
            if path.len().saturating_add(suffix.len()) <= limit {
                path.extend_from_slice(suffix);
                out.insert((path, Vec::new()));
            }
        } else if source.projection.starts_with(&needle.projection) {
            let suffix = source.projection[needle.projection.len()..].to_vec();
            out.insert((output_path.clone(), suffix));
        }
    }
    out.into_iter().collect()
}

fn predicate_unknown_group(
    group: &BTreeSet<Atom>,
    mappings: &[(Projection, Atom)],
    limit: usize,
) -> BTreeSet<PredicateEndpoint> {
    let mut out = BTreeSet::new();
    for atom in group {
        match atom.origin {
            Origin::Formal(formal) => {
                out.insert(PredicateEndpoint::Formal(formal, atom.projection.clone()));
            }
            Origin::InternalResource(_) => {
                let paths = map_internal_to_output(atom, mappings, limit);
                out.extend(paths.into_iter().map(PredicateEndpoint::Bound));
            }
            Origin::Subject(_) => {}
        }
    }
    out
}

fn join_predicate_alternatives(
    left: PredicateSummary,
    right: PredicateSummary,
) -> PredicateSummary {
    fn join_map<K: Ord + Clone>(
        left: &BTreeMap<K, Coverage>,
        right: &BTreeMap<K, Coverage>,
    ) -> BTreeMap<K, Coverage> {
        let keys: BTreeSet<_> = left.keys().chain(right.keys()).cloned().collect();
        keys.into_iter()
            .filter_map(|key| {
                let joined = left
                    .get(&key)
                    .copied()
                    .unwrap_or_default()
                    .merge_alternative(right.get(&key).copied().unwrap_or_default());
                joined.some.then_some((key, joined))
            })
            .collect()
    }

    PredicateSummary {
        constrained: join_map(&left.constrained, &right.constrained),
        unknown: left.unknown.union(&right.unknown).cloned().collect(),
        intrinsic_content: left
            .intrinsic_content
            .merge_alternative(right.intrinsic_content),
        intrinsic_unknown: left.intrinsic_unknown || right.intrinsic_unknown,
        value_uses: join_map(&left.value_uses, &right.value_uses),
        shape_uses: join_map(&left.shape_uses, &right.shape_uses),
        unknown_inputs: left
            .unknown_inputs
            .union(&right.unknown_inputs)
            .cloned()
            .collect(),
        unknown_shape_inputs: left
            .unknown_shape_inputs
            .union(&right.unknown_shape_inputs)
            .cloned()
            .collect(),
        relations: join_map(&left.relations, &right.relations),
        projection_relations: join_map(&left.projection_relations, &right.projection_relations),
        relation_unknown: left
            .relation_unknown
            .union(&right.relation_unknown)
            .cloned()
            .collect(),
        address_relation_unknown: left
            .address_relation_unknown
            .union(&right.address_relation_unknown)
            .cloned()
            .collect(),
        partial: left.partial || right.partial,
        opaque: left.opaque || right.opaque,
        parameter_count: left.parameter_count.max(right.parameter_count),
        global_symbols: if left.global_symbols.is_empty() {
            right.global_symbols
        } else {
            left.global_symbols
        },
    }
}

struct Analyzer<'a> {
    module: &'a SpecModule,
    options: &'a SanityOptions,
    logical: BTreeMap<Sym, LogicalSummary>,
    predicates: BTreeMap<Sym, PredicateSummary>,
    limitations: BTreeSet<AnalysisLimitation>,
}

impl<'a> Analyzer<'a> {
    fn new(module: &'a SpecModule, options: &'a SanityOptions) -> Self {
        let limit = options.projection_depth.max(1);
        let logical = logical_summaries(module, limit);
        let predicates = predicate_summaries(module, &logical, limit);
        Self {
            module,
            options,
            logical,
            predicates,
            limitations: BTreeSet::new(),
        }
    }

    fn run(mut self) -> SanityReport {
        self.note_duplicate_declarations();
        let mut diagnostics = Vec::new();
        let mut functions = Vec::new();
        let mut selected = false;
        for (index, function) in self.module.functions.iter().enumerate() {
            if self
                .options
                .function
                .as_deref()
                .is_some_and(|name| name != function.name)
            {
                continue;
            }
            selected = true;
            let mut analysis = FunctionAnalysis::new(
                self.module,
                function,
                index,
                self.options,
                &self.logical,
                &self.predicates,
            );
            analysis.analyze();
            let (summary, mut findings, limitations) = analysis.finish();
            diagnostics.append(&mut findings);
            self.limitations.extend(limitations);
            functions.push(summary);
        }

        if !selected && let Some(name) = &self.options.function {
            self.limitations.insert(AnalysisLimitation {
                code: LimitationCode::UnknownFunction,
                function: Some(name.clone()),
                subject: None,
                message: format!("no specified function named `{name}` was found"),
                location: IrLocation {
                    function: Some(name.clone()),
                    phase: SanityPhase::Signature,
                    path: "function-selection".into(),
                },
            });
        }

        diagnostics.sort_by(|left, right| {
            left.function
                .cmp(&right.function)
                .then_with(|| left.code.cmp(&right.code))
                .then_with(|| left.subject.cmp(&right.subject))
                .then_with(|| left.message.cmp(&right.message))
        });
        diagnostics.dedup_by(|left, right| {
            left.function == right.function
                && left.code == right.code
                && left.subject == right.subject
                && left.message == right.message
        });
        let mut suppressed_codes: Vec<_> = self.options.suppress.iter().copied().collect();
        suppressed_codes.sort();
        SanityReport {
            schema_version: SanityReport::SCHEMA_VERSION,
            diagnostics,
            limitations: self.limitations.into_iter().collect(),
            functions,
            suppressed_codes,
        }
    }

    fn note_duplicate_declarations(&mut self) {
        let mut predicates = BTreeSet::new();
        for predicate in &self.module.predicates {
            if !predicates.insert(predicate.name.clone()) {
                self.limitations.insert(AnalysisLimitation {
                    code: LimitationCode::DuplicatePredicate,
                    function: None,
                    subject: None,
                    message: format!(
                        "predicate {} has duplicate declarations; calls use the first",
                        symbol_label(&predicate.name)
                    ),
                    location: IrLocation {
                        function: None,
                        phase: SanityPhase::Predicate,
                        path: format!("predicates[{}]", predicate.name.num),
                    },
                });
            }
        }
        let mut functions = BTreeSet::new();
        for function in &self.module.logical_functions {
            if !functions.insert(function.name.clone()) {
                self.limitations.insert(AnalysisLimitation {
                    code: LimitationCode::DuplicateLogicalFunction,
                    function: None,
                    subject: None,
                    message: format!(
                        "logical function {} has duplicate declarations; calls use the first",
                        symbol_label(&function.name)
                    ),
                    location: IrLocation {
                        function: None,
                        phase: SanityPhase::LogicalFunction,
                        path: format!("logical_functions[{}]", function.name.num),
                    },
                });
            }
        }
    }
}

struct FunctionAnalysis<'a> {
    module: &'a SpecModule,
    function: &'a crate::spec::FunctionSpec,
    function_index: usize,
    options: &'a SanityOptions,
    logical: &'a BTreeMap<Sym, LogicalSummary>,
    predicates: &'a BTreeMap<Sym, PredicateSummary>,
    subjects: Vec<SubjectData>,
    env: Env,
    relations: BTreeMap<RelationKey, RelationEvidence>,
    relation_links: BTreeMap<RelationLinkKey, RelationEvidence>,
    limitations: BTreeSet<AnalysisLimitation>,
    return_key: Option<SubjectKey>,
    post_resources: Vec<SubjectKey>,
}

impl<'a> FunctionAnalysis<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        module: &'a SpecModule,
        function: &'a crate::spec::FunctionSpec,
        function_index: usize,
        options: &'a SanityOptions,
        logical: &'a BTreeMap<Sym, LogicalSummary>,
        predicates: &'a BTreeMap<Sym, PredicateSummary>,
    ) -> Self {
        Self {
            module,
            function,
            function_index,
            options,
            logical,
            predicates,
            subjects: Vec::new(),
            env: Env::new(),
            relations: BTreeMap::new(),
            relation_links: BTreeMap::new(),
            limitations: BTreeSet::new(),
            return_key: None,
            post_resources: Vec::new(),
        }
    }

    fn analyze(&mut self) {
        self.seed_globals();
        if let Some(internal) = &self.function.internal {
            let mut node = internal;
            loop {
                match node {
                    crate::spec::ArgType::Computational((sym, bt), rest) => {
                        self.bind_input(sym, bt, SubjectKind::Argument);
                        node = rest;
                    }
                    crate::spec::ArgType::Ghost((sym, bt), rest) => {
                        self.bind_input(sym, bt, SubjectKind::GhostArgument);
                        node = rest;
                    }
                    crate::spec::ArgType::L(lat) => {
                        self.walk_precondition(lat);
                        break;
                    }
                }
            }
        } else {
            self.seed_unbound_c_arguments();
        }

        if self.function.ret != CType::Void {
            let (sym, bt) = &self.function.ensures.var;
            let key = self.add_subject(
                SubjectKind::ReturnValue,
                symbol_label(sym),
                Some(sym.num),
                bt.clone(),
                false,
                true,
            );
            self.env.insert(sym.clone(), Flow::atom(Atom::subject(key)));
            self.return_key = Some(key);
        } else {
            self.env
                .insert(self.function.ensures.var.0.clone(), Flow::default());
        }
        self.walk_postcondition(&self.function.ensures.body);
        self.materialize_relation_closure();
    }

    fn seed_globals(&mut self) {
        for global in &self.module.globals {
            let key = self.add_subject(
                SubjectKind::Global,
                symbol_label(&global.address),
                Some(global.address.num),
                BaseType::loc_to(global.ty.clone()),
                true,
                false,
            );
            self.subjects[key.0].relevant = false;
            self.env
                .insert(global.address.clone(), Flow::atom(Atom::subject(key)));
        }
    }

    fn bind_input(&mut self, sym: &Sym, bt: &BaseType, kind: SubjectKind) {
        let key = self.add_subject(
            kind,
            symbol_label(sym),
            Some(sym.num),
            bt.clone(),
            true,
            false,
        );
        self.env.insert(sym.clone(), Flow::atom(Atom::subject(key)));
    }

    fn seed_unbound_c_arguments(&mut self) {
        if self.function.params.is_empty() {
            return;
        }
        let location = self.location(SanityPhase::Signature, "internal");
        self.limitations.insert(AnalysisLimitation {
            code: LimitationCode::UnboundCArguments,
            function: Some(self.function.name.clone()),
            subject: None,
            message: "the specification has C parameters but no retained logical argument binders"
                .into(),
            location: location.clone(),
        });
        for field in &self.function.params {
            let bt = ctype_base_type(&field.ty, self.module).unwrap_or(BaseType::Unit);
            let key = self.add_subject(
                SubjectKind::Argument,
                field.name.clone(),
                None,
                bt,
                true,
                false,
            );
            self.subjects[key.0]
                .post_value_use
                .mark_unknown(Vec::new(), &location);
            self.subjects[key.0].public.symbol = None;
        }
    }

    fn add_subject(
        &mut self,
        kind: SubjectKind,
        name: String,
        symbol: Option<u64>,
        bt: BaseType,
        input: bool,
        output: bool,
    ) -> SubjectKey {
        let key = SubjectKey(self.subjects.len());
        self.subjects.push(SubjectData::new(
            Subject {
                kind,
                name,
                symbol,
                projection: Vec::new(),
            },
            bt,
            input,
            output,
        ));
        key
    }

    fn walk_precondition(&mut self, lat: &Lat<()>) {
        let mut node = lat;
        let mut position = 0usize;
        loop {
            let location = self.location(
                SanityPhase::Precondition,
                format!("functions[{}].requires[{position}]", self.function_index),
            );
            match node {
                Lat::Define((binder, value), rest) => {
                    let mut evaluated = eval_index_term(
                        value,
                        &self.env,
                        self.logical,
                        self.options.projection_depth.max(1),
                    );
                    if value.bt == BaseType::Bool {
                        evaluated.condition = Some(Box::new(inspect_condition_term(
                            value,
                            &self.env,
                            self.logical,
                            self.options.projection_depth.max(1),
                        )));
                    }
                    self.env.insert(binder.clone(), evaluated);
                    node = rest;
                }
                Lat::Resource((binder, (request, bt)), rest) => {
                    self.inspect_pre_request(request, &location);
                    let key = self.add_subject(
                        SubjectKind::PreStateResource,
                        symbol_label(binder),
                        Some(binder.num),
                        bt.clone(),
                        true,
                        false,
                    );
                    self.subjects[key.0]
                        .ownership
                        .observe(Vec::new(), Coverage::EVERY, &location);
                    self.subjects[key.0].content_relevant = request_has_content(request, bt);
                    self.env
                        .insert(binder.clone(), Flow::atom(Atom::subject(key)));
                    node = rest;
                }
                Lat::Constraint(constraint, rest) => {
                    let evidence = inspect_constraint(
                        constraint,
                        &self.env,
                        self.logical,
                        self.options.projection_depth.max(1),
                    );
                    self.mark_precondition_evidence(evidence, &location);
                    node = rest;
                }
                Lat::I(()) => break,
            }
            position += 1;
        }
    }

    fn walk_postcondition(&mut self, lat: &Lat<()>) {
        let mut node = lat;
        let mut position = 0usize;
        loop {
            let location = self.location(
                SanityPhase::Postcondition,
                format!(
                    "functions[{}].ensures.body[{position}]",
                    self.function_index
                ),
            );
            match node {
                Lat::Define((binder, value), rest) => {
                    let mut evaluated = eval_index_term(
                        value,
                        &self.env,
                        self.logical,
                        self.options.projection_depth.max(1),
                    );
                    if value.bt == BaseType::Bool {
                        evaluated.condition = Some(Box::new(inspect_condition_term(
                            value,
                            &self.env,
                            self.logical,
                            self.options.projection_depth.max(1),
                        )));
                    }
                    self.env.insert(binder.clone(), evaluated);
                    node = rest;
                }
                Lat::Resource((binder, (request, bt)), rest) => {
                    let key = self.add_subject(
                        SubjectKind::PostStateResource,
                        symbol_label(binder),
                        Some(binder.num),
                        bt.clone(),
                        false,
                        true,
                    );
                    self.subjects[key.0].content_relevant = request_has_content(request, bt);
                    self.inspect_post_request(request, key, &location);
                    self.env
                        .insert(binder.clone(), Flow::atom(Atom::subject(key)));
                    self.post_resources.push(key);
                    node = rest;
                }
                Lat::Constraint(constraint, rest) => {
                    let evidence = inspect_constraint(
                        constraint,
                        &self.env,
                        self.logical,
                        self.options.projection_depth.max(1),
                    );
                    self.apply_condition(evidence, &location);
                    node = rest;
                }
                Lat::I(()) => break,
            }
            position += 1;
        }
    }

    fn mark_precondition_evidence(&mut self, evidence: ConditionEvidence, location: &IrLocation) {
        if evidence.incomplete {
            self.note_incomplete_limitation(location);
        }
        let mut atoms = BTreeSet::new();
        atoms.extend(evidence.uses.keys().cloned());
        atoms.extend(evidence.shape_uses.keys().cloned());
        atoms.extend(evidence.content_uses.keys().cloned());
        atoms.extend(evidence.unknown);
        atoms.extend(evidence.shape_unknown);
        atoms.extend(evidence.content_shape_unknown);
        atoms.extend(evidence.content_unknown);
        for group in evidence
            .relation_unknown
            .into_iter()
            .chain(evidence.address_relation_unknown)
        {
            atoms.extend(group);
        }
        for ((left, right, _), _) in evidence.relations {
            atoms.insert(left);
            atoms.insert(right);
        }
        for ((left, right), _) in evidence.projection_relations {
            atoms.insert(left);
            atoms.insert(right);
        }
        if evidence.incomplete {
            atoms.extend(
                self.subjects
                    .iter()
                    .enumerate()
                    .filter(|(_, subject)| subject.public.kind == SubjectKind::Global)
                    .map(|(index, _)| Atom::subject(SubjectKey(index))),
            );
        }
        for atom in atoms {
            if let Origin::Subject(key) = atom.origin {
                self.subjects[key.0].relevant = true;
            }
        }
    }

    fn inspect_pre_request(&mut self, request: &Request, location: &IrLocation) {
        let (pointer, iargs, permission) = request_flows(
            request,
            &self.env,
            self.logical,
            self.options.projection_depth.max(1),
        );
        for flow in std::iter::once(&pointer)
            .chain(iargs.iter())
            .chain(permission.iter())
        {
            self.mark_shape_flow(flow, location, false);
        }
    }

    fn inspect_post_request(
        &mut self,
        request: &Request,
        bound: SubjectKey,
        location: &IrLocation,
    ) {
        self.subjects[bound.0]
            .ownership
            .observe(Vec::new(), Coverage::EVERY, location);
        let (pointer, iargs, permission) = request_flows(
            request,
            &self.env,
            self.logical,
            self.options.projection_depth.max(1),
        );
        let all_flows: Vec<_> = std::iter::once(&pointer)
            .chain(iargs.iter())
            .chain(permission.iter())
            .collect();
        for flow in &all_flows {
            self.mark_shape_flow(flow, location, true);
        }

        let bound_atom = Atom::subject(bound);
        for flow in &all_flows {
            let conditional = flow.conditional_atoms();
            let inputs: Vec<_> = flow
                .atoms()
                .into_iter()
                .filter(|atom| self.is_input_atom(atom))
                .collect();
            for input in inputs {
                self.record_relation(
                    bound_atom.clone(),
                    input.clone(),
                    RelationKind::AddressOnly,
                    Coverage::branch(conditional.contains(&input)),
                    location,
                );
            }
        }
        for flow in &all_flows {
            let outputs: Vec<_> = flow
                .atoms()
                .into_iter()
                .filter(|atom| self.is_output_atom(atom))
                .collect();
            let inputs: Vec<_> = flow
                .atoms()
                .into_iter()
                .filter(|atom| self.is_input_atom(atom))
                .collect();
            let conditional = flow.conditional_atoms();
            for output in outputs {
                for input in &inputs {
                    if flow.atoms_are_exclusive(&output, input) {
                        continue;
                    }
                    self.record_relation(
                        output.clone(),
                        input.clone(),
                        RelationKind::AddressOnly,
                        Coverage::branch(
                            conditional.contains(&output) || conditional.contains(input),
                        ),
                        location,
                    );
                }
            }
        }

        let RequestName::PName(name) = request.name() else {
            return;
        };
        let mut actuals = vec![pointer];
        actuals.extend(iargs);
        match self.predicates.get(name) {
            Some(summary) if !summary.opaque => {
                self.apply_post_predicate(summary, bound, &actuals, location)
            }
            opaque => {
                self.subjects[bound.0]
                    .value
                    .mark_unknown(Vec::new(), location);
                self.subjects[bound.0]
                    .resource_contents
                    .mark_unknown(Vec::new(), location);
                let global_symbols = opaque
                    .map(|summary| summary.global_symbols.clone())
                    .unwrap_or_else(|| {
                        self.module
                            .globals
                            .iter()
                            .map(|global| global.address.clone())
                            .collect()
                    });
                let mut unknown_flows = actuals.clone();
                unknown_flows.extend(
                    global_symbols
                        .iter()
                        .filter_map(|symbol| self.env.get(symbol).cloned()),
                );
                if unknown_flows
                    .iter()
                    .flat_map(|flow| flow.atoms().into_iter().chain(flow.unknown_atoms()))
                    .any(|atom| self.is_input_atom(&atom))
                {
                    self.subjects[bound.0]
                        .relation
                        .mark_unknown(Vec::new(), location);
                }
                for atom in unknown_flows.iter().flat_map(|flow| flow.atoms()) {
                    self.mark_unknown_atom(&atom, location);
                }
                for atom in unknown_flows.iter().flat_map(|flow| flow.unknown_atoms()) {
                    self.mark_unknown_atom(&atom, location);
                }
                let detail = match self.module.predicate(name) {
                    Some(predicate) => match predicate.catch_all_clauses() {
                        None => "is uninterpreted",
                        Some(Err(ClauseShapeError::Empty)) => "has no clauses",
                        Some(Err(ClauseShapeError::GuardedLast)) => {
                            "has no literal-true catch-all clause"
                        }
                        Some(Ok(_)) => "could not be summarized",
                    },
                    None => "has no declaration",
                };
                self.limitations.insert(AnalysisLimitation {
                    code: LimitationCode::OpaquePredicate,
                    function: Some(self.function.name.clone()),
                    subject: Some(self.subjects[bound.0].public.clone()),
                    message: format!("predicate {} {detail}", symbol_label(name)),
                    location: location.clone(),
                });
            }
        }
    }

    fn apply_post_predicate(
        &mut self,
        summary: &PredicateSummary,
        bound: SubjectKey,
        actuals: &[Flow],
        location: &IrLocation,
    ) {
        if actuals.len() != summary.parameter_count {
            self.subjects[bound.0]
                .value
                .mark_unknown(Vec::new(), location);
            self.subjects[bound.0]
                .resource_contents
                .mark_unknown(Vec::new(), location);
            if actuals
                .iter()
                .flat_map(|flow| flow.atoms().into_iter().chain(flow.unknown_atoms()))
                .any(|atom| self.is_input_atom(&atom))
            {
                self.subjects[bound.0]
                    .relation
                    .mark_unknown(Vec::new(), location);
            }
            for atom in actuals
                .iter()
                .flat_map(|flow| flow.atoms().into_iter().chain(flow.unknown_atoms()))
            {
                self.mark_unknown_atom(&atom, location);
            }
            self.note_incomplete_limitation(location);
            return;
        }
        if summary.intrinsic_content.some {
            self.subjects[bound.0].resource_contents.observe(
                Vec::new(),
                summary.intrinsic_content,
                location,
            );
        }
        if summary.intrinsic_unknown {
            self.subjects[bound.0]
                .resource_contents
                .mark_unknown(Vec::new(), location);
        }
        for ((formal, path), coverage) in &summary.value_uses {
            let Some(actual) = predicate_actual(summary, *formal, actuals, &self.env) else {
                self.note_incomplete_limitation(location);
                continue;
            };
            let actual = actual.project_path(path, self.options.projection_depth.max(1));
            let conditional = actual.conditional_atoms();
            for atom in actual
                .data_atoms()
                .into_iter()
                .chain(actual.control.iter().cloned())
            {
                self.mark_value_atom(
                    &atom,
                    coverage.narrow(conditional.contains(&atom)),
                    location,
                );
            }
            for atom in &actual.shape_control {
                self.mark_value_atom(atom, scale_coverage(*coverage, Coverage::SOME), location);
                self.mark_post_use(
                    atom,
                    scale_coverage(*coverage, Coverage::SOME),
                    location,
                    true,
                );
            }
            for atom in actual.unknown_atoms() {
                self.mark_unknown_atom(&atom, location);
            }
        }
        for ((formal, path), coverage) in &summary.shape_uses {
            let Some(actual) = predicate_actual(summary, *formal, actuals, &self.env) else {
                self.note_incomplete_limitation(location);
                continue;
            };
            let actual = actual.project_path(path, self.options.projection_depth.max(1));
            let conditional = actual.conditional_atoms();
            for atom in actual
                .data_atoms()
                .into_iter()
                .chain(actual.shape_control.iter().cloned())
            {
                self.mark_post_use(
                    &atom,
                    coverage.narrow(conditional.contains(&atom)),
                    location,
                    true,
                );
            }
            for atom in &actual.control {
                self.mark_post_use(
                    atom,
                    scale_coverage(*coverage, Coverage::SOME),
                    location,
                    true,
                );
            }
            for atom in actual.unknown_atoms() {
                self.mark_unknown_shape_atom(&atom, location, true);
            }
        }
        for (formal, path) in &summary.unknown_inputs {
            let Some(actual) = predicate_actual(summary, *formal, actuals, &self.env) else {
                self.note_incomplete_limitation(location);
                continue;
            };
            let actual = actual.project_path(path, self.options.projection_depth.max(1));
            for atom in actual.atoms().into_iter().chain(actual.unknown_atoms()) {
                self.mark_unknown_atom(&atom, location);
            }
        }
        for (formal, path) in &summary.unknown_shape_inputs {
            let Some(actual) = predicate_actual(summary, *formal, actuals, &self.env) else {
                self.note_incomplete_limitation(location);
                continue;
            };
            let actual = actual.project_path(path, self.options.projection_depth.max(1));
            for atom in actual.atoms().into_iter().chain(actual.unknown_atoms()) {
                self.mark_unknown_shape_atom(&atom, location, true);
            }
        }
        for (path, coverage) in &summary.constrained {
            self.subjects[bound.0]
                .value
                .observe(path.clone(), *coverage, location);
        }
        for path in &summary.unknown {
            self.subjects[bound.0]
                .value
                .mark_unknown(path.clone(), location);
        }
        let bound_atom = Atom::subject(bound);
        for (groups, address_only) in [
            (&summary.relation_unknown, false),
            (&summary.address_relation_unknown, true),
        ] {
            for group in groups {
                let (atoms, incomplete) = substitute_predicate_unknown_group(
                    summary,
                    group,
                    &bound_atom,
                    actuals,
                    &self.env,
                    self.options.projection_depth.max(1),
                );
                if incomplete {
                    self.note_incomplete_limitation(location);
                }
                if !atoms.iter().any(|atom| self.is_input_atom(atom)) {
                    continue;
                }
                for atom in atoms {
                    if address_only {
                        self.mark_address_relation_unknown_atom(&atom, location);
                        if self.is_input_atom(&atom) {
                            self.mark_unknown_shape_atom(&atom, location, true);
                        }
                    } else {
                        self.mark_relation_unknown_atom(&atom, location);
                        if self.is_input_atom(&atom) {
                            self.mark_unknown_atom(&atom, location);
                        }
                    }
                }
            }
        }
        if summary.partial || summary.intrinsic_unknown || !summary.unknown.is_empty() {
            self.limitations.insert(AnalysisLimitation {
                code: LimitationCode::PredicateSummary,
                function: Some(self.function.name.clone()),
                subject: Some(self.subjects[bound.0].public.clone()),
                message: "the predicate summary is partial because of recursion, projection loss, or unsupported evidence"
                    .into(),
                location: location.clone(),
            });
        }
        for ((output_path, formal, formal_path, kind), coverage) in &summary.relations {
            if *kind == RelationKind::Value
                && summary.projection_relations.contains_key(&(
                    output_path.clone(),
                    *formal,
                    formal_path.clone(),
                ))
            {
                continue;
            }
            let Some(actual) = predicate_actual(summary, *formal, actuals, &self.env) else {
                if *kind == RelationKind::AddressOnly {
                    self.subjects[bound.0]
                        .address_relation
                        .mark_unknown(output_path.clone(), location);
                } else {
                    self.subjects[bound.0]
                        .relation
                        .mark_unknown(output_path.clone(), location);
                }
                self.note_incomplete_limitation(location);
                continue;
            };
            let projected = actual.project_path(formal_path, self.options.projection_depth.max(1));
            let conditional = projected.conditional_atoms();
            for input in projected.data_atoms() {
                let mut output = Atom::subject(bound);
                output.projection = output_path.clone();
                let relation_coverage = coverage.narrow(conditional.contains(&input));
                self.record_relation(output, input.clone(), *kind, relation_coverage, location);
                self.mark_post_use(
                    &input,
                    relation_coverage,
                    location,
                    *kind == RelationKind::AddressOnly,
                );
            }
            for input in &projected.control {
                let mut output = Atom::subject(bound);
                output.projection = output_path.clone();
                let relation_kind = compose_relation_kind(*kind, RelationKind::Control);
                self.record_relation(
                    output,
                    input.clone(),
                    relation_kind,
                    scale_coverage(*coverage, Coverage::SOME),
                    location,
                );
                self.mark_post_use(
                    input,
                    scale_coverage(*coverage, Coverage::SOME),
                    location,
                    relation_kind == RelationKind::AddressOnly,
                );
            }
            for input in &projected.shape_control {
                let mut output = Atom::subject(bound);
                output.projection = output_path.clone();
                let relation_kind = compose_relation_kind(*kind, RelationKind::Control);
                self.record_relation(
                    output,
                    input.clone(),
                    relation_kind,
                    scale_coverage(*coverage, Coverage::SOME),
                    location,
                );
                self.mark_post_use(
                    input,
                    scale_coverage(*coverage, Coverage::SOME),
                    location,
                    relation_kind == RelationKind::AddressOnly,
                );
                if relation_kind != RelationKind::AddressOnly {
                    self.mark_post_use(
                        input,
                        scale_coverage(*coverage, Coverage::SOME),
                        location,
                        true,
                    );
                }
            }
            for unknown in projected.unknown_atoms() {
                if *kind == RelationKind::AddressOnly {
                    self.mark_unknown_shape_atom(&unknown, location, true);
                    self.subjects[bound.0]
                        .address_relation
                        .mark_unknown(output_path.clone(), location);
                } else {
                    self.mark_unknown_atom(&unknown, location);
                    self.subjects[bound.0]
                        .relation
                        .mark_unknown(output_path.clone(), location);
                }
            }
        }
        for ((output_path, formal, formal_path), coverage) in &summary.projection_relations {
            let Some(actual) = predicate_actual(summary, *formal, actuals, &self.env) else {
                self.note_incomplete_limitation(location);
                continue;
            };
            let projected = actual.project_path(formal_path, self.options.projection_depth.max(1));
            if !projected.projection_preserving() {
                if projected.fields.is_empty() {
                    let conditional = projected.conditional_atoms();
                    for input in projected.data_atoms() {
                        let mut output = Atom::subject(bound);
                        output.projection = output_path.clone();
                        let relation_coverage = coverage.narrow(conditional.contains(&input));
                        self.record_relation(
                            output,
                            input.clone(),
                            RelationKind::Value,
                            relation_coverage,
                            location,
                        );
                        self.mark_post_use(&input, relation_coverage, location, false);
                    }
                    for input in projected.control.iter().chain(&projected.shape_control) {
                        let mut output = Atom::subject(bound);
                        output.projection = output_path.clone();
                        self.record_relation(
                            output,
                            input.clone(),
                            RelationKind::Control,
                            scale_coverage(*coverage, Coverage::SOME),
                            location,
                        );
                        self.mark_post_use(
                            input,
                            scale_coverage(*coverage, Coverage::SOME),
                            location,
                            false,
                        );
                    }
                    if projected.incomplete || !projected.unknown_atoms().is_empty() {
                        self.subjects[bound.0]
                            .relation
                            .mark_unknown(output_path.clone(), location);
                        self.note_incomplete_limitation(location);
                    }
                    continue;
                }
                self.subjects[bound.0]
                    .relation
                    .mark_unknown(output_path.clone(), location);
                for atom in projected
                    .atoms()
                    .into_iter()
                    .chain(projected.unknown_atoms())
                {
                    if self.is_input_atom(&atom) {
                        self.mark_unknown_atom(&atom, location);
                    }
                }
                self.note_incomplete_limitation(location);
                continue;
            }
            let conditional = projected.conditional_atoms();
            for input in projected.data_atoms() {
                let mut output = Atom::subject(bound);
                output.projection = output_path.clone();
                let relation_coverage = coverage.narrow(conditional.contains(&input));
                self.record_link(
                    output.clone(),
                    input.clone(),
                    RelationKind::Value,
                    relation_coverage,
                    true,
                    location,
                );
                if self.is_input_atom(&input) {
                    self.record_public_relation(
                        output,
                        input.clone(),
                        RelationKind::Value,
                        relation_coverage,
                        location,
                    );
                }
                self.mark_post_use(&input, relation_coverage, location, false);
            }
        }
    }

    fn apply_condition(&mut self, mut evidence: ConditionEvidence, location: &IrLocation) {
        if evidence.incomplete {
            self.note_incomplete_limitation(location);
            let global_atoms: Vec<_> = self
                .subjects
                .iter()
                .enumerate()
                .filter(|(_, subject)| subject.public.kind == SubjectKind::Global)
                .map(|(index, _)| Atom::subject(SubjectKey(index)))
                .collect();
            for groups in [
                &mut evidence.relation_unknown,
                &mut evidence.address_relation_unknown,
            ] {
                *groups = std::mem::take(groups)
                    .into_iter()
                    .map(|mut group| {
                        if group.iter().any(|atom| self.is_output_atom(atom)) {
                            group.extend(global_atoms.iter().cloned());
                        }
                        group
                    })
                    .collect();
            }
        }
        for (atom, coverage) in evidence.uses {
            self.mark_value_atom(&atom, coverage, location);
        }
        for (atom, coverage) in evidence.shape_uses {
            if self.is_resource_content_atom(&atom) {
                self.mark_value_atom(&atom, coverage, location);
            } else {
                self.mark_post_use(&atom, coverage, location, true);
            }
        }
        for atom in evidence.unknown {
            self.mark_unknown_atom(&atom, location);
        }
        for atom in evidence.shape_unknown {
            if self.is_resource_content_atom(&atom) {
                self.mark_unknown_atom(&atom, location);
            } else {
                self.mark_unknown_shape_atom(&atom, location, true);
            }
        }
        for group in evidence.relation_unknown {
            if group.iter().any(|atom| self.is_input_atom(atom))
                && group.iter().any(|atom| self.is_output_atom(atom))
            {
                for atom in group {
                    self.mark_relation_unknown_atom(&atom, location);
                    if self.is_input_atom(&atom) {
                        self.mark_unknown_atom(&atom, location);
                    }
                }
            }
        }
        for group in evidence.address_relation_unknown {
            if group.iter().any(|atom| self.is_input_atom(atom))
                && group.iter().any(|atom| self.is_output_atom(atom))
            {
                let content_relation = group.iter().any(|atom| self.is_resource_content_atom(atom));
                for atom in group {
                    if content_relation {
                        self.mark_relation_unknown_atom(&atom, location);
                        if self.is_input_atom(&atom) {
                            self.mark_unknown_atom(&atom, location);
                        }
                    } else {
                        self.mark_address_relation_unknown_atom(&atom, location);
                        if self.is_input_atom(&atom) {
                            self.mark_unknown_shape_atom(&atom, location, true);
                        }
                    }
                }
            }
        }
        for ((left, right, kind), coverage) in evidence.relations {
            let kind = self.condition_relation_kind(&left, &right, kind);
            self.record_link(left.clone(), right.clone(), kind, coverage, false, location);
            if self.is_output_atom(&left) && self.is_input_atom(&right) {
                self.record_public_relation(left, right, kind, coverage, location);
            }
        }
        for ((left, right), coverage) in evidence.projection_relations {
            self.record_link(
                left.clone(),
                right.clone(),
                RelationKind::Value,
                coverage,
                true,
                location,
            );
            if self.is_output_atom(&left) && self.is_input_atom(&right) {
                self.record_public_relation(left, right, RelationKind::Value, coverage, location);
            }
        }
    }

    fn is_resource_content_atom(&self, atom: &Atom) -> bool {
        matches!(atom.origin, Origin::Subject(key) if {
            let subject = &self.subjects[key.0];
            subject.content_relevant
                && matches!(
                    subject.public.kind,
                    SubjectKind::PreStateResource | SubjectKind::PostStateResource
                )
        })
    }

    fn condition_relation_kind(
        &self,
        left: &Atom,
        right: &Atom,
        kind: RelationKind,
    ) -> RelationKind {
        if kind == RelationKind::AddressOnly
            && (self.is_resource_content_atom(left) || self.is_resource_content_atom(right))
        {
            RelationKind::Value
        } else {
            kind
        }
    }

    fn note_incomplete_limitation(&mut self, location: &IrLocation) {
        self.limitations.insert(AnalysisLimitation {
            code: LimitationCode::UnknownExpression,
            function: Some(self.function.name.clone()),
            subject: None,
            message: "an undeclared, malformed, unsupported, or budget-limited expression prevents complete classification"
                .into(),
            location: location.clone(),
        });
    }

    fn mark_value_atom(&mut self, atom: &Atom, coverage: Coverage, location: &IrLocation) {
        let Origin::Subject(key) = atom.origin else {
            return;
        };
        let subject = &mut self.subjects[key.0];
        subject.relevant = true;
        if subject.output {
            subject
                .value
                .observe(atom.projection.clone(), coverage, location);
        }
        if subject.input {
            subject
                .post_value_use
                .observe(atom.projection.clone(), coverage, location);
        }
    }

    fn mark_post_use(
        &mut self,
        atom: &Atom,
        coverage: Coverage,
        location: &IrLocation,
        shape: bool,
    ) {
        let Origin::Subject(key) = atom.origin else {
            return;
        };
        let subject = &mut self.subjects[key.0];
        subject.relevant = true;
        if shape {
            subject
                .ownership
                .observe(atom.projection.clone(), coverage, location);
        }
        if subject.input && shape {
            subject
                .post_shape_use
                .observe(atom.projection.clone(), coverage, location);
        } else if subject.input {
            subject
                .post_value_use
                .observe(atom.projection.clone(), coverage, location);
        } else if subject.output && shape {
            subject
                .ownership
                .observe(atom.projection.clone(), coverage, location);
        }
    }

    fn mark_shape_flow(&mut self, flow: &Flow, location: &IrLocation, post: bool) {
        if flow.incomplete {
            self.note_incomplete_limitation(location);
        }
        let conditional = flow.conditional_atoms();
        for atom in flow.atoms() {
            let Origin::Subject(key) = atom.origin else {
                continue;
            };
            self.subjects[key.0].relevant = true;
            self.subjects[key.0].ownership.observe(
                atom.projection.clone(),
                Coverage::branch(conditional.contains(&atom)),
                location,
            );
            if post && self.subjects[key.0].input {
                self.subjects[key.0].post_shape_use.observe(
                    atom.projection.clone(),
                    Coverage::branch(conditional.contains(&atom)),
                    location,
                );
            }
        }
        for atom in flow.unknown_atoms() {
            self.mark_unknown_shape_atom(&atom, location, post);
        }
    }

    fn mark_unknown_shape_atom(&mut self, atom: &Atom, location: &IrLocation, post: bool) {
        let Origin::Subject(key) = atom.origin else {
            return;
        };
        let subject = &mut self.subjects[key.0];
        subject.relevant = true;
        subject
            .ownership
            .mark_unknown(atom.projection.clone(), location);
        if post && subject.input {
            subject
                .post_shape_use
                .mark_unknown(atom.projection.clone(), location);
        }
        self.note_unknown_limitation(key, &atom.projection, location);
    }

    fn mark_unknown_atom(&mut self, atom: &Atom, location: &IrLocation) {
        let Origin::Subject(key) = atom.origin else {
            return;
        };
        let subject = &mut self.subjects[key.0];
        subject.relevant = true;
        if subject.output {
            subject
                .value
                .mark_unknown(atom.projection.clone(), location);
        }
        if subject.input {
            subject
                .post_value_use
                .mark_unknown(atom.projection.clone(), location);
            subject
                .post_shape_use
                .mark_unknown(atom.projection.clone(), location);
        }
        self.note_unknown_limitation(key, &atom.projection, location);
    }

    fn mark_relation_unknown_atom_in(
        &mut self,
        atom: &Atom,
        location: &IrLocation,
        address_only: bool,
    ) {
        let Origin::Subject(key) = atom.origin else {
            return;
        };
        let subject = &mut self.subjects[key.0];
        subject.relevant = true;
        if subject.output {
            let ledger = if address_only {
                &mut subject.address_relation
            } else {
                &mut subject.relation
            };
            ledger.mark_unknown(atom.projection.clone(), location);
            self.note_unknown_limitation(key, &atom.projection, location);
        }
    }

    fn mark_relation_unknown_atom(&mut self, atom: &Atom, location: &IrLocation) {
        self.mark_relation_unknown_atom_in(atom, location, false);
    }

    fn mark_address_relation_unknown_atom(&mut self, atom: &Atom, location: &IrLocation) {
        self.mark_relation_unknown_atom_in(atom, location, true);
    }

    fn note_unknown_limitation(
        &mut self,
        key: SubjectKey,
        projection: &[String],
        location: &IrLocation,
    ) {
        self.limitations.insert(AnalysisLimitation {
            code: LimitationCode::UnknownExpression,
            function: Some(self.function.name.clone()),
            subject: Some(with_projection(&self.subjects[key.0].public, projection)),
            message: "an opaque, unsupported, or budget-limited expression prevents classification"
                .into(),
            location: location.clone(),
        });
    }

    fn record_relation(
        &mut self,
        output: Atom,
        input: Atom,
        kind: RelationKind,
        coverage: Coverage,
        location: &IrLocation,
    ) {
        self.record_link(
            output.clone(),
            input.clone(),
            kind,
            coverage,
            false,
            location,
        );
        self.record_public_relation(output, input, kind, coverage, location);
    }

    fn record_link(
        &mut self,
        mut left: Atom,
        mut right: Atom,
        kind: RelationKind,
        coverage: Coverage,
        projectable: bool,
        location: &IrLocation,
    ) {
        if !matches!(left.origin, Origin::Subject(_))
            || !matches!(right.origin, Origin::Subject(_))
            || left == right
        {
            return;
        }
        if right < left {
            std::mem::swap(&mut left, &mut right);
        }
        let key = RelationLinkKey {
            left,
            right,
            kind,
            projectable,
        };
        if self.relation_links.len() >= MAX_CONDITION_RELATIONS
            && !self.relation_links.contains_key(&key)
        {
            self.note_incomplete_limitation(location);
            return;
        }
        let entry = self.relation_links.entry(key).or_default();
        merge_coverage(&mut entry.coverage, coverage);
        entry.locations.insert(location.clone());
    }

    fn record_public_relation(
        &mut self,
        output: Atom,
        input: Atom,
        kind: RelationKind,
        coverage: Coverage,
        location: &IrLocation,
    ) {
        if !self.is_output_atom(&output) || !self.is_input_atom(&input) {
            return;
        }
        if let Origin::Subject(key) = output.origin {
            let evidence = if kind == RelationKind::AddressOnly {
                &mut self.subjects[key.0].address_relation
            } else {
                &mut self.subjects[key.0].relation
            };
            evidence.observe(output.projection.clone(), coverage, location);
        }
        if let Origin::Subject(key) = input.origin {
            let evidence = if kind == RelationKind::AddressOnly {
                &mut self.subjects[key.0].address_relation
            } else {
                &mut self.subjects[key.0].relation
            };
            evidence.observe(input.projection.clone(), coverage, location);
        }
        let entry = self
            .relations
            .entry(RelationKey {
                output,
                input,
                kind,
            })
            .or_default();
        merge_coverage(&mut entry.coverage, coverage);
        entry.locations.insert(location.clone());
    }

    fn materialize_relation_closure(&mut self) {
        let mut edges = Vec::new();
        for (key, evidence) in &self.relation_links {
            if !evidence.coverage.every {
                continue;
            }
            edges.push((
                key.left.clone(),
                key.right.clone(),
                key.kind,
                key.projectable,
                evidence.clone(),
            ));
        }

        let mut starts = BTreeSet::new();
        for (left, right, _, _, _) in &edges {
            if self.is_output_atom(left) {
                starts.insert(left.clone());
            }
            if self.is_output_atom(right) {
                starts.insert(right.clone());
            }
        }
        for (index, subject) in self.subjects.iter().enumerate() {
            if !subject.output {
                continue;
            }
            let shape = projection_shape(
                &subject.bt,
                self.module,
                self.options.projection_depth.max(1),
            );
            for projection in shape.known {
                starts.insert(Atom {
                    origin: Origin::Subject(SubjectKey(index)),
                    projection,
                });
            }
        }
        let mut discovered = Vec::new();
        for start in starts {
            let mut queue = vec![(
                start.clone(),
                RelationKind::Value,
                BTreeSet::<IrLocation>::new(),
                BTreeSet::from([start.clone()]),
            )];
            let mut cursor = 0usize;
            while cursor < queue.len() && cursor < MAX_CONDITION_RELATIONS {
                let (node, path_kind, path_locations, path_nodes) = queue[cursor].clone();
                cursor += 1;
                for (left, right, edge_kind, projectable, edge_evidence) in &edges {
                    let next = relation_link_neighbor(
                        &node,
                        left,
                        right,
                        *edge_kind,
                        *projectable,
                        self.options.projection_depth.max(1),
                    )
                    .or_else(|| {
                        relation_link_neighbor(
                            &node,
                            right,
                            left,
                            *edge_kind,
                            *projectable,
                            self.options.projection_depth.max(1),
                        )
                    });
                    let Some(next) = next else {
                        continue;
                    };
                    if path_nodes.contains(&next) {
                        continue;
                    }
                    let kind = compose_relation_kind(path_kind, *edge_kind);
                    let mut locations = path_locations.clone();
                    locations.extend(edge_evidence.locations.iter().cloned());
                    if self.is_input_atom(&next) {
                        discovered.push((start.clone(), next.clone(), kind, locations.clone()));
                        continue;
                    }
                    let mut next_path = path_nodes.clone();
                    next_path.insert(next.clone());
                    queue.push((next, kind, locations, next_path));
                }
            }
            if cursor < queue.len() {
                let location = self.location(SanityPhase::Postcondition, "relation-closure");
                self.mark_relation_unknown_atom(&start, &location);
                self.note_incomplete_limitation(&location);
            }
        }
        for (output, input, kind, locations) in discovered {
            let location =
                locations.iter().next().cloned().unwrap_or_else(|| {
                    self.location(SanityPhase::Postcondition, "relation-closure")
                });
            let key = RelationKey {
                output: output.clone(),
                input: input.clone(),
                kind,
            };
            self.record_public_relation(output, input, kind, Coverage::EVERY, &location);
            if let Some(entry) = self.relations.get_mut(&key) {
                entry.locations.extend(locations);
            }
        }
    }

    fn is_input_atom(&self, atom: &Atom) -> bool {
        matches!(atom.origin, Origin::Subject(key) if self.subjects[key.0].input)
    }

    fn is_output_atom(&self, atom: &Atom) -> bool {
        matches!(atom.origin, Origin::Subject(key) if self.subjects[key.0].output)
    }

    fn location(&self, phase: SanityPhase, path: impl Into<String>) -> IrLocation {
        IrLocation {
            function: Some(self.function.name.clone()),
            phase,
            path: path.into(),
        }
    }

    fn finish(
        mut self,
    ) -> (
        FunctionSummary,
        Vec<SanityDiagnostic>,
        BTreeSet<AnalysisLimitation>,
    ) {
        let trivial = self.function.ensures.is_assert_true();
        let subject_summaries = self.subject_summaries();
        let relation_summaries = self.relation_summaries();
        let diagnostics = self.diagnostics(trivial);
        for subject in &subject_summaries {
            if subject.value_constraint == Evidence::Unknown
                || subject.resource_contents_constraint == Some(Evidence::Unknown)
                || subject.input_output_relation == Evidence::Unknown
            {
                let location = subject
                    .locations
                    .first()
                    .cloned()
                    .unwrap_or_else(|| self.location(SanityPhase::Postcondition, "analysis"));
                self.limitations.insert(AnalysisLimitation {
                    code: LimitationCode::PartialSubject,
                    function: Some(self.function.name.clone()),
                    subject: Some(subject.subject.clone()),
                    message: "this subject could only be classified partially".into(),
                    location,
                });
            }
        }
        (
            FunctionSummary {
                function: self.function.name.clone(),
                trivial_postcondition: trivial,
                subjects: subject_summaries,
                relations: relation_summaries,
            },
            diagnostics,
            self.limitations,
        )
    }

    fn subject_summaries(&self) -> Vec<SubjectSummary> {
        let mut out = Vec::new();
        for subject in &self.subjects {
            if !subject.relevant {
                continue;
            }
            let shape = projection_shape(
                &subject.bt,
                self.module,
                self.options.projection_depth.max(1),
            );
            let paths: Vec<_> = if shape.known.is_empty() {
                vec![Vec::new()]
            } else {
                shape.known.iter().cloned().collect()
            };
            for path in paths {
                let static_unknown = shape
                    .unknown
                    .iter()
                    .any(|unknown| projection_matches(unknown, &path));
                let (ownership, _) = subject.ownership.evidence_at(&path);
                let (mut value, coverage) = if subject.output {
                    subject.value.evidence_at(&path)
                } else {
                    subject.post_value_use.evidence_at(&path)
                };
                let (mut relation, _) = subject.relation.evidence_at(&path);
                let resource_contents_constraint = (subject.public.kind
                    == SubjectKind::PostStateResource)
                    .then(|| subject.resource_contents.evidence_at(&[]).0);
                if static_unknown {
                    if value == Evidence::NoEvidence {
                        value = Evidence::Unknown;
                    }
                    if relation == Evidence::NoEvidence {
                        relation = Evidence::Unknown;
                    }
                }
                let mut locations = BTreeSet::new();
                locations.extend(subject.ownership.locations.iter().cloned());
                locations.extend(subject.value.locations.iter().cloned());
                locations.extend(subject.resource_contents.locations.iter().cloned());
                locations.extend(subject.post_value_use.locations.iter().cloned());
                locations.extend(subject.post_shape_use.locations.iter().cloned());
                locations.extend(subject.relation.locations.iter().cloned());
                locations.extend(subject.address_relation.locations.iter().cloned());
                out.push(SubjectSummary {
                    subject: with_projection(&subject.public, &path),
                    ownership_or_shape_use: ownership,
                    value_constraint: value,
                    resource_contents_constraint,
                    input_output_relation: relation,
                    constraint_coverage: coverage,
                    locations: locations.into_iter().collect(),
                });
            }
            for path in shape.unknown.difference(&shape.known) {
                let mut public = with_projection(&subject.public, path);
                if public.projection.is_empty() && shape.aggregate {
                    public.projection.push("<deeper>".into());
                }
                out.push(SubjectSummary {
                    subject: public,
                    ownership_or_shape_use: Evidence::Unknown,
                    value_constraint: Evidence::Unknown,
                    resource_contents_constraint: (subject.public.kind
                        == SubjectKind::PostStateResource)
                        .then(|| subject.resource_contents.evidence_at(&[]).0),
                    input_output_relation: Evidence::Unknown,
                    constraint_coverage: None,
                    locations: Vec::new(),
                });
            }
        }
        out.sort_by(|left, right| left.subject.cmp(&right.subject));
        out
    }

    fn relation_summaries(&self) -> Vec<RelationSummary> {
        let mut out = Vec::new();
        for (key, evidence) in &self.relations {
            let (Origin::Subject(output), Origin::Subject(input)) =
                (&key.output.origin, &key.input.origin)
            else {
                continue;
            };
            let Some(coverage) = evidence.coverage.public() else {
                continue;
            };
            out.push(RelationSummary {
                output: with_projection(&self.subjects[output.0].public, &key.output.projection),
                input: with_projection(&self.subjects[input.0].public, &key.input.projection),
                kind: key.kind,
                coverage,
                locations: evidence.locations.iter().cloned().collect(),
            });
        }
        out.sort();
        out.dedup();
        out
    }

    fn diagnostics(&self, trivial: bool) -> Vec<SanityDiagnostic> {
        let mut out = Vec::new();
        if trivial {
            out.push(self.make_diagnostic(
                DiagnosticCode::TrivialPostcondition,
                None,
                "The postcondition is explicitly trivial.".into(),
                "The retained postcondition is an empty chain or the literal assertion `true`; this does not distinguish source omission from deliberate triviality."
                    .into(),
                vec![self.location(
                    SanityPhase::Postcondition,
                    format!("functions[{}].ensures", self.function_index),
                )],
            ));
            return out;
        }

        let return_unmentioned = self.return_key.is_some_and(|key| {
            let subject = &self.subjects[key.0];
            !subject.value.any_observed()
                && !subject.ownership.any_observed()
                && !subject.value.any_unknown()
                && !subject.ownership.any_unknown()
        });
        if return_unmentioned {
            let key = self.return_key.expect("checked above");
            out.push(self.make_diagnostic(
                DiagnosticCode::ReturnUnmentioned,
                Some(self.subjects[key.0].public.clone()),
                "The function's return value has no detected meaningful use in the postcondition."
                    .into(),
                "Neither a nontrivial value condition nor an ownership/shape request refers to the return value. Constant-output functions may legitimately do this."
                    .into(),
                Vec::new(),
            ));
        }

        for (key, subject) in self.subjects.iter().enumerate() {
            if !subject.output {
                continue;
            }
            let shape = projection_shape(
                &subject.bt,
                self.module,
                self.options.projection_depth.max(1),
            );
            if !shape.aggregate || shape.known.len() < 2 {
                continue;
            }
            let mut constrained = Vec::new();
            let mut uncovered = Vec::new();
            let mut unknown = Vec::new();
            for path in &shape.known {
                match subject.value.evidence_at(path).0 {
                    Evidence::Observed => constrained.push(path.clone()),
                    Evidence::NoEvidence => uncovered.push(path.clone()),
                    Evidence::Unknown => unknown.push(path.clone()),
                }
            }
            unknown.extend(shape.unknown.iter().cloned());
            if !constrained.is_empty() && !uncovered.is_empty() {
                let uncovered_text = uncovered
                    .iter()
                    .map(|path| projection_label(path))
                    .collect::<Vec<_>>()
                    .join(", ");
                let unknown_text = if unknown.is_empty() {
                    String::new()
                } else {
                    format!(
                        " Unknown projections were kept separate: {}.",
                        unknown
                            .iter()
                            .map(|path| projection_label(path))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                out.push(self.make_diagnostic(
                    DiagnosticCode::OutputPartiallyConstrained,
                    Some(self.subjects[key].public.clone()),
                    format!(
                        "Some aggregate output fields have value-constraint evidence, but these do not: {uncovered_text}."
                    ),
                    format!(
                        "Conditions cover at least one statically identified field and leave at least one other field without supported evidence.{unknown_text}"
                    ),
                    subject.value.locations.iter().cloned().collect(),
                ));
            }
        }

        for key in &self.post_resources {
            let subject = &self.subjects[key.0];
            if subject.content_relevant
                && !subject.value.any_observed()
                && !subject.value.any_unknown()
                && !subject.resource_contents.any_observed()
                && !subject.resource_contents.any_unknown()
            {
                out.push(self.make_diagnostic(
                    DiagnosticCode::ResourceContentsUnconstrained,
                    Some(subject.public.clone()),
                    format!(
                        "The postcondition accounts for {}'s ownership or shape but has no detected constraint on its initialized contents.",
                        subject.public.display_name()
                    ),
                    "The request and its predicate summary provide ownership/shape evidence only. Deliberately abstract or memory-safety-only specifications may be intentional."
                        .into(),
                    subject.ownership.locations.iter().cloned().collect(),
                ));
            }
        }

        let relevant_inputs = self
            .subjects
            .iter()
            .any(|subject| subject.input && subject.relevant && subject.content_relevant);
        let relevant_outputs = self.subjects.iter().any(|subject| {
            subject.output
                && subject.content_relevant
                && (subject.value.any_observed() || subject.ownership.any_observed())
        });
        let content_relation = self.relations.keys().any(|relation| {
            if relation.kind == RelationKind::AddressOnly {
                return false;
            }
            let (Origin::Subject(output), Origin::Subject(input)) =
                (&relation.output.origin, &relation.input.origin)
            else {
                return false;
            };
            self.subjects[output.0].content_relevant && self.subjects[input.0].content_relevant
        });
        let relation_unknown = self.subjects.iter().any(|subject| {
            subject.output && subject.content_relevant && subject.relation.any_unknown()
        });
        if relevant_inputs && relevant_outputs && !content_relation && !relation_unknown {
            let address_only = self.relations.keys().any(|relation| {
                if relation.kind != RelationKind::AddressOnly {
                    return false;
                }
                let (Origin::Subject(output), Origin::Subject(input)) =
                    (&relation.output.origin, &relation.input.origin)
                else {
                    return false;
                };
                self.subjects[output.0].content_relevant && self.subjects[input.0].content_relevant
            });
            out.push(self.make_diagnostic(
                DiagnosticCode::NoInputOutputRelation,
                None,
                "No supported value or control relation between outputs and inputs or pre-state observations was detected."
                    .into(),
                if address_only {
                    "The checker found an ownership/address-only relation, which does not establish any relation between stored contents."
                        .into()
                } else {
                    "Output observations and relevant inputs exist, but no supported condition connects their values. This is a syntactic approximation, not proof of a missing requirement."
                        .into()
                },
                Vec::new(),
            ));
        }

        if self.options.pedantic {
            for subject in &self.subjects {
                if !subject.input || !subject.relevant {
                    continue;
                }
                if subject.post_value_use.any_unknown() || subject.post_shape_use.any_unknown() {
                    continue;
                }
                let value = subject.post_value_use.any_observed();
                let shape = subject.post_shape_use.any_observed();
                if value {
                    continue;
                }
                let (message, explanation) = if shape {
                    (
                        format!(
                            "{} is used only for ownership or shape in the postcondition.",
                            subject.public.display_name()
                        ),
                        "No supported postcondition condition uses its value; address, permission, or bound use is reported separately."
                            .into(),
                    )
                } else {
                    (
                        format!(
                            "{} has no detected role in the postcondition.",
                            subject.public.display_name()
                        ),
                        "No value, control, ownership, or shape use was found. Consumption or deliberate abstraction may be intentional."
                            .into(),
                    )
                };
                out.push(self.make_diagnostic(
                    DiagnosticCode::InputNotUsedInPost,
                    Some(subject.public.clone()),
                    message,
                    explanation,
                    Vec::new(),
                ));
            }
        }
        out
    }

    fn make_diagnostic(
        &self,
        code: DiagnosticCode,
        subject: Option<Subject>,
        message: String,
        explanation: String,
        mut locations: Vec<IrLocation>,
    ) -> SanityDiagnostic {
        locations.sort();
        locations.dedup();
        SanityDiagnostic {
            code,
            severity: code.severity(),
            function: self.function.name.clone(),
            subject,
            message,
            explanation,
            locations,
            suppressed: self.options.suppress.contains(&code),
        }
    }
}

fn request_has_content(request: &Request, bt: &BaseType) -> bool {
    if *bt == BaseType::Unit {
        return false;
    }
    match request.name() {
        RequestName::Owned(_, Init::Uninit) => false,
        RequestName::Owned(_, Init::Init) | RequestName::PName(_) => true,
    }
}

fn with_projection(subject: &Subject, projection: &[String]) -> Subject {
    let mut subject = subject.clone();
    subject.projection = projection.to_vec();
    subject
}

fn projection_label(path: &[String]) -> String {
    if path.is_empty() {
        return "<whole value>".into();
    }
    let mut out = String::new();
    for part in path {
        if part.starts_with('[') || part.starts_with('<') {
            out.push_str(part);
        } else {
            if !out.is_empty() {
                out.push('.');
            }
            out.push_str(part);
        }
    }
    out
}

fn ctype_base_type(ty: &CType, module: &SpecModule) -> Option<BaseType> {
    let tag = |name: &str| {
        module
            .types
            .iter()
            .find(|def| def.name == name)
            .and_then(|def| def.tag.clone())
    };
    ty.to_sctype(&tag)?.to_base_type()
}

#[derive(Debug, Default)]
struct ProjectionShape {
    known: BTreeSet<Projection>,
    unknown: BTreeSet<Projection>,
    aggregate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ShapeMarker {
    Struct(Sym),
    Datatype(Sym),
}

fn projection_shape(bt: &BaseType, module: &SpecModule, limit: usize) -> ProjectionShape {
    fn walk(
        bt: &BaseType,
        module: &SpecModule,
        limit: usize,
        path: &mut Projection,
        stack: &mut BTreeSet<ShapeMarker>,
        shape: &mut ProjectionShape,
    ) {
        if path.len() >= limit {
            shape.unknown.insert(path.clone());
            return;
        }
        match bt {
            BaseType::Record(fields) => {
                shape.aggregate = true;
                if fields.is_empty() {
                    shape.known.insert(path.clone());
                }
                for (field, bt) in fields {
                    path.push(field.0.clone());
                    walk(bt, module, limit, path, stack, shape);
                    path.pop();
                }
            }
            BaseType::Tuple(fields) => {
                shape.aggregate = true;
                if fields.is_empty() {
                    shape.known.insert(path.clone());
                }
                for (index, bt) in fields.iter().enumerate() {
                    path.push(format!("[{index}]"));
                    walk(bt, module, limit, path, stack, shape);
                    path.pop();
                }
            }
            BaseType::Struct(tag) => {
                shape.aggregate = true;
                let marker = ShapeMarker::Struct(tag.clone());
                if !stack.insert(marker.clone()) {
                    shape.unknown.insert(path.clone());
                    return;
                }
                let Some(definition) = module
                    .types
                    .iter()
                    .find(|definition| definition.tag.as_ref() == Some(tag))
                else {
                    shape.unknown.insert(path.clone());
                    stack.remove(&marker);
                    return;
                };
                if definition.fields.is_empty() {
                    shape.known.insert(path.clone());
                }
                for field in &definition.fields {
                    path.push(field.name.clone());
                    if let Some(bt) = ctype_base_type(&field.ty, module) {
                        walk(&bt, module, limit, path, stack, shape);
                    } else {
                        shape.unknown.insert(path.clone());
                    }
                    path.pop();
                }
                stack.remove(&marker);
            }
            BaseType::Datatype(name) => {
                shape.aggregate = true;
                let marker = ShapeMarker::Datatype(name.clone());
                if !stack.insert(marker.clone()) {
                    shape.unknown.insert(path.clone());
                    return;
                }
                let Some(datatype) = module.datatype(name) else {
                    shape.unknown.insert(path.clone());
                    stack.remove(&marker);
                    return;
                };
                if datatype.constructors.is_empty() {
                    shape.unknown.insert(path.clone());
                }
                for constructor in &datatype.constructors {
                    path.push(constructor_projection(&constructor.name));
                    if constructor.params.is_empty() {
                        shape.known.insert(path.clone());
                    }
                    for (field, bt) in &constructor.params {
                        path.push(field.0.clone());
                        walk(bt, module, limit, path, stack, shape);
                        path.pop();
                    }
                    path.pop();
                }
                stack.remove(&marker);
            }
            BaseType::Map(_, value) | BaseType::List(value) | BaseType::Set(value) => {
                shape.aggregate = true;
                path.push("[*]".into());
                walk(value, module, limit, path, stack, shape);
                path.pop();
            }
            BaseType::Option(value) => {
                shape.aggregate = true;
                path.push("some".into());
                walk(value, module, limit, path, stack, shape);
                path.pop();
            }
            BaseType::Unit => {
                shape.known.insert(path.clone());
            }
            _ => {
                shape.known.insert(path.clone());
            }
        }
    }

    let mut shape = ProjectionShape::default();
    walk(
        bt,
        module,
        limit,
        &mut Vec::new(),
        &mut BTreeSet::new(),
        &mut shape,
    );
    shape
}
