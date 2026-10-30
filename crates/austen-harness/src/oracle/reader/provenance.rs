use austen_lib::spec::{
    FunctionBody, Id, IndexTerm, Pattern, PatternKind, PointerOrigin, Scalar, SpecModule, SymMap,
    Term, Value,
};

use super::{MemoryRegion, RootSet, merge_roots, pointer_origin};
use austen_lib::spec::eval::{
    MAX_DEPTH, boolean, eval_with_defaults as eval, integer, pattern_bindings,
};

#[derive(Debug, Clone)]
pub(super) enum ValueProvenance {
    Roots(RootSet),
    Fields(Vec<(Id, ValueProvenance)>),
    Map {
        entries: Vec<(i128, ValueProvenance)>,
        default: Option<Box<ValueProvenance>>,
    },
}

impl Default for ValueProvenance {
    fn default() -> Self {
        Self::Roots(Vec::new())
    }
}

impl ValueProvenance {
    pub(super) fn from_value(value: &Value, roots: &[u64]) -> Self {
        Self::from_value_at(value, roots, 0)
    }

    fn from_value_at(value: &Value, roots: &[u64], depth: usize) -> Self {
        if roots.is_empty() {
            return Self::default();
        }
        if depth >= MAX_DEPTH {
            return Self::Roots(value_roots(value, roots));
        }
        match value {
            Value::Scalar(Scalar::Pointer {
                addr,
                origin: PointerOrigin::Heap,
                ..
            }) if roots.contains(addr) => Self::Roots(vec![*addr]),
            Value::Unit | Value::Scalar(_) => Self::default(),
            Value::Struct { fields, .. }
            | Value::Record(fields)
            | Value::Constructor { fields, .. } => Self::Fields(
                fields
                    .iter()
                    .map(|(id, value)| (id.clone(), Self::from_value_at(value, roots, depth + 1)))
                    .collect(),
            ),
            Value::Map { entries, default } => Self::Map {
                entries: entries
                    .iter()
                    .map(|(key, value)| (*key, Self::from_value_at(value, roots, depth + 1)))
                    .collect(),
                default: default
                    .as_deref()
                    .map(|value| Box::new(Self::from_value_at(value, roots, depth + 1))),
            },
        }
    }

    pub(super) fn roots(&self) -> RootSet {
        let mut roots = Vec::new();
        let mut pending = vec![self];
        while let Some(value) = pending.pop() {
            match value {
                Self::Roots(found) => roots.extend_from_slice(found),
                Self::Fields(fields) => pending.extend(fields.iter().map(|(_, value)| value)),
                Self::Map { entries, default } => {
                    pending.extend(entries.iter().map(|(_, value)| value));
                    pending.extend(default.as_deref());
                }
            }
        }
        roots.sort_unstable();
        roots.dedup();
        roots
    }

    fn field(&self, member: &Id) -> Self {
        match self {
            Self::Fields(fields) => fields
                .iter()
                .find(|(id, _)| id == member)
                .map(|(_, value)| value.clone())
                .unwrap_or_default(),
            _ => Self::Roots(self.roots()),
        }
    }
}

fn value_roots(value: &Value, allocation_roots: &[u64]) -> RootSet {
    let mut roots = value
        .scalars()
        .filter_map(|scalar| scalar.heap_address())
        .filter(|addr| allocation_roots.contains(addr))
        .collect::<Vec<_>>();
    roots.sort_unstable();
    roots.dedup();
    roots
}

pub(super) fn seed_provenance(env: &SymMap<Value>, roots: &[u64]) -> SymMap<ValueProvenance> {
    if roots.is_empty() {
        return SymMap::new();
    }
    env.iter()
        .map(|(symbol, value)| (symbol.clone(), ValueProvenance::from_value(value, roots)))
        .collect()
}

pub(super) fn term_provenance(
    term: &IndexTerm,
    env: &SymMap<Value>,
    provenance: &SymMap<ValueProvenance>,
    module: &SpecModule,
    regions: &[MemoryRegion],
    roots: &[u64],
    defaults: &austen_lib::spec::DefaultChoices,
) -> ValueProvenance {
    if roots.is_empty() {
        return ValueProvenance::default();
    }
    Tracker {
        defaults,
        module,
        regions,
        roots,
    }
    .term(term, env, provenance, 0)
}

struct Tracker<'a> {
    defaults: &'a austen_lib::spec::DefaultChoices,
    module: &'a SpecModule,
    regions: &'a [MemoryRegion],
    roots: &'a [u64],
}

impl Tracker<'_> {
    fn eval(&self, term: &IndexTerm, env: &SymMap<Value>) -> Option<Value> {
        eval(
            term,
            env,
            self.module,
            &|addr| pointer_origin(addr, self.regions),
            self.defaults,
        )
    }

    fn term(
        &self,
        term: &IndexTerm,
        env: &SymMap<Value>,
        provenance: &SymMap<ValueProvenance>,
        depth: usize,
    ) -> ValueProvenance {
        let fallback = || self.fallback(term, env, provenance);
        if depth >= MAX_DEPTH {
            return fallback();
        }
        let child = |term| self.term(term, env, provenance, depth + 1);
        match &term.term {
            Term::Sym(symbol) => provenance.get(symbol).cloned().unwrap_or_else(|| {
                env.get(symbol)
                    .map(|value| ValueProvenance::from_value(value, self.roots))
                    .unwrap_or_default()
            }),
            Term::MemberShift(base, _, _) | Term::ArrayShift { base, .. } | Term::Cast(_, base) => {
                ValueProvenance::Roots(child(base).roots())
            }
            Term::Struct(_, fields) | Term::Record(fields) | Term::Constructor(_, fields) => {
                ValueProvenance::Fields(
                    fields
                        .iter()
                        .map(|(id, term)| (id.clone(), child(term)))
                        .collect(),
                )
            }
            Term::StructMember(target, member) | Term::RecordMember(target, member) => {
                child(target).field(member)
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
                let ValueProvenance::Fields(mut fields) = child(target) else {
                    return fallback();
                };
                let Some((_, origin)) = fields.iter_mut().find(|(id, _)| id == member) else {
                    return fallback();
                };
                *origin = child(value);
                ValueProvenance::Fields(fields)
            }
            Term::MapConst(_, value) => ValueProvenance::Map {
                entries: Vec::new(),
                default: Some(Box::new(child(value))),
            },
            Term::MapSet(target, key, value) => {
                let ValueProvenance::Map {
                    mut entries,
                    default,
                } = child(target)
                else {
                    return fallback();
                };
                let Some(key) = self.eval(key, env).and_then(integer) else {
                    return fallback();
                };
                let origin = child(value);
                match entries.binary_search_by_key(&key, |(key, _)| *key) {
                    Ok(at) => entries[at].1 = origin,
                    Err(at) => entries.insert(at, (key, origin)),
                }
                ValueProvenance::Map { entries, default }
            }
            Term::MapGet(target, key) => {
                let origin = child(target);
                let Some(key) = self.eval(key, env).and_then(integer) else {
                    return fallback();
                };
                match origin {
                    ValueProvenance::Map { entries, default } => entries
                        .binary_search_by_key(&key, |(key, _)| *key)
                        .ok()
                        .map(|at| entries[at].1.clone())
                        .or_else(|| default.map(|value| *value))
                        .unwrap_or_default(),
                    other => ValueProvenance::Roots(other.roots()),
                }
            }
            Term::Ite(condition, yes, no) => match self.eval(condition, env).and_then(boolean) {
                Some(true) => child(yes),
                Some(false) => child(no),
                None => fallback(),
            },
            Term::Let((symbol, bound), body) => {
                let Some(value) = self.eval(bound, env) else {
                    return fallback();
                };
                let origin = child(bound);
                let mut nested_env = env.clone();
                let mut nested_provenance = provenance.clone();
                nested_env.insert(symbol.clone(), value);
                nested_provenance.insert(symbol.clone(), origin);
                self.term(body, &nested_env, &nested_provenance, depth + 1)
            }
            Term::Apply(name, args) => {
                let Some(function) = self.module.logical_function(name) else {
                    return fallback();
                };
                let Some(FunctionBody::Def(body) | FunctionBody::RecDef(body)) = &function.body
                else {
                    return fallback();
                };
                if function.args.len() != args.len() {
                    return fallback();
                }
                let mut nested_env = SymMap::new();
                let mut nested_provenance = SymMap::new();
                for ((formal, _), actual) in function.args.iter().zip(args) {
                    let Some(value) = self.eval(actual, env) else {
                        return fallback();
                    };
                    nested_env.insert(formal.clone(), value);
                    nested_provenance.insert(formal.clone(), child(actual));
                }
                self.term(body, &nested_env, &nested_provenance, depth + 1)
            }
            Term::Match(scrutinee, arms) => {
                let Some(value @ Value::Constructor { .. }) = self.eval(scrutinee, env) else {
                    return fallback();
                };
                for (pattern, body) in arms {
                    let mut bindings = Vec::new();
                    if !pattern_bindings(pattern, &value, &mut bindings) {
                        continue;
                    }
                    let mut nested_env = env.clone();
                    nested_env.extend(
                        bindings
                            .into_iter()
                            .map(|(symbol, value)| (symbol.clone(), value.clone())),
                    );
                    let mut nested_provenance = provenance.clone();
                    bind_pattern_provenance(pattern, &child(scrutinee), &mut nested_provenance);
                    return self.term(body, &nested_env, &nested_provenance, depth + 1);
                }
                fallback()
            }
            _ => fallback(),
        }
    }

    fn fallback(
        &self,
        term: &IndexTerm,
        env: &SymMap<Value>,
        provenance: &SymMap<ValueProvenance>,
    ) -> ValueProvenance {
        let mut roots = Vec::new();
        for symbol in term.free_vars() {
            if let Some(origin) = provenance.get(&symbol) {
                merge_roots(&mut roots, origin.roots());
            } else if let Some(value) = env.get(&symbol) {
                merge_roots(
                    &mut roots,
                    ValueProvenance::from_value(value, self.roots).roots(),
                );
            }
        }
        if let Some(value) = self.eval(term, env) {
            merge_roots(
                &mut roots,
                ValueProvenance::from_value(&value, self.roots).roots(),
            );
        }
        ValueProvenance::Roots(roots)
    }
}

fn bind_pattern_provenance(
    pattern: &Pattern,
    origin: &ValueProvenance,
    provenance: &mut SymMap<ValueProvenance>,
) {
    match &pattern.pattern {
        PatternKind::Sym(symbol) => {
            provenance.insert(symbol.clone(), origin.clone());
        }
        PatternKind::Wild => {}
        PatternKind::Constructor(_, members) => {
            for (id, pattern) in members {
                bind_pattern_provenance(pattern, &origin.field(id), provenance);
            }
        }
    }
}
