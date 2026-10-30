use std::collections::{BTreeMap, btree_map::Entry};
use std::fmt;

use crate::gen_dsl::DefaultChoices;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScopeMergeError {
    #[error("scope fragment writes label {} twice at path {path:?}", .label.0)]
    DuplicateChoice { path: Vec<ScopeKey>, label: Label },
}

impl ScopeMergeError {
    fn under(mut self, key: &ScopeKey) -> Self {
        let Self::DuplicateChoice { path, .. } = &mut self;
        path.insert(0, key.clone());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AttachError {
    #[error("a recording scope's dynamic key does not evaluate")]
    MissingKey,
    #[error(transparent)]
    Merge(ScopeMergeError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Label(pub u32);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeId {
    pub tag: Option<String>,
    pub num: u32,
}

impl PartialEq for ScopeId {
    fn eq(&self, other: &Self) -> bool {
        self.num == other.num
    }
}

impl Eq for ScopeId {}

impl PartialOrd for ScopeId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScopeId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.num.cmp(&other.num)
    }
}

impl std::hash::Hash for ScopeId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.num.hash(state);
    }
}

impl fmt::Display for ScopeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.tag {
            Some(tag) => write!(f, "{tag}#{}", self.num),
            None => write!(f, "#{}", self.num),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SiteGen {
    next: u32,
}

impl SiteGen {
    pub fn new() -> Self {
        Self::default()
    }

    fn fresh(&mut self) -> u32 {
        let num = self.next;
        self.next += 1;
        num
    }

    pub fn label(&mut self) -> Label {
        Label(self.fresh())
    }

    pub fn scope(&mut self, tag: impl Into<String>) -> ScopeId {
        ScopeId {
            tag: Some(tag.into()),
            num: self.fresh(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Choice {
    Scalar { value: i128, bits: u8 },
    Struct(Vec<Choice>),
    Array(Vec<Choice>),
}

impl Choice {
    pub fn scalar(&self) -> Option<(i128, u8)> {
        match self {
            Choice::Scalar { value, bits } => Some((*value, *bits)),
            Choice::Struct(_) | Choice::Array(_) => None,
        }
    }

    pub fn value(&self) -> i128 {
        self.scalar().expect("scalar choice").0
    }

    pub fn bits(&self) -> u8 {
        self.scalar().expect("scalar choice").1
    }

    pub fn members(&self) -> &[Choice] {
        match self {
            Choice::Scalar { .. } => &[],
            Choice::Struct(members) | Choice::Array(members) => members,
        }
    }

    pub fn at_path_mut(&mut self, path: &[usize]) -> Option<&mut Choice> {
        let mut node = self;
        for &index in path {
            node = match node {
                Choice::Scalar { .. } => return None,
                Choice::Struct(members) | Choice::Array(members) => members.get_mut(index)?,
            };
        }
        Some(node)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ScopeKey {
    Str(String),
    Index(i128),
    Sid(ScopeId),
}

impl fmt::Display for ScopeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScopeKey::Str(s) => write!(f, "{s:?}"),
            ScopeKey::Index(i) => write!(f, "{i}"),
            ScopeKey::Sid(sid) => write!(f, "{sid}"),
        }
    }
}

impl From<ScopeId> for ScopeKey {
    fn from(sid: ScopeId) -> Self {
        ScopeKey::Sid(sid)
    }
}

impl From<&str> for ScopeKey {
    fn from(s: &str) -> Self {
        ScopeKey::Str(s.to_owned())
    }
}

impl From<String> for ScopeKey {
    fn from(s: String) -> Self {
        ScopeKey::Str(s)
    }
}

impl From<i128> for ScopeKey {
    fn from(i: i128) -> Self {
        ScopeKey::Index(i)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Scope {
    pub choices: BTreeMap<Label, Choice>,
    pub scopes: BTreeMap<ScopeKey, Scope>,
}

impl Scope {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.choices.is_empty() && self.scopes.values().all(Scope::is_empty)
    }

    pub fn insert_choice(&mut self, label: Label, value: i128, bits: u8) {
        self.choices.insert(label, Choice::Scalar { value, bits });
    }

    pub fn insert(&mut self, label: Label, choice: Choice) {
        self.choices.insert(label, choice);
    }

    pub fn insert_scope(&mut self, key: impl Into<ScopeKey>, scope: Scope) {
        self.scopes.insert(key.into(), scope);
    }

    pub fn try_merge(&mut self, fragment: Scope) -> Result<(), ScopeMergeError> {
        let fragment = fragment.without_empty_descendants();
        if fragment.is_empty() {
            return Ok(());
        }
        self.find_merge_conflict(&fragment)?;
        self.merge_unchecked(fragment);
        Ok(())
    }

    pub fn try_attach(&mut self, key: Option<ScopeKey>, child: Scope) -> Result<(), AttachError> {
        if child.is_empty() {
            return Ok(());
        }
        let key = key.ok_or(AttachError::MissingKey)?;
        self.try_merge_scope(&key, child)
            .map_err(AttachError::Merge)
    }

    pub fn try_merge_scope(
        &mut self,
        key: &ScopeKey,
        fragment: Scope,
    ) -> Result<(), ScopeMergeError> {
        let fragment = fragment.without_empty_descendants();
        if fragment.is_empty() {
            return Ok(());
        }
        match self.scopes.get_mut(key) {
            Some(existing) => {
                existing
                    .find_merge_conflict(&fragment)
                    .map_err(|conflict| conflict.under(key))?;
                existing.merge_unchecked(fragment);
            }
            None => {
                self.scopes.insert(key.clone(), fragment);
            }
        }
        Ok(())
    }

    fn find_merge_conflict(&self, fragment: &Scope) -> Result<(), ScopeMergeError> {
        if let Some(label) = fragment
            .choices
            .keys()
            .find(|label| self.choices.contains_key(label))
        {
            return Err(ScopeMergeError::DuplicateChoice {
                path: Vec::new(),
                label: *label,
            });
        }
        for (key, child) in &fragment.scopes {
            if child.is_empty() {
                continue;
            }
            if let Some(existing) = self.scopes.get(key) {
                existing
                    .find_merge_conflict(child)
                    .map_err(|conflict| conflict.under(key))?;
            }
        }
        Ok(())
    }

    fn merge_unchecked(&mut self, fragment: Scope) {
        self.choices.extend(fragment.choices);
        for (key, child) in fragment.scopes {
            if child.is_empty() {
                continue;
            }
            match self.scopes.entry(key) {
                Entry::Occupied(mut entry) => {
                    entry.get_mut().merge_unchecked(child);
                }
                Entry::Vacant(entry) => {
                    entry.insert(child);
                }
            }
        }
    }

    fn without_empty_descendants(mut self) -> Self {
        if !self.has_empty_descendant() {
            return self;
        }
        self.scopes = self
            .scopes
            .into_iter()
            .filter_map(|(key, child)| {
                let child = child.without_empty_descendants();
                (!child.is_empty()).then_some((key, child))
            })
            .collect();
        self
    }

    fn has_empty_descendant(&self) -> bool {
        self.scopes
            .values()
            .any(|child| child.is_empty() || child.has_empty_descendant())
    }

    pub fn child_tagged(&self, tag: &str) -> Option<&Scope> {
        self.scopes.iter().find_map(|(key, child)| match key {
            ScopeKey::Str(s) if s == tag => Some(child),
            ScopeKey::Sid(sid) if sid.tag.as_deref() == Some(tag) => Some(child),
            _ => None,
        })
    }

    pub fn remove_choice(&mut self, label: Label) {
        self.choices.remove(&label);
    }

    pub fn with_choice(mut self, label: Label, value: i128, bits: u8) -> Self {
        self.insert_choice(label, value, bits);
        self
    }

    pub fn with_scope(mut self, key: impl Into<ScopeKey>, scope: Scope) -> Self {
        self.insert_scope(key, scope);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChoiceTree {
    pub root: Scope,
    #[serde(default)]
    pub defaults: DefaultChoices,
}

impl ChoiceTree {
    pub fn new(root: Scope) -> Self {
        Self {
            root,
            defaults: DefaultChoices::new(),
        }
    }

    pub fn with_defaults(mut self, defaults: DefaultChoices) -> Self {
        self.defaults = defaults;
        self
    }

    pub fn scalar(label: Label, value: i128, bits: u8) -> Self {
        Self::new(Scope::new().with_choice(label, value, bits))
    }

    pub fn sparse(&self) -> Self {
        Self::new(self.root.clone().without_empty_descendants())
            .with_defaults(self.defaults.clone())
    }
}
