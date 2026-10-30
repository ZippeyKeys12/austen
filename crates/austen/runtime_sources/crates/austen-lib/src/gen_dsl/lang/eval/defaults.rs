use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use super::Evaluator;
use crate::gen_dsl::choice::draws::{ChoiceTape, Draw, DrawSource, Site};
use crate::gen_dsl::{BaseType, GenModule, Sign, SplitMix64, TestCaseSize, Value};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DefaultError {
    #[error("input defaults do not support base type {bt:?}")]
    UnsupportedBaseType { bt: BaseType },
    #[error("input-default width {bits} is not representable")]
    UnrepresentableWidth { bits: u16 },
    #[error("input has no default choice for base type {bt:?}")]
    MissingChoice { bt: BaseType },
    #[error("input supplied conflicting default choices for base type {bt:?}")]
    ConflictingChoice { bt: BaseType },
    #[error("input-default value {value} does not fit {bits} unsigned bits for {bt:?}")]
    ValueOutOfRange {
        bt: BaseType,
        value: i128,
        bits: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DefaultChoice {
    ty: BaseType,
    raw: i128,
}

impl DefaultChoice {
    pub fn width_for(ty: &BaseType) -> Result<u16, DefaultError> {
        match ty {
            BaseType::Bool => Ok(1),
            BaseType::Bits(sign, bits)
                if *bits <= u16::from(u8::MAX) && (*sign != Sign::Signed || *bits < 127) =>
            {
                Ok(*bits)
            }
            BaseType::Bits(_, bits) => Err(DefaultError::UnrepresentableWidth { bits: *bits }),
            _ => Err(DefaultError::UnsupportedBaseType { bt: ty.clone() }),
        }
    }

    pub fn new(ty: BaseType, raw: i128) -> Result<Self, DefaultError> {
        let bits = Self::width_for(&ty)?;
        if raw < 0 || (bits < 127 && raw >= 1i128 << bits) {
            return Err(DefaultError::ValueOutOfRange {
                bt: ty,
                value: raw,
                bits,
            });
        }
        Ok(Self { ty, raw })
    }

    pub fn ty(&self) -> &BaseType {
        &self.ty
    }
    pub fn raw(&self) -> i128 {
        self.raw
    }
    pub fn width(&self) -> u16 {
        Self::width_for(&self.ty).expect("validated default type")
    }
    pub fn as_value(&self) -> Value {
        crate::gen_dsl::choice::translate::scalar_value(self.raw, &self.ty)
            .expect("validated scalar default")
    }
}

impl<'de> Deserialize<'de> for DefaultChoice {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            ty: BaseType,
            raw: i128,
        }
        let raw = Raw::deserialize(deserializer)?;
        Self::new(raw.ty, raw.raw).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DefaultChoices(BTreeMap<BaseType, DefaultChoice>);

impl Hash for DefaultChoice {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match &self.ty {
            BaseType::Bool => 0u8.hash(state),
            BaseType::Bits(sign, bits) => {
                1u8.hash(state);
                (*sign == Sign::Signed).hash(state);
                bits.hash(state);
            }
            _ => unreachable!("validated default type"),
        }
        self.raw.hash(state);
    }
}

impl Hash for DefaultChoices {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.len().hash(state);
        for choice in self.iter() {
            choice.hash(state);
        }
    }
}

impl DefaultChoices {
    pub const fn new() -> Self {
        Self(BTreeMap::new())
    }
    pub fn get(&self, ty: &BaseType) -> Option<&DefaultChoice> {
        self.0.get(ty)
    }
    pub fn get_value(&self, ty: &BaseType) -> Option<Value> {
        self.get(ty).map(DefaultChoice::as_value)
    }
    pub fn iter(&self) -> impl Iterator<Item = &DefaultChoice> {
        self.0.values()
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn insert(&mut self, choice: DefaultChoice) -> Option<DefaultChoice> {
        self.0.insert(choice.ty.clone(), choice)
    }
    fn merge(&mut self, choices: &Self) -> Result<(), DefaultError> {
        for choice in choices.iter() {
            DefaultChoice::new(choice.ty.clone(), choice.raw)?;
            if self.get(&choice.ty).is_some_and(|old| old != choice) {
                return Err(DefaultError::ConflictingChoice {
                    bt: choice.ty.clone(),
                });
            }
        }
        self.0.extend(choices.0.clone());
        Ok(())
    }
}

impl Serialize for DefaultChoices {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.values().collect::<Vec<_>>().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DefaultChoices {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut choices = Self::new();
        for choice in Vec::<DefaultChoice>::deserialize(deserializer)? {
            if choices.insert(choice).is_some() {
                return Err(D::Error::custom("duplicate input default type"));
            }
        }
        Ok(choices)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct DefaultTypes(Vec<BaseType>);

impl DefaultTypes {
    pub const fn new() -> Self {
        Self(Vec::new())
    }
    pub fn as_slice(&self) -> &[BaseType] {
        &self.0
    }
    pub fn iter(&self) -> std::slice::Iter<'_, BaseType> {
        self.0.iter()
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn supported(types: BTreeSet<BaseType>) -> (Self, Option<DefaultError>) {
        let mut unsupported = None;
        let supported = types
            .into_iter()
            .filter(|ty| match DefaultChoice::width_for(ty) {
                Ok(_) => true,
                Err(error) => {
                    unsupported.get_or_insert(error);
                    false
                }
            })
            .collect();
        (Self(supported), unsupported)
    }
}

impl std::ops::Deref for DefaultTypes {
    type Target = [BaseType];
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}
impl<'a> IntoIterator for &'a DefaultTypes {
    type Item = &'a BaseType;
    type IntoIter = std::slice::Iter<'a, BaseType>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DefaultTypesError {
    #[error("input default types are not sorted and duplicate-free")]
    NonCanonical,
    #[error(transparent)]
    Unsupported(#[from] DefaultError),
}
impl TryFrom<Vec<BaseType>> for DefaultTypes {
    type Error = DefaultTypesError;
    fn try_from(types: Vec<BaseType>) -> Result<Self, Self::Error> {
        if types.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(DefaultTypesError::NonCanonical);
        }
        for ty in &types {
            DefaultChoice::width_for(ty)?;
        }
        Ok(Self(types))
    }
}
impl<'de> Deserialize<'de> for DefaultTypes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::try_from(Vec::<BaseType>::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

pub(super) trait Source {
    fn resolve(&self, ty: &BaseType) -> Result<Value, DefaultError>;
    fn choices(&self) -> DefaultChoices;
    fn merge(&self, choices: &DefaultChoices) -> Result<(), DefaultError>;
    fn size(&self) -> Option<TestCaseSize>;
    fn post_types(&self) -> DefaultTypes;
    fn error(&self) -> Option<DefaultError>;
}

pub(super) enum Runtime<'a> {
    Disabled,
    Sampled {
        size: TestCaseSize,
        rng: SplitMix64,
        choices: DefaultChoices,
        post: DefaultTypes,
    },
    Fixed(DefaultChoices),
    Shared(&'a dyn Source),
    Encoding {
        source: &'a dyn Source,
        emitted: BTreeSet<BaseType>,
        tape: ChoiceTape,
    },
    Decoding {
        draws: &'a mut dyn DrawSource,
        choices: DefaultChoices,
        post: DefaultTypes,
    },
}

impl std::fmt::Debug for Runtime<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Disabled => "Disabled",
            Self::Sampled { .. } => "Sampled",
            Self::Fixed(_) => "Fixed",
            Self::Shared(_) => "Shared",
            Self::Encoding { .. } => "Encoding",
            Self::Decoding { .. } => "Decoding",
        })
    }
}

impl<'a> Evaluator<'a> {
    pub fn sampled(
        module: &'a GenModule,
        size: TestCaseSize,
        seed: u64,
        post: &DefaultTypes,
    ) -> Result<Self, DefaultError> {
        let evaluator = Self {
            declarations: module.declarations(),
            defaults: RefCell::new(Runtime::Sampled {
                size,
                rng: SplitMix64::new(seed),
                choices: DefaultChoices::new(),
                post: post.clone(),
            }),
            default_error: RefCell::new(None),
        };
        for ty in post {
            evaluator.resolve_default(ty)?;
        }
        Ok(evaluator)
    }

    pub fn with_defaults(module: &'a GenModule, choices: DefaultChoices) -> Self {
        Self {
            declarations: module.declarations(),
            defaults: RefCell::new(Runtime::Fixed(choices)),
            default_error: RefCell::new(None),
        }
    }

    pub fn for_module<'b>(&'b self, module: &'b GenModule) -> Evaluator<'b> {
        Evaluator {
            declarations: module.declarations(),
            defaults: RefCell::new(Runtime::Shared(self)),
            default_error: RefCell::new(None),
        }
    }

    pub(in crate::gen_dsl) fn encoding<'b>(
        &'b self,
        module: &'b GenModule,
        choices: &DefaultChoices,
    ) -> Result<Evaluator<'b>, DefaultError> {
        self.merge_defaults(choices)?;
        Ok(Evaluator {
            declarations: module.declarations(),
            defaults: RefCell::new(Runtime::Encoding {
                source: self,
                emitted: BTreeSet::new(),
                tape: ChoiceTape::new(),
            }),
            default_error: RefCell::new(None),
        })
    }

    pub(in crate::gen_dsl) fn decoding(
        module: &'a GenModule,
        draws: &'a mut dyn DrawSource,
        post: &DefaultTypes,
    ) -> Self {
        Self {
            declarations: module.declarations(),
            defaults: RefCell::new(Runtime::Decoding {
                draws,
                choices: DefaultChoices::new(),
                post: post.clone(),
            }),
            default_error: RefCell::new(None),
        }
    }

    pub fn size(&self) -> Option<TestCaseSize> {
        match &*self.defaults.borrow() {
            Runtime::Sampled { size, .. } => Some(*size),
            Runtime::Shared(source) | Runtime::Encoding { source, .. } => source.size(),
            _ => None,
        }
    }

    pub fn default_choices(&self) -> DefaultChoices {
        match &*self.defaults.borrow() {
            Runtime::Sampled { choices, .. }
            | Runtime::Fixed(choices)
            | Runtime::Decoding { choices, .. } => choices.clone(),
            Runtime::Shared(source) | Runtime::Encoding { source, .. } => source.choices(),
            Runtime::Disabled => DefaultChoices::new(),
        }
    }

    pub fn default_error(&self) -> Option<DefaultError> {
        self.default_error
            .borrow()
            .clone()
            .or_else(|| match &*self.defaults.borrow() {
                Runtime::Shared(source) | Runtime::Encoding { source, .. } => source.error(),
                _ => None,
            })
    }

    pub fn merge_defaults(&self, values: &DefaultChoices) -> Result<(), DefaultError> {
        match &mut *self.defaults.borrow_mut() {
            Runtime::Sampled { choices, .. }
            | Runtime::Fixed(choices)
            | Runtime::Decoding { choices, .. } => choices.merge(values),
            Runtime::Shared(source) | Runtime::Encoding { source, .. } => source.merge(values),
            Runtime::Disabled if values.is_empty() => Ok(()),
            Runtime::Disabled => Err(DefaultError::MissingChoice {
                bt: values.iter().next().unwrap().ty.clone(),
            }),
        }
    }

    pub fn resolve_default(&self, ty: &BaseType) -> Result<Value, DefaultError> {
        let result = self.resolve_default_inner(ty);
        if let Err(error) = &result {
            self.default_error
                .borrow_mut()
                .get_or_insert_with(|| error.clone());
        }
        result
    }

    fn resolve_default_inner(&self, ty: &BaseType) -> Result<Value, DefaultError> {
        let width = DefaultChoice::width_for(ty)?;
        match &mut *self.defaults.borrow_mut() {
            Runtime::Disabled => Err(DefaultError::MissingChoice { bt: ty.clone() }),
            Runtime::Fixed(choices) => choices
                .get_value(ty)
                .ok_or_else(|| DefaultError::MissingChoice { bt: ty.clone() }),
            Runtime::Shared(source) => source.resolve(ty),
            Runtime::Sampled {
                choices, rng, size, ..
            } => {
                if let Some(value) = choices.get_value(ty) {
                    return Ok(value);
                }
                let raw = crate::gen_dsl::sized_scalar(rng, size.get(), ty)
                    .expect("validated scalar type")
                    .map_err(|_| DefaultError::UnrepresentableWidth { bits: width })?;
                let choice = DefaultChoice::new(ty.clone(), raw)?;
                let value = choice.as_value();
                choices.insert(choice);
                Ok(value)
            }
            Runtime::Encoding {
                source,
                emitted,
                tape,
            } => {
                let value = source.resolve(ty)?;
                if emitted.insert(ty.clone()) {
                    let raw = crate::gen_dsl::choice::translate::choice_value(&value, ty)
                        .ok_or(DefaultError::UnrepresentableWidth { bits: width })?;
                    tape.push(raw, width);
                }
                Ok(value)
            }
            Runtime::Decoding { draws, choices, .. } => {
                if let Some(value) = choices.get_value(ty) {
                    return Ok(value);
                }
                let raw = draws
                    .draw(Site::new(width))
                    .ok_or_else(|| DefaultError::MissingChoice { bt: ty.clone() })?
                    .at(width)
                    .value;
                let choice = DefaultChoice::new(ty.clone(), raw)?;
                let value = choice.as_value();
                choices.insert(choice);
                Ok(value)
            }
        }
    }

    pub(in crate::gen_dsl) fn push_choice(&self, raw: i128, bits: u16) {
        let Runtime::Encoding { tape, .. } = &mut *self.defaults.borrow_mut() else {
            panic!("choice emission requires an encoding evaluator")
        };
        tape.push(raw, bits);
    }

    pub(in crate::gen_dsl) fn draw_choice(&self, site: Site) -> Option<Draw> {
        let Runtime::Decoding { draws, .. } = &mut *self.defaults.borrow_mut() else {
            panic!("choice draws require a decoding evaluator")
        };
        draws.draw(site)
    }

    fn post_default_types(&self) -> DefaultTypes {
        match &*self.defaults.borrow() {
            Runtime::Sampled { post, .. } | Runtime::Decoding { post, .. } => post.clone(),
            Runtime::Shared(source) | Runtime::Encoding { source, .. } => source.post_types(),
            _ => DefaultTypes::new(),
        }
    }

    fn finish_defaults(&self) -> Result<(), DefaultError> {
        if let Some(error) = self.default_error() {
            return Err(error);
        }
        for ty in &self.post_default_types() {
            self.resolve_default(ty)?;
        }
        Ok(())
    }

    pub(in crate::gen_dsl) fn finish_encoding(self) -> Result<ChoiceTape, DefaultError> {
        self.finish_defaults()?;
        let Runtime::Encoding { tape, .. } = self.defaults.into_inner() else {
            panic!("finishing requires an encoding evaluator")
        };
        Ok(tape)
    }

    pub(in crate::gen_dsl) fn finish_decoding(self) -> Result<DefaultChoices, DefaultError> {
        self.finish_defaults()?;
        let Runtime::Decoding { choices, .. } = self.defaults.into_inner() else {
            panic!("finishing requires a decoding evaluator")
        };
        Ok(choices)
    }
}

impl Source for Evaluator<'_> {
    fn resolve(&self, ty: &BaseType) -> Result<Value, DefaultError> {
        self.resolve_default(ty)
    }
    fn choices(&self) -> DefaultChoices {
        self.default_choices()
    }
    fn merge(&self, choices: &DefaultChoices) -> Result<(), DefaultError> {
        self.merge_defaults(choices)
    }
    fn size(&self) -> Option<TestCaseSize> {
        self.size()
    }
    fn post_types(&self) -> DefaultTypes {
        self.post_default_types()
    }
    fn error(&self) -> Option<DefaultError> {
        self.default_error()
    }
}
