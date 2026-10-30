use std::collections::BTreeSet;
use std::fmt;

use crate::gen_dsl::choice::tree::{Label, ScopeKey};
use crate::gen_dsl::lang::gen_def::GenDef;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind, ScopeName};
use crate::gen_dsl::lang::sym::{Sym, SymSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyClass {
    Str(String),
    Index(i128),
    AnyIndex,
    Sid(u32),
}

impl KeyClass {
    pub fn collides_with(&self, other: &KeyClass) -> bool {
        use KeyClass::{AnyIndex, Index, Sid, Str};
        match (self, other) {
            (Str(a), Str(b)) => a == b,
            (Sid(a), Sid(b)) => a == b,
            (AnyIndex, AnyIndex) | (AnyIndex, Index(_)) | (Index(_), AnyIndex) => true,
            (Index(a), Index(b)) => a == b,
            (Str(_), Index(_) | AnyIndex | Sid(_))
            | (Index(_) | AnyIndex | Sid(_), Str(_))
            | (Sid(_), Index(_) | AnyIndex)
            | (Index(_) | AnyIndex, Sid(_)) => false,
        }
    }
}

impl fmt::Display for KeyClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyClass::Str(s) => write!(f, "{s:?}"),
            KeyClass::Index(i) => write!(f, "{i}"),
            KeyClass::AnyIndex => f.write_str("some integer index"),
            KeyClass::Sid(num) => write!(f, "#{num}"),
        }
    }
}

impl From<ScopeKey> for KeyClass {
    fn from(key: ScopeKey) -> Self {
        match key {
            ScopeKey::Str(s) => KeyClass::Str(s),
            ScopeKey::Index(i) => KeyClass::Index(i),
            ScopeKey::Sid(sid) => KeyClass::Sid(sid.num),
        }
    }
}

pub fn scope_class(term: &GenTerm) -> Option<KeyClass> {
    match &term.term {
        GenTermKind::Scope { name, .. } => {
            Some(name.const_key().map_or(KeyClass::AnyIndex, KeyClass::from))
        }
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScopeError {
    #[error("term writes the label {} twice in one scope", .label.0)]
    DuplicateLabel { label: Label },
    #[error("term can open the scope key {claim} twice")]
    DuplicateScope { claim: KeyClass },
}

impl GenTerm {
    pub fn well_scoped(&self) -> Result<(), ScopeError> {
        check_scope::<Claims>(self)
    }

    pub fn well_scoped_reopenable(&self) -> Result<(), ScopeError> {
        check_scope::<ReopenClaims>(self)
    }
}

impl GenDef {
    pub fn well_scoped(&self) -> Result<(), ScopeError> {
        self.body.well_scoped()
    }

    pub fn well_scoped_reopenable(&self) -> Result<(), ScopeError> {
        self.body.well_scoped_reopenable()
    }
}

#[derive(Clone, Default)]
struct Claims {
    labels: BTreeSet<Label>,
    scopes: Vec<KeyClass>,
}

fn per_iteration(name: &ScopeName, repeats: &SymSet) -> bool {
    repeats.is_empty() || repeats.is_subset(&name.free_vars())
}

static NO_REPEATS: SymSet = SymSet::new();

fn with_binder(repeats: &SymSet, var: &Sym) -> SymSet {
    let mut inner = repeats.clone();
    inner.insert(var.clone());
    inner
}

#[derive(Clone)]
struct ReopenScope {
    name: ScopeName,
    class: KeyClass,
    claims: ReopenClaims,
}

#[derive(Clone, Default)]
struct ReopenClaims {
    labels: BTreeSet<Label>,
    scopes: Vec<ReopenScope>,
}

trait Claiming: Clone + Default {
    fn labels(&mut self) -> &mut BTreeSet<Label>;

    fn write(&mut self, label: Label) -> Result<(), ScopeError> {
        if self.labels().insert(label) {
            Ok(())
        } else {
            Err(ScopeError::DuplicateLabel { label })
        }
    }

    fn enter_scope<F>(
        &mut self,
        name: &ScopeName,
        class: KeyClass,
        repeated: bool,
        body: F,
    ) -> Result<(), ScopeError>
    where
        F: FnOnce(&mut Self) -> Result<(), ScopeError>;

    fn merge_alternative(&mut self, other: Self);
}

impl Claiming for Claims {
    fn labels(&mut self) -> &mut BTreeSet<Label> {
        &mut self.labels
    }

    fn enter_scope<F>(
        &mut self,
        _name: &ScopeName,
        class: KeyClass,
        repeated: bool,
        body: F,
    ) -> Result<(), ScopeError>
    where
        F: FnOnce(&mut Self) -> Result<(), ScopeError>,
    {
        if repeated {
            return Err(ScopeError::DuplicateScope { claim: class });
        }
        if let Some(claim) = self.scopes.iter().find(|claim| claim.collides_with(&class)) {
            return Err(ScopeError::DuplicateScope {
                claim: claim.clone(),
            });
        }
        self.scopes.push(class);

        let mut child = Self::default();
        body(&mut child)
    }

    fn merge_alternative(&mut self, other: Self) {
        self.labels.extend(other.labels);
        for claim in other.scopes {
            if !self.scopes.contains(&claim) {
                self.scopes.push(claim);
            }
        }
    }
}

impl Claiming for ReopenClaims {
    fn labels(&mut self) -> &mut BTreeSet<Label> {
        &mut self.labels
    }

    fn enter_scope<F>(
        &mut self,
        name: &ScopeName,
        class: KeyClass,
        _repeated: bool,
        body: F,
    ) -> Result<(), ScopeError>
    where
        F: FnOnce(&mut Self) -> Result<(), ScopeError>,
    {
        let exact = self.scopes.iter().position(|scope| &scope.name == name);
        if let Some(conflict) = self.scopes.iter().enumerate().find_map(|(at, scope)| {
            (Some(at) != exact && scope.class.collides_with(&class)).then_some(scope.class.clone())
        }) {
            return Err(ScopeError::DuplicateScope { claim: conflict });
        }
        let at = match exact {
            Some(at) => at,
            None => {
                self.scopes.push(ReopenScope {
                    name: name.clone(),
                    class,
                    claims: Self::default(),
                });
                self.scopes.len() - 1
            }
        };

        body(&mut self.scopes[at].claims)
    }

    fn merge_alternative(&mut self, other: Self) {
        self.labels.extend(other.labels);
        for other_scope in other.scopes {
            if let Some(scope) = self
                .scopes
                .iter_mut()
                .find(|scope| scope.name == other_scope.name)
            {
                scope.claims.merge_alternative(other_scope.claims);
            } else {
                self.scopes.push(other_scope);
            }
        }
    }
}

fn check_scope<C: Claiming>(term: &GenTerm) -> Result<(), ScopeError> {
    check_in(term, &mut C::default(), &NO_REPEATS)
}

fn check_in<C: Claiming>(
    term: &GenTerm,
    claims: &mut C,
    repeats: &SymSet,
) -> Result<(), ScopeError> {
    match &term.term {
        GenTermKind::Arbitrary => Ok(()),
        GenTermKind::WriteChoice { label, rest, .. } => {
            if !repeats.is_empty() {
                return Err(ScopeError::DuplicateLabel { label: *label });
            }
            claims.write(*label)?;
            check_in(rest, claims, repeats)
        }
        GenTermKind::LetStar { value, rest, .. } => {
            check_in(value, claims, repeats)?;
            check_in(rest, claims, repeats)
        }
        GenTermKind::Asgn { rest, .. } | GenTermKind::Assert { rest, .. } => {
            check_in(rest, claims, repeats)
        }
        GenTermKind::Return(_) | GenTermKind::Call { .. } => Ok(()),
        GenTermKind::Ite { then_, else_, .. } => {
            check_alternatives(claims, repeats, [then_.as_ref(), else_.as_ref()])
        }
        GenTermKind::Map { var, body, .. } => check_in(body, claims, &with_binder(repeats, var)),
        GenTermKind::Scope { name, body } => {
            let class = scope_class(term).expect("an explicit scope has a key class");
            let discharged = per_iteration(name, repeats);
            let child_repeats = if discharged { &NO_REPEATS } else { repeats };
            claims.enter_scope(name, class, !discharged, |child| {
                check_in(body, child, child_repeats)
            })
        }
    }
}

fn check_alternatives<'a, C: Claiming>(
    claims: &mut C,
    repeats: &SymSet,
    alts: impl IntoIterator<Item = &'a GenTerm>,
) -> Result<(), ScopeError> {
    let incoming = claims.clone();
    for alt in alts {
        let mut alt_claims = incoming.clone();
        check_in(alt, &mut alt_claims, repeats)?;
        claims.merge_alternative(alt_claims);
    }
    Ok(())
}
