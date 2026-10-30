use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

use crate::gen_dsl::{
    BaseType, GenModule, GenTerm, GenTermKind, Id, IndexTerm, Pattern, PatternKind, ScopeName,
    Sctype, Sym, Term,
};

use super::EngineOpenError;
#[cfg(feature = "driver")]
use super::{AnyTreeEngine, EngineBuild, TreeEngine, TreeEngineFactory};

#[derive(Debug, Clone)]
pub struct CustomProducer {
    pub source: PathBuf,
    pub entry: Sym,
    pub module: Arc<GenModule>,
}

#[derive(Debug, Clone)]
pub struct CustomSelection {
    pub module: Arc<GenModule>,
    pub wire_entry: Sym,
}

impl CustomProducer {
    pub(crate) fn error(&self, reason: impl ToString) -> EngineOpenError {
        EngineOpenError::Custom {
            file: self.source.clone(),
            entry: self.entry.clone(),
            reason: reason.to_string(),
        }
    }

    pub fn validate(&self, wire: &GenModule) -> Result<(), EngineOpenError> {
        self.validate_inner(wire)
            .map_err(|reason| self.error(reason))
    }

    fn validate_inner(&self, wire: &GenModule) -> Result<(), String> {
        let module = &self.module;
        module.validate().map_err(|error| error.to_string())?;
        let entry = module
            .generator(&self.entry)
            .ok_or_else(|| format!("unknown entry {:?}", self.entry))?;
        unique(module.structs.iter().map(|(name, _)| name), "struct")?;
        unique(module.globals.iter().map(|(name, _)| name), "global")?;
        unique(
            module.functions.iter().map(|(name, _)| name),
            "logical function",
        )?;
        unique(module.datatypes.iter().map(|(name, _)| name), "datatype")?;
        unique(
            module
                .datatypes
                .iter()
                .flat_map(|(_, def)| def.constructors.iter().map(|c| &c.name)),
            "constructor",
        )?;
        for (name, global) in &module.globals {
            if !wire.globals.iter().any(|(other, decl)| {
                other.num == name.num && other.name == name.name && decl == global
            }) {
                return Err(format!(
                    "global {name:?} does not match a published specification global"
                ));
            }
            check_global_layout(&global.ty, module, wire, &mut BTreeSet::new())?;
        }
        crate::gen_dsl::lang::logical_function::well_formed(&module.functions)
            .map_err(|e| e.to_string())?;
        for (name, def) in &module.functions {
            if name.num != def.name.num || name.name != def.name.name {
                return Err(format!(
                    "logical function key {name:?} disagrees with its declaration"
                ));
            }
            unique(def.args.iter().map(|(name, _)| name), "logical parameter")?;
            for (_, bt) in &def.args {
                check_type(bt, module)?;
            }
            check_type(&def.return_bt, module)?;
            if let Some(body) = &def.body {
                let body = body.term();
                if body.bt != def.return_bt {
                    return Err(format!(
                        "logical function {name:?} has an inconsistent result type"
                    ));
                }
                check_free(body.free_vars(), def.args.iter().map(|(name, _)| name))?;
                check_expression(body, module)?;
            }
        }
        for (name, def) in &module.datatypes {
            if name.num != def.name.num || name.name != def.name.name {
                return Err(format!(
                    "datatype key {name:?} disagrees with its declaration"
                ));
            }
            for constructor in &def.constructors {
                let mut members = BTreeSet::new();
                for (member, bt) in &constructor.params {
                    if !members.insert(member) {
                        return Err(format!(
                            "constructor {:?} has duplicate member {member:?}",
                            constructor.name
                        ));
                    }
                    check_type(bt, module)?;
                }
            }
        }
        let graph = module.call_graph();
        for (name, def) in &module.defs {
            unique(
                def.iargs.iter().map(|(name, _)| name),
                "generator parameter",
            )?;
            for (_, bt) in &def.iargs {
                check_type(bt, module)?;
            }
            check_type(&def.oarg, module)?;
            if def.size_arg.is_none() {
                return Err(format!(
                    "generator {name:?} has no test-case size metadata; imports must already be sized"
                ));
            }
            crate::gen_dsl::validate_size_args(def, module).map_err(|error| {
                format!("generator {name:?} has invalid size metadata: {error:?}")
            })?;
            let metadata: Vec<_> = def.size_args().chain(def.heap_args()).collect();
            unique(metadata.iter().copied(), "size/heap parameter")?;
            if def.heap_base_arg.is_some() != def.heap_size_arg.is_some() {
                return Err(format!(
                    "generator {name:?} must declare both heap arguments or neither"
                ));
            }
            for param in def.heap_args() {
                if !def
                    .iargs
                    .iter()
                    .any(|(name, bt)| name == param && bt == &crate::gen_dsl::uintptr_bt())
                    || module.find_global(param).is_some()
                {
                    return Err(format!(
                        "generator {name:?} has invalid heap parameter {param:?}"
                    ));
                }
            }
            let mut pending: Vec<_> = graph.get(name).into_iter().flatten().cloned().collect();
            let mut visited = BTreeSet::new();
            while let Some(next) = pending.pop() {
                if next == *name && def.rec_size_arg.is_none() {
                    return Err(format!(
                        "recursive generator {name:?} has no recursion-size metadata"
                    ));
                }
                if visited.insert(next.clone()) {
                    pending.extend(graph.get(&next).into_iter().flatten().cloned());
                }
            }
            if def.body.bt != def.oarg {
                return Err(format!(
                    "generator {name:?} has an inconsistent result type"
                ));
            }
            check_free(def.body.free_vars(), def.iargs.iter().map(|(name, _)| name))?;
            def.body
                .well_scoped_reopenable()
                .map_err(|error| format!("generator {name:?}: {error}"))?;
            check_term(&def.body, module)?;
        }
        for (param, bt) in &entry.iargs {
            if !entry
                .size_args()
                .chain(entry.heap_args())
                .any(|special| special == param)
                && (module.find_global(param).is_none() || !matches!(bt, BaseType::Loc(_)))
            {
                return Err(format!("entry has unsupplied input parameter {param:?}"));
            }
        }
        Ok(())
    }

    #[cfg(feature = "driver")]
    pub(crate) fn open(
        &self,
        kind: AnyTreeEngine,
        mut build: EngineBuild,
        wire_entry: Sym,
    ) -> Result<Box<dyn TreeEngine>, EngineOpenError> {
        build.custom = Some(CustomSelection {
            module: Arc::clone(&self.module),
            wire_entry,
        });
        kind.open_validated(build)
            .map_err(|error| self.error(error))
    }
}

fn unique<'a>(names: impl Iterator<Item = &'a Sym>, kind: &str) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for name in names {
        if !seen.insert(name.num) {
            return Err(format!("duplicate {kind} identity {name:?}"));
        }
    }
    Ok(())
}

fn check_free<'a>(
    free: crate::gen_dsl::SymSet,
    bound: impl Iterator<Item = &'a Sym>,
) -> Result<(), String> {
    let bound: BTreeSet<_> = bound.map(|s| s.num).collect();
    for name in free {
        if !bound.contains(&name.num) {
            return Err(format!("unbound variable {name:?}"));
        }
    }
    Ok(())
}

fn check_term(term: &GenTerm, module: &GenModule) -> Result<(), String> {
    check_type(&term.bt, module)?;
    match &term.term {
        GenTermKind::Arbitrary => (),
        GenTermKind::Return(value) => {
            check_expression(value, module)?;
            if value.bt != term.bt {
                return Err("return type disagrees with its value".into());
            }
        }
        GenTermKind::Call { fsym, args } => {
            let def = module
                .resolve_call(fsym, args.len())
                .map_err(|e| e.to_string())?;
            if term.bt != def.oarg
                || args
                    .iter()
                    .zip(&def.iargs)
                    .any(|(arg, (_, bt))| &arg.bt != bt)
            {
                return Err(format!(
                    "call to {fsym:?} has incompatible argument or result types"
                ));
            }
            for arg in args {
                check_expression(arg, module)?;
            }
        }
        GenTermKind::WriteChoice { value, rest, .. } => {
            check_expression(value, module)?;
            check_rest(term, rest, module)?;
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => {
            check_storage_type(ct, module)?;
            check_expression(addr, module)?;
            check_expression(value, module)?;
            check_rest(term, rest, module)?;
        }
        GenTermKind::LetStar { value, rest, .. } => {
            check_term(value, module)?;
            check_rest(term, rest, module)?;
        }
        GenTermKind::Assert { constraint, rest } => {
            match constraint {
                crate::gen_dsl::LogicalConstraint::T(expr) => check_expression(expr, module)?,
                crate::gen_dsl::LogicalConstraint::Forall {
                    permission, body, ..
                } => {
                    check_expression(permission, module)?;
                    check_expression(body, module)?;
                }
            }
            check_rest(term, rest, module)?;
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            check_expression(cond, module)?;
            if cond.bt != BaseType::Bool {
                return Err("branch condition must be Boolean".into());
            }
            check_rest(term, then_, module)?;
            check_rest(term, else_, module)?;
        }
        GenTermKind::Map {
            var_bt, perm, body, ..
        } => {
            check_type(var_bt, module)?;
            if term.bt != BaseType::make_map(var_bt.clone(), body.bt.clone()) {
                return Err("map has an inconsistent result type".into());
            }
            check_expression(perm, module)?;
            check_term(body, module)?;
        }
        GenTermKind::Scope { name, body } => {
            if let ScopeName::Index(expr) = name {
                check_expression(expr, module)?;
            }
            check_rest(term, body, module)?;
        }
    }
    Ok(())
}

fn check_rest(term: &GenTerm, rest: &GenTerm, module: &GenModule) -> Result<(), String> {
    if term.bt != rest.bt {
        return Err("generator continuation has an inconsistent result type".into());
    }
    check_term(rest, module)
}

fn check_expression(expression: &IndexTerm, module: &GenModule) -> Result<(), String> {
    let mut error = None;
    expression.clone().map_post(&mut |node| {
        if error.is_none() {
            error = check_type(&node.bt, module).err();
        }
        if error.is_none() {
            error = match &node.term {
                Term::Apply(name, args) => match module.find_function(name) {
                    None => Some(format!("unknown logical function {name:?}")),
                    Some(def)
                        if args.len() != def.args.len()
                            || args
                                .iter()
                                .zip(&def.args)
                                .any(|(arg, (_, bt))| &arg.bt != bt)
                            || node.bt != def.return_bt =>
                    {
                        Some(format!("invalid logical call to {name:?}"))
                    }
                    _ => None,
                },
                Term::Constructor(name, fields) => check_constructor(
                    name,
                    fields.iter().map(|(id, value)| (id, &value.bt)),
                    &node.bt,
                    module,
                    true,
                )
                .err(),
                Term::Match(scrutinee, arms) => arms
                    .iter()
                    .try_for_each(|(pattern, body)| {
                        if pattern.bt != scrutinee.bt || body.bt != node.bt {
                            return Err("match has inconsistent pattern or result types".into());
                        }
                        check_pattern(pattern, module)
                    })
                    .err(),
                Term::Const(crate::gen_dsl::Const::InputDefault(bt))
                | Term::Const(crate::gen_dsl::Const::Unknown(bt))
                | Term::Nil(bt)
                | Term::MapConst(bt, _)
                | Term::MapDef((_, bt), _)
                | Term::Cast(bt, _)
                | Term::CnNone(bt) => check_type(bt, module).err(),
                Term::Struct(name, _) | Term::OffsetOf(name, _) | Term::MemberShift(_, name, _)
                    if module.find_struct(name).is_none() =>
                {
                    Some(format!("unknown struct {name:?}"))
                }
                _ => None,
            };
        }
        node
    });
    error.map_or(Ok(()), Err)
}

fn check_type(bt: &BaseType, module: &GenModule) -> Result<(), String> {
    match bt {
        BaseType::Struct(name) if module.find_struct(name).is_none() => {
            return Err(format!("unknown struct type {name:?}"));
        }
        BaseType::Datatype(name) if !module.datatypes.iter().any(|(other, _)| other == name) => {
            return Err(format!("unknown datatype {name:?}"));
        }
        BaseType::Record(members) => {
            for (_, member) in members {
                check_type(member, module)?;
            }
        }
        BaseType::Map(key, value) => {
            check_type(key, module)?;
            check_type(value, module)?;
        }
        BaseType::List(value) | BaseType::Set(value) | BaseType::Option(value) => {
            check_type(value, module)?;
        }
        BaseType::Tuple(members) => {
            for member in members {
                check_type(member, module)?;
            }
        }
        _ => (),
    }
    Ok(())
}

fn check_storage_type(ct: &Sctype, module: &GenModule) -> Result<(), String> {
    match ct {
        Sctype::Struct(name) => {
            let definition = module
                .find_struct(name)
                .ok_or_else(|| format!("unknown assignment struct {name:?}"))?;
            if definition.layout_in(&module.structs).is_none() {
                return Err(format!(
                    "assignment struct {name:?} has no complete object layout"
                ));
            }
        }
        Sctype::Array(element, _) => check_storage_type(element, module)?,
        _ => (),
    }
    Ok(())
}

fn check_constructor<'a>(
    name: &Sym,
    fields: impl Iterator<Item = (&'a Id, &'a BaseType)>,
    result: &BaseType,
    module: &GenModule,
    complete: bool,
) -> Result<(), String> {
    let (datatype, declaration) = module
        .datatypes
        .iter()
        .find_map(|(datatype, declaration)| {
            declaration
                .constructor(name)
                .map(|constructor| (datatype, constructor))
        })
        .ok_or_else(|| format!("unknown constructor {name:?}"))?;
    if result != &BaseType::Datatype(datatype.clone()) {
        return Err(format!("constructor {name:?} has an inconsistent datatype"));
    }
    let mut count = 0;
    let mut seen = BTreeSet::new();
    for (field, bt) in fields {
        count += 1;
        if !seen.insert(field)
            || !declaration
                .params
                .iter()
                .any(|(expected, expected_bt)| expected == field && expected_bt == bt)
        {
            return Err(format!(
                "constructor {name:?} has an invalid member {field:?}"
            ));
        }
    }
    if complete && count != declaration.params.len() {
        return Err(format!("constructor {name:?} has missing members"));
    }
    Ok(())
}

fn check_pattern(pattern: &Pattern, module: &GenModule) -> Result<(), String> {
    check_type(&pattern.bt, module)?;
    if let PatternKind::Constructor(name, fields) = &pattern.pattern {
        check_constructor(
            name,
            fields.iter().map(|(id, value)| (id, &value.bt)),
            &pattern.bt,
            module,
            false,
        )?;
        for (_, pattern) in fields {
            check_pattern(pattern, module)?;
        }
    }
    Ok(())
}

fn check_global_layout(
    ct: &crate::gen_dsl::Sctype,
    module: &GenModule,
    wire: &GenModule,
    visited: &mut BTreeSet<u64>,
) -> Result<(), String> {
    use crate::gen_dsl::Sctype;
    match ct {
        Sctype::Struct(name) if visited.insert(name.num) => {
            let def = module
                .find_struct(name)
                .ok_or_else(|| format!("missing global struct layout {name:?}"))?;
            if Some(def) != wire.find_struct(name) {
                return Err(format!(
                    "global struct {name:?} has a different object layout"
                ));
            }
            for member in &def.members {
                check_global_layout(&member.1, module, wire, visited)?;
            }
        }
        Sctype::Array(inner, _) => check_global_layout(inner, module, wire, visited)?,
        _ => (),
    }
    Ok(())
}
