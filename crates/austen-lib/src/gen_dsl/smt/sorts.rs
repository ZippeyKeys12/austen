use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::gen_dsl::{BaseType, DatatypeDef, LogicalFunctionDef, Sign, StructDef, Sym};
use z3::datatype_builder::create_datatypes;
use z3::{DatatypeAccessor, DatatypeBuilder, DatatypeSort, Sort};

use super::error::SmtError;
use super::names;
use super::pointers::PointerEncoding;

#[derive(Debug, Clone, Copy, Default)]
pub struct Program<'p> {
    structs: &'p [(Sym, StructDef)],
    functions: &'p [(Sym, LogicalFunctionDef)],
    datatypes: &'p [(Sym, DatatypeDef)],
}

impl<'p> Program<'p> {
    pub fn new(structs: &'p [(Sym, StructDef)]) -> Self {
        Program {
            structs,
            functions: &[],
            datatypes: &[],
        }
    }

    #[must_use]
    pub fn and_functions(self, functions: &'p [(Sym, LogicalFunctionDef)]) -> Self {
        Program { functions, ..self }
    }

    #[must_use]
    pub fn and_datatypes(self, datatypes: &'p [(Sym, DatatypeDef)]) -> Self {
        Program { datatypes, ..self }
    }

    pub fn structs(&self) -> &'p [(Sym, StructDef)] {
        self.structs
    }

    pub fn functions(&self) -> &'p [(Sym, LogicalFunctionDef)] {
        self.functions
    }

    pub fn datatypes(&self) -> &'p [(Sym, DatatypeDef)] {
        self.datatypes
    }

    pub fn struct_def(&self, tag: &Sym) -> Result<&'p StructDef, SmtError> {
        self.structs
            .iter()
            .find(|(t, _)| t == tag)
            .map(|(_, d)| d)
            .ok_or_else(|| SmtError::UnknownStruct { tag: tag.clone() })
    }

    pub fn function(&self, name: &Sym) -> Result<&'p LogicalFunctionDef, SmtError> {
        self.functions
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, f)| f)
            .ok_or_else(|| SmtError::UnknownLogicalFunction { name: name.clone() })
    }

    pub fn datatype_def(&self, tag: &Sym) -> Result<&'p DatatypeDef, SmtError> {
        self.datatype_row(tag).map(|(_, def)| def)
    }

    pub fn datatype_row(&self, tag: &Sym) -> Result<(&'p Sym, &'p DatatypeDef), SmtError> {
        self.datatypes
            .iter()
            .find(|(t, _)| t == tag)
            .map(|(t, d)| (t, d))
            .ok_or_else(|| SmtError::UnknownDatatype { tag: tag.clone() })
    }
}

enum Entry {
    Plain(Sort),
    Data(Rc<DatatypeSort>),
}

impl Entry {
    fn sort(&self) -> Sort {
        match self {
            Entry::Plain(s) => s.clone(),
            Entry::Data(d) => d.sort.clone(),
        }
    }
}

pub struct Sorts<'p> {
    program: Program<'p>,
    pointers: PointerEncoding,
    entries: BTreeMap<BaseType, Entry>,
    building: BTreeSet<BaseType>,
}

impl<'p> Sorts<'p> {
    pub fn with_pointers(program: Program<'p>, pointers: PointerEncoding) -> Self {
        Sorts {
            program,
            pointers,
            entries: BTreeMap::new(),
            building: BTreeSet::new(),
        }
    }

    pub fn program(&self) -> Program<'p> {
        self.program
    }

    pub fn pointers(&self) -> PointerEncoding {
        self.pointers
    }

    pub(in crate::gen_dsl) fn key(bt: &BaseType) -> BaseType {
        Self::normalise(bt, true)
    }

    fn normalise(bt: &BaseType, sort_aliases: bool) -> BaseType {
        let nested = |bt: &BaseType| Self::normalise(bt, sort_aliases);
        match bt {
            BaseType::Loc(_) => BaseType::Loc(None),
            BaseType::Bits(_, width) if sort_aliases => BaseType::Bits(Sign::Unsigned, *width),
            BaseType::Unit if sort_aliases => BaseType::Tuple(vec![]),
            BaseType::AllocId if sort_aliases => BaseType::Integer,
            BaseType::Record(ms) => BaseType::Tuple(ms.iter().map(|(_, t)| nested(t)).collect()),
            BaseType::Tuple(ts) => BaseType::Tuple(ts.iter().map(nested).collect()),
            BaseType::Map(k, v) => BaseType::make_map(nested(k), nested(v)),
            BaseType::List(t) => BaseType::List(Box::new(nested(t))),
            BaseType::Set(t) => BaseType::Set(Box::new(nested(t))),
            BaseType::Option(t) => BaseType::Option(Box::new(nested(t))),
            other => other.clone(),
        }
    }

    pub(in crate::gen_dsl) fn same_type(a: &BaseType, b: &BaseType) -> bool {
        Self::normalise(a, false) == Self::normalise(b, false)
    }

    fn check_encoding(&self, bt: &BaseType) -> Result<(), SmtError> {
        if self.pointers.is_provenance() {
            return Ok(());
        }
        let mut pending = vec![bt];
        while let Some(bt) = pending.pop() {
            match bt {
                BaseType::AllocId => {
                    return Err(SmtError::NeedsProvenance {
                        what: "the AllocId base type",
                    });
                }
                BaseType::Map(key, value) => pending.extend([&**key, &**value]),
                BaseType::Tuple(parts) => pending.extend(parts),
                BaseType::Record(parts) => pending.extend(parts.iter().map(|(_, bt)| bt)),
                BaseType::List(elem) | BaseType::Set(elem) | BaseType::Option(elem) => {
                    pending.push(elem);
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn sort(&mut self, bt: &BaseType) -> Result<Sort, SmtError> {
        self.check_encoding(bt)?;
        let key = Self::key(bt);
        if let Some(e) = self.entries.get(&key) {
            return Ok(e.sort());
        }
        let sort = self.build_sort(&key)?;
        self.entries
            .entry(key)
            .or_insert_with(|| Entry::Plain(sort.clone()));
        Ok(sort)
    }

    fn build_sort(&mut self, key: &BaseType) -> Result<Sort, SmtError> {
        Ok(match key {
            BaseType::Bool => Sort::bool(),
            BaseType::Integer => Sort::int(),
            BaseType::Real => Sort::real(),
            BaseType::Bits(_, 0) => return Err(SmtError::ZeroWidthBitvector),
            BaseType::Bits(_, w) => Sort::bitvector(u32::from(*w)),
            BaseType::Loc(_) if self.pointers.is_provenance() => self.datatype(key)?.sort.clone(),
            BaseType::Loc(_) => Sort::bitvector(64),
            BaseType::Map(k, v) => {
                let (k, v) = (self.sort(k)?, self.sort(v)?);
                Sort::array(&k, &v)
            }
            BaseType::Set(t) => {
                let t = self.sort(t)?;
                Sort::set(&t)
            }
            BaseType::Unit
            | BaseType::Tuple(_)
            | BaseType::Record(_)
            | BaseType::Struct(_)
            | BaseType::List(_)
            | BaseType::Option(_) => self.datatype(key)?.sort.clone(),
            BaseType::AllocId if self.pointers.is_provenance() => Sort::int(),
            BaseType::AllocId => {
                return Err(SmtError::NeedsProvenance {
                    what: "the AllocId base type",
                });
            }
            BaseType::MemByte | BaseType::CType => {
                return Err(SmtError::UnsupportedBaseType(key.clone()));
            }
            BaseType::Datatype(_) => self.datatype(key)?.sort.clone(),
        })
    }

    pub fn datatype(&mut self, bt: &BaseType) -> Result<Rc<DatatypeSort>, SmtError> {
        self.check_encoding(bt)?;
        let key = Self::key(bt);
        if let Some(Entry::Data(d)) = self.entries.get(&key) {
            return Ok(Rc::clone(d));
        }
        if let BaseType::Datatype(tag) = &key {
            return self.build_declared_group(tag);
        }
        if !self.building.insert(key.clone()) {
            return Err(SmtError::CyclicDeclaration(key));
        }
        let built = self.build_datatype(&key);
        self.building.remove(&key);
        let decl = Rc::new(built?);
        self.entries.insert(key, Entry::Data(Rc::clone(&decl)));
        Ok(decl)
    }

    fn build_datatype(&mut self, key: &BaseType) -> Result<DatatypeSort, SmtError> {
        match key {
            BaseType::Tuple(components) => self.build_tuple(components),
            BaseType::Struct(tag) => self.build_struct(tag),
            BaseType::List(elem) => self.build_list(elem),
            BaseType::Option(elem) => self.build_option(elem),
            BaseType::Loc(_) if self.pointers.is_provenance() => self.build_pointer(),
            other => Err(SmtError::UnsupportedBaseType(other.clone())),
        }
    }

    fn build_declared_group(&mut self, root: &Sym) -> Result<Rc<DatatypeSort>, SmtError> {
        let mut pending = vec![root.clone()];
        let mut tags = BTreeSet::new();
        while let Some(tag) = pending.pop() {
            let (canonical, def) = self.program.datatype_row(&tag)?;
            let key = BaseType::Datatype(canonical.clone());
            if matches!(self.entries.get(&key), Some(Entry::Data(_)))
                || !tags.insert(canonical.clone())
            {
                continue;
            }
            for (_, bt) in def.constructors.iter().flat_map(|c| c.params.iter()) {
                if let BaseType::Datatype(dependency) = bt {
                    pending.push(dependency.clone());
                }
            }
        }

        let mut rows: BTreeMap<&Sym, usize> = BTreeMap::new();
        for (tag, _) in self.program.datatypes() {
            *rows.entry(tag).or_default() += 1;
        }
        if let Some(tag) = tags
            .iter()
            .find(|tag| rows.get(tag).copied().unwrap_or_default() != 1)
        {
            return Err(SmtError::DuplicateDatatype { tag: tag.clone() });
        }

        let keys: Vec<BaseType> = tags.iter().cloned().map(BaseType::Datatype).collect();
        if let Some(key) = keys.iter().find(|key| self.building.contains(*key)) {
            return Err(SmtError::CyclicDeclaration(key.clone()));
        }
        self.building.extend(keys.iter().cloned());
        let built = self.make_declared_group(&tags);
        for key in &keys {
            self.building.remove(key);
        }

        let mut root_declaration = None;
        for (tag, declaration) in tags.iter().cloned().zip(built?) {
            let declaration = Rc::new(declaration);
            if tag == *root {
                root_declaration = Some(Rc::clone(&declaration));
            }
            self.entries
                .insert(BaseType::Datatype(tag), Entry::Data(declaration));
        }
        Ok(root_declaration.expect("the root of a declaration group joins it"))
    }

    fn make_declared_group(&mut self, tags: &BTreeSet<Sym>) -> Result<Vec<DatatypeSort>, SmtError> {
        let defs: Vec<(&Sym, &'p DatatypeDef)> = tags
            .iter()
            .map(|tag| Ok((tag, self.program.datatype_def(tag)?)))
            .collect::<Result<_, SmtError>>()?;

        let mut cases: BTreeMap<&Sym, usize> = BTreeMap::new();
        for (_, datatype) in self.program.datatypes() {
            for constructor in &datatype.constructors {
                *cases.entry(&constructor.name).or_default() += 1;
            }
        }

        for (tag, def) in &defs {
            if def.constructors.is_empty() {
                return Err(SmtError::EmptyDatatype {
                    tag: (*tag).clone(),
                });
            }
            let mut members = BTreeSet::new();
            for constructor in &def.constructors {
                if cases.get(&constructor.name).copied().unwrap_or_default() != 1 {
                    return Err(SmtError::DuplicateConstructor {
                        constructor: constructor.name.clone(),
                    });
                }
                for (member, _) in &constructor.params {
                    if !members.insert(member) {
                        return Err(SmtError::DuplicateConstructorMember {
                            tag: (*tag).clone(),
                            constructor: constructor.name.clone(),
                            member: member.clone(),
                        });
                    }
                }
            }
        }

        let mut inhabited = BTreeSet::new();
        loop {
            let before = inhabited.len();
            for (tag, def) in &defs {
                if def.constructors.iter().any(|constructor| {
                    constructor.params.iter().all(|(_, bt)| match bt {
                        BaseType::Datatype(dependency) if tags.contains(dependency) => {
                            inhabited.contains(dependency)
                        }
                        _ => true,
                    })
                }) {
                    inhabited.insert((*tag).clone());
                }
            }
            if inhabited.len() == before {
                break;
            }
        }
        if let Some(tag) = tags.iter().find(|tag| !inhabited.contains(*tag)) {
            return Err(SmtError::NonWellFoundedDatatype { tag: tag.clone() });
        }

        let mut builders = Vec::with_capacity(defs.len());
        for (tag, def) in &defs {
            let sort_name = names::datatype_sort_name(tag);
            let mut builder = DatatypeBuilder::new(sort_name.as_str());
            for constructor in &def.constructors {
                let constructor_name = names::datatype_con_name(tag, &constructor.name);
                let mut field_names = Vec::with_capacity(constructor.params.len());
                let mut accessors = Vec::with_capacity(constructor.params.len());
                for (member, bt) in &constructor.params {
                    field_names.push(names::datatype_field_name(tag, &constructor.name, member));
                    accessors.push(match bt {
                        BaseType::Datatype(dependency) => {
                            let canonical = self.program.datatype_row(dependency)?.0;
                            if tags.contains(canonical) {
                                DatatypeAccessor::Datatype(
                                    names::datatype_sort_name(canonical).into(),
                                )
                            } else {
                                DatatypeAccessor::Sort(
                                    self.sort(&BaseType::Datatype(canonical.clone()))?,
                                )
                            }
                        }
                        other => DatatypeAccessor::Sort(self.sort(other)?),
                    });
                }
                let fields = field_names
                    .iter()
                    .map(String::as_str)
                    .zip(accessors)
                    .collect();
                builder = builder.variant(&constructor_name, fields);
            }
            builders.push(builder);
        }
        Ok(create_datatypes(builders))
    }

    fn build_pointer(&mut self) -> Result<DatatypeSort, SmtError> {
        let alloc_id = self.sort(&BaseType::AllocId)?;
        Ok(DatatypeBuilder::new("pointer")
            .variant("NULL", vec![])
            .variant(
                "AiA",
                vec![
                    ("alloc_id", DatatypeAccessor::Sort(alloc_id)),
                    ("addr", DatatypeAccessor::Sort(Sort::bitvector(64))),
                ],
            )
            .finish())
    }

    fn build_tuple(&mut self, components: &[BaseType]) -> Result<DatatypeSort, SmtError> {
        let sorts = components
            .iter()
            .map(|bt| self.sort(bt))
            .collect::<Result<Vec<_>, _>>()?;
        let name = names::tuple_sort_name(components);
        let fields: Vec<String> = (0..components.len()).map(names::tuple_field_name).collect();
        let variant: Vec<(&str, DatatypeAccessor)> = fields
            .iter()
            .zip(sorts)
            .map(|(f, s)| (f.as_str(), DatatypeAccessor::Sort(s)))
            .collect();
        Ok(DatatypeBuilder::new(name.as_str())
            .variant(&format!("mk_{name}"), variant)
            .finish())
    }

    fn build_struct(&mut self, tag: &Sym) -> Result<DatatypeSort, SmtError> {
        let program = self.program;
        let members = &program.struct_def(tag)?.members;
        let mut fields = Vec::with_capacity(members.len());
        for (id, ct) in members {
            let bt = ct
                .to_base_type()
                .ok_or_else(|| SmtError::UnsizedCtype(ct.clone()))?;
            fields.push((names::struct_field_name(tag, id), self.sort(&bt)?));
        }
        let name = names::struct_sort_name(tag);
        let con = names::struct_con_name(tag);
        let variant: Vec<(&str, DatatypeAccessor)> = fields
            .iter()
            .map(|(f, s)| (f.as_str(), DatatypeAccessor::Sort(s.clone())))
            .collect();
        Ok(DatatypeBuilder::new(name.as_str())
            .variant(&con, variant)
            .finish())
    }

    fn build_list(&mut self, elem: &BaseType) -> Result<DatatypeSort, SmtError> {
        let elem_sort = self.sort(elem)?;
        let name = names::list_sort_name(elem);
        Ok(DatatypeBuilder::new(name.as_str())
            .variant("cn_nil", vec![])
            .variant(
                "cn_cons",
                vec![
                    ("cn_head", DatatypeAccessor::Sort(elem_sort)),
                    ("cn_tail", DatatypeAccessor::Datatype(name.as_str().into())),
                ],
            )
            .finish())
    }

    fn build_option(&mut self, elem: &BaseType) -> Result<DatatypeSort, SmtError> {
        let elem_sort = self.sort(elem)?;
        let name = names::option_sort_name(elem);
        Ok(DatatypeBuilder::new(name.as_str())
            .variant("cn_none", vec![])
            .variant(
                "cn_some",
                vec![("cn_val", DatatypeAccessor::Sort(elem_sort))],
            )
            .finish())
    }
}

pub(in crate::gen_dsl) mod variant {
    pub const FIRST: usize = 0;
    pub const SECOND: usize = 1;
}
