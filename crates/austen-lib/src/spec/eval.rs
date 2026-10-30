use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::spec::{
    BaseType, BinOp, Bindings, CType, Const, DefaultChoices, FunctionBody, Id, IndexTerm,
    IntegerType, Pattern, PatternKind, PointerOrigin, Scalar, Sctype, Sign, SpecModule, Sym,
    SymMap, SymbolicMapBounds, Term, UnOp, Value, check_quantified_permission,
};

use num_bigint::BigInt;

pub const MAX_DEPTH: usize = 128;

const MAX_LOGICAL_CALLS: usize = 4096;

const MAX_LOGICAL_VALUE_DEPTH: usize = 512;

#[derive(Clone)]
struct LogicalCall {
    name: Sym,
    args: Vec<Value>,
}

impl PartialEq for LogicalCall {
    fn eq(&self, other: &Self) -> bool {
        same_call(self, other)
    }
}

impl Eq for LogicalCall {}

impl Hash for LogicalCall {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.args.len().hash(state);
        for value in &self.args {
            hash_call_value(value, state);
        }
    }
}

fn hash_call_value<H: Hasher>(value: &Value, state: &mut H) {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Unit => {
                8u8.hash(state);
            }
            Value::Scalar(Scalar::Int(value)) => {
                0u8.hash(state);
                value.hash(state);
            }
            Value::Scalar(Scalar::Bits(sign, width, value)) => {
                1u8.hash(state);
                sign.hash(state);
                width.hash(state);
                value.hash(state);
            }
            Value::Scalar(Scalar::Bool(value)) => {
                2u8.hash(state);
                value.hash(state);
            }
            Value::Scalar(Scalar::Pointer { addr, origin }) => {
                3u8.hash(state);
                addr.hash(state);
                match origin {
                    PointerOrigin::Heap => 0u8.hash(state),
                    PointerOrigin::Global(id) => {
                        1u8.hash(state);
                        id.hash(state);
                    }
                    PointerOrigin::Opaque => 2u8.hash(state),
                }
            }
            Value::Struct { tag, fields } => {
                4u8.hash(state);
                tag.hash(state);
                fields.len().hash(state);
                for (id, _) in fields {
                    id.hash(state);
                }
                pending.extend(fields.iter().rev().map(|(_, value)| value));
            }
            Value::Record(fields) => {
                5u8.hash(state);
                fields.len().hash(state);
                for (id, _) in fields {
                    id.hash(state);
                }
                pending.extend(fields.iter().rev().map(|(_, value)| value));
            }
            Value::Constructor { name, fields } => {
                6u8.hash(state);
                name.hash(state);
                fields.len().hash(state);
                for (id, _) in fields {
                    id.hash(state);
                }
                pending.extend(fields.iter().rev().map(|(_, value)| value));
            }
            Value::Map { entries, default } => {
                7u8.hash(state);
                entries.len().hash(state);
                for (key, _) in entries {
                    key.hash(state);
                }
                default.is_some().hash(state);
                if let Some(default) = default {
                    pending.push(default);
                }
                pending.extend(entries.iter().rev().map(|(_, value)| value));
            }
        }
    }
}

struct CallFrame {
    call: LogicalCall,
    results: HashMap<LogicalCall, Value>,
}

#[derive(Default)]
struct Calls {
    pending: Vec<CallFrame>,
    root_results: HashMap<LogicalCall, Value>,
    requested: Option<LogicalCall>,
    started: usize,
    driving: bool,
}

impl Calls {
    fn results(&self) -> &HashMap<LogicalCall, Value> {
        self.pending
            .last()
            .map_or(&self.root_results, |frame| &frame.results)
    }

    fn start(&mut self, call: LogicalCall) -> bool {
        if self.started >= MAX_LOGICAL_CALLS
            || self
                .pending
                .iter()
                .any(|frame| same_call(&frame.call, &call))
        {
            return false;
        }
        self.started += 1;
        self.pending.push(CallFrame {
            call,
            results: HashMap::new(),
        });
        true
    }

    fn complete(&mut self, value: Value) -> Result<Option<Value>, ()> {
        if !logical_value_depth_is_bounded(&value) {
            return Err(());
        }
        let frame = self.pending.pop().expect("a completed logical call");
        if let Some(parent) = self.pending.last_mut() {
            parent.results.insert(frame.call, value);
            Ok(None)
        } else {
            self.root_results.insert(frame.call, value.clone());
            Ok(Some(value))
        }
    }
}

fn logical_value_depth_is_bounded(value: &Value) -> bool {
    let mut pending = vec![(value, 0usize)];
    while let Some((value, depth)) = pending.pop() {
        if depth > MAX_LOGICAL_VALUE_DEPTH {
            return false;
        }
        let child_depth = depth + 1;
        match value {
            Value::Unit | Value::Scalar(_) => {}
            Value::Struct { fields, .. }
            | Value::Record(fields)
            | Value::Constructor { fields, .. } => {
                pending.extend(fields.iter().map(|(_, value)| (value, child_depth)));
            }
            Value::Map { entries, default } => {
                pending.extend(entries.iter().map(|(_, value)| (value, child_depth)));
                if let Some(default) = default {
                    pending.push((default, child_depth));
                }
            }
        }
    }
    true
}

#[derive(Clone, Copy)]
struct Context<'a> {
    defaults: Option<&'a DefaultChoices>,
    module: &'a SpecModule,
    origins: &'a dyn Fn(u64) -> PointerOrigin,
    calls: Option<&'a RefCell<Calls>>,
}

pub fn eval_with_defaults(
    term: &IndexTerm,
    env: &SymMap<Value>,
    module: &SpecModule,
    origins: &dyn Fn(u64) -> PointerOrigin,
    defaults: &DefaultChoices,
) -> Option<Value> {
    let env = Bindings::root(env);
    resolve_calls(module, origins, defaults, |context| {
        eval_at(term, &env, context, 0)
    })
}

fn resolve_calls<T>(
    module: &SpecModule,
    origins: &dyn Fn(u64) -> PointerOrigin,
    defaults: &DefaultChoices,
    mut root: impl FnMut(Context<'_>) -> Option<T>,
) -> Option<T> {
    let calls = RefCell::new(Calls::default());
    let context = Context {
        defaults: Some(defaults),
        module,
        origins,
        calls: Some(&calls),
    };
    root(context)
}

fn drive_call(context: Context<'_>, initial: LogicalCall) -> Option<Value> {
    let calls = context.calls?;
    {
        let mut state = calls.borrow_mut();
        debug_assert!(!state.driving);
        state.driving = true;
        if !state.start(initial) {
            state.driving = false;
            return None;
        }
    }

    let result = (|| loop {
        calls.borrow_mut().requested = None;
        if let Some(value) = eval_pending_call(context) {
            match calls.borrow_mut().complete(value).ok()? {
                Some(value) => return Some(value),
                None => continue,
            }
        }
        let requested = calls.borrow_mut().requested.take()?;
        if !calls.borrow_mut().start(requested) {
            return None;
        }
    })();

    let mut state = calls.borrow_mut();
    state.driving = false;
    state.requested = None;
    state.pending.clear();
    result
}

fn eval_pending_call(context: Context<'_>) -> Option<Value> {
    let call = context.calls?.borrow().pending.last()?.call.clone();
    let function = context.module.logical_function(&call.name)?;
    if function.args.len() != call.args.len() {
        return None;
    }
    let body = match function.body.as_ref()? {
        FunctionBody::Def(body) | FunctionBody::RecDef(body) => body,
    };
    let root = Bindings::root(&NO_BINDINGS);
    let env = root.batch(
        function
            .args
            .iter()
            .zip(call.args)
            .map(|((formal, _), value)| (formal, value))
            .collect(),
    );
    eval_at(body, &env, context, 0)
}

fn eval_at(it: &IndexTerm, env: &Bindings, context: Context<'_>, depth: usize) -> Option<Value> {
    let module = context.module;
    if depth >= MAX_DEPTH {
        return None;
    }
    let d = depth + 1;
    match &it.term {
        Term::Const(Const::InputDefault(ty)) => context.defaults?.get_value(ty),
        Term::Const(Const::Unit) => Some(Value::Unit),
        Term::Const(c) => const_value(c).map(Value::Scalar),
        Term::Sym(s) => env.get(s).cloned(),
        Term::Unop(op, value) => {
            unary(*op, scalar(eval_at(value, env, context, d)?)?).map(Value::Scalar)
        }
        Term::Binop(BinOp::And, left, right) => {
            let left = boolean(eval_at(left, env, context, d)?)?;
            Some(bool_value(
                left && boolean(eval_at(right, env, context, d)?)?,
            ))
        }
        Term::Binop(BinOp::Or, left, right) => {
            let left = boolean(eval_at(left, env, context, d)?)?;
            Some(bool_value(
                left || boolean(eval_at(right, env, context, d)?)?,
            ))
        }
        Term::Binop(BinOp::Implies, left, right) => {
            let left = boolean(eval_at(left, env, context, d)?)?;
            Some(bool_value(
                !left || boolean(eval_at(right, env, context, d)?)?,
            ))
        }
        Term::Binop(BinOp::Eq, left, right) => Some(bool_value(language_eq(
            &eval_at(left, env, context, d)?,
            &eval_at(right, env, context, d)?,
            module,
        )?)),
        Term::Binop(op, left, right) => binary(
            *op,
            scalar(eval_at(left, env, context, d)?)?,
            scalar(eval_at(right, env, context, d)?)?,
        )
        .map(Value::Scalar),
        Term::Ite(cond, yes, no) => {
            if boolean(eval_at(cond, env, context, d)?)? {
                eval_at(yes, env, context, d)
            } else {
                eval_at(no, env, context, d)
            }
        }
        Term::EachI {
            start,
            var,
            end,
            body,
        } => {
            let start = i128::from(*start);
            let end = i128::from(*end).checked_add(1)?;
            Some(bool_value(forall_range(
                var,
                None,
                body,
                start..end,
                env,
                context,
                d,
            )?))
        }
        Term::Let((name, bound), body) => {
            let value = eval_at(bound, env, context, d)?;
            eval_at(body, &env.push(name, value), context, d)
        }
        Term::Struct(tag, members) => Some(Value::Struct {
            tag: tag.clone(),
            fields: eval_members(members, env, context, d)?,
        }),
        Term::StructMember(value, member) => match eval_at(value, env, context, d)? {
            Value::Struct { fields, .. } => field(&fields, member).cloned(),
            _ => None,
        },
        Term::StructUpdate {
            target,
            member,
            value,
        } => match eval_at(target, env, context, d)? {
            Value::Struct { tag, mut fields } => {
                field_mut(&mut fields, member)?.clone_from(&eval_at(value, env, context, d)?);
                Some(Value::Struct { tag, fields })
            }
            _ => None,
        },
        Term::Record(members) => Some(Value::Record(eval_members(members, env, context, d)?)),
        Term::RecordMember(value, member) => match eval_at(value, env, context, d)? {
            Value::Record(fields) => field(&fields, member).cloned(),
            _ => None,
        },
        Term::RecordUpdate {
            target,
            member,
            value,
        } => match eval_at(target, env, context, d)? {
            Value::Record(mut fields) => {
                field_mut(&mut fields, member)?.clone_from(&eval_at(value, env, context, d)?);
                Some(Value::Record(fields))
            }
            _ => None,
        },
        Term::Constructor(name, members) => constructor(name, members, env, context, d),
        Term::Match(scrutinee, arms) => {
            let value = eval_at(scrutinee, env, context, d)?;
            if !matches!(value, Value::Constructor { .. }) {
                return None;
            }
            let (bindings, body) = arms.iter().find_map(|(pattern, body)| {
                let mut bindings = Vec::new();
                pattern_bindings(pattern, &value, &mut bindings).then_some((bindings, body))
            })?;
            eval_bound(&bindings, body, env, context, d)
        }
        Term::MemberShift(base, tag, member) => {
            let (addr, origin) = scalar(eval_at(base, env, context, d)?)?.as_pointer()?;
            let offset = struct_layout(module, tag)?
                .members
                .iter()
                .find(|m| &m.id == member)?
                .offset;
            Some(pointer_value(addr.wrapping_add(offset), origin))
        }
        Term::ArrayShift { base, ct, index } => {
            let (addr, origin) = scalar(eval_at(base, env, context, d)?)?.as_pointer()?;
            let index = unsigned64(scalar(eval_at(index, env, context, d)?)?)?;
            let size = size_of(module, ct)?;
            Some(pointer_value(
                addr.wrapping_add(size.wrapping_mul(index)),
                origin,
            ))
        }
        Term::SizeOf(ct) => number(i128::from(size_of(module, ct)?), &it.bt).map(Value::Scalar),
        Term::OffsetOf(tag, member) => {
            let n = struct_layout(module, tag)?
                .members
                .iter()
                .find(|m| &m.id == member)?
                .offset;
            number(i128::from(n), &it.bt).map(Value::Scalar)
        }
        Term::Cast(target, value) => cast(
            target,
            scalar(eval_at(value, env, context, d)?)?,
            context.origins,
        )
        .map(Value::Scalar),
        Term::Representable(ct, value) => {
            value_check(false, ct, value, env, context, d, 0).map(bool_value)
        }
        Term::Good(ct, value) => value_check(true, ct, value, env, context, d, 0).map(bool_value),
        Term::Aligned { t, align } => {
            let value = scalar(eval_at(t, env, context, d)?)?;
            let align = scalar(eval_at(align, env, context, d)?)?;
            let (addr, _) = value.as_pointer()?;
            let divisor = unsigned64(align)?;
            (divisor != 0).then(|| bool_value(addr % divisor == 0))
        }
        Term::Apply(name, args) => apply(name, args, env, context, d),
        Term::MapConst(_, default) => Some(Value::Map {
            entries: Vec::new(),
            default: Some(Box::new(eval_at(default, env, context, d)?)),
        }),
        Term::MapSet(map, key, value) => {
            let Value::Map {
                mut entries,
                default,
            } = eval_at(map, env, context, d)?
            else {
                return None;
            };
            let key = integer(eval_at(key, env, context, d)?)?;
            let value = eval_at(value, env, context, d)?;
            match entries.binary_search_by_key(&key, |(k, _)| *k) {
                Ok(at) => entries[at].1 = value,
                Err(at) => entries.insert(at, (key, value)),
            }
            Some(Value::Map { entries, default })
        }
        Term::MapGet(map, key) => {
            let key = integer(eval_at(key, env, context, d)?)?;
            match eval_at(map, env, context, d)? {
                Value::Map { entries, default } => entries
                    .binary_search_by_key(&key, |(k, _)| *k)
                    .ok()
                    .map(|at| entries[at].1.clone())
                    .or_else(|| default.map(|v| *v)),
                _ => None,
            }
        }
        Term::CopyAllocId { .. }
        | Term::HasAllocId(_)
        | Term::Nil(_)
        | Term::Cons(_, _)
        | Term::Head(_)
        | Term::Tail(_)
        | Term::Tuple(_)
        | Term::NthTuple(_, _)
        | Term::WrapI(_, _)
        | Term::MapDef(_, _)
        | Term::CnNone(_)
        | Term::CnSome(_)
        | Term::IsSome(_)
        | Term::GetOpt(_) => None,
    }
}

fn eval_members(
    members: &[(Id, IndexTerm)],
    env: &Bindings,
    context: Context<'_>,
    depth: usize,
) -> Option<Vec<(Id, Value)>> {
    members
        .iter()
        .map(|(id, value)| Some((id.clone(), eval_at(value, env, context, depth)?)))
        .collect()
}

fn field<'a>(fields: &'a [(Id, Value)], id: &Id) -> Option<&'a Value> {
    fields.iter().find(|(name, _)| name == id).map(|(_, v)| v)
}

fn field_mut<'a>(fields: &'a mut [(Id, Value)], id: &Id) -> Option<&'a mut Value> {
    fields
        .iter_mut()
        .find(|(name, _)| name == id)
        .map(|(_, v)| v)
}

fn constructor(
    name: &Sym,
    members: &[(Id, IndexTerm)],
    env: &Bindings,
    context: Context<'_>,
    depth: usize,
) -> Option<Value> {
    let module = context.module;
    let declaration = module
        .datatypes
        .iter()
        .flat_map(|d| &d.constructors)
        .find(|c| &c.name == name)?;
    if declaration.params.len() != members.len() {
        return None;
    }
    let fields = declaration
        .params
        .iter()
        .map(|(id, _)| {
            let term = &members.iter().find(|(actual, _)| actual == id)?.1;
            Some((id.clone(), eval_at(term, env, context, depth)?))
        })
        .collect::<Option<_>>()?;
    Some(Value::Constructor {
        name: name.clone(),
        fields,
    })
}

pub fn pattern_bindings<'p, 'v>(
    pattern: &'p Pattern,
    value: &'v Value,
    bindings: &mut Vec<(&'p Sym, &'v Value)>,
) -> bool {
    match &pattern.pattern {
        PatternKind::Sym(name) => {
            bindings.push((name, value));
            true
        }
        PatternKind::Wild => true,
        PatternKind::Constructor(name, patterns) => match value {
            Value::Constructor {
                name: actual,
                fields,
            } if actual == name => patterns.iter().all(|(id, pattern)| {
                field(fields, id).is_some_and(|value| pattern_bindings(pattern, value, bindings))
            }),
            _ => false,
        },
    }
}

fn eval_bound(
    bindings: &[(&Sym, &Value)],
    body: &IndexTerm,
    env: &Bindings,
    context: Context<'_>,
    depth: usize,
) -> Option<Value> {
    if bindings.is_empty() {
        return eval_at(body, env, context, depth);
    }
    let nested = env.batch(
        bindings
            .iter()
            .map(|(sym, value)| (*sym, (*value).clone()))
            .collect(),
    );
    eval_at(body, &nested, context, depth)
}

fn apply(
    name: &Sym,
    args: &[IndexTerm],
    caller: &Bindings,
    context: Context<'_>,
    depth: usize,
) -> Option<Value> {
    let module = context.module;
    let function = module.logical_function(name)?;
    if function.args.len() != args.len() {
        return None;
    }
    let body = match function.body.as_ref()? {
        FunctionBody::Def(body) | FunctionBody::RecDef(body) => body,
    };
    let mut values = Vec::with_capacity(args.len());
    for arg in args {
        values.push(eval_at(arg, caller, context, depth)?);
    }

    if let Some(calls) = context.calls {
        let call = LogicalCall {
            name: name.clone(),
            args: values,
        };
        if let Some(value) = calls.borrow().results().get(&call).cloned() {
            return Some(value);
        }
        if calls.borrow().driving {
            calls.borrow_mut().requested = Some(call);
            return None;
        }
        return drive_call(context, call);
    }

    let bindings = function
        .args
        .iter()
        .zip(values)
        .map(|((param, _), value)| (param, value))
        .collect();
    let root = Bindings::root(&NO_BINDINGS);
    let nested = root.batch(bindings);
    eval_at(body, &nested, context, depth)
}

fn same_call(left: &LogicalCall, right: &LogicalCall) -> bool {
    left.name == right.name
        && left.args.len() == right.args.len()
        && left
            .args
            .iter()
            .rev()
            .zip(right.args.iter().rev())
            .all(|(left, right)| same_value(left, right))
}

fn same_value(left: &Value, right: &Value) -> bool {
    structural_value_eq(left, right, true)
}

fn structural_value_eq(left: &Value, right: &Value, pointer_origins: bool) -> bool {
    let mut pending = vec![(left, right)];
    while let Some((left, right)) = pending.pop() {
        match (left, right) {
            (Value::Unit, Value::Unit) => {}
            (Value::Scalar(left), Value::Scalar(right))
                if same_scalar(left, right, pointer_origins) => {}
            (
                Value::Struct {
                    tag: left_tag,
                    fields: left_fields,
                },
                Value::Struct {
                    tag: right_tag,
                    fields: right_fields,
                },
            ) if left_tag == right_tag => {
                if !queue_fields(left_fields, right_fields, &mut pending) {
                    return false;
                }
            }
            (Value::Record(left), Value::Record(right)) => {
                if !queue_fields(left, right, &mut pending) {
                    return false;
                }
            }
            (
                Value::Constructor {
                    name: left_name,
                    fields: left_fields,
                },
                Value::Constructor {
                    name: right_name,
                    fields: right_fields,
                },
            ) if left_name == right_name => {
                if !queue_fields(left_fields, right_fields, &mut pending) {
                    return false;
                }
            }
            (
                Value::Map {
                    entries: left_entries,
                    default: left_default,
                },
                Value::Map {
                    entries: right_entries,
                    default: right_default,
                },
            ) => {
                if left_entries.len() != right_entries.len() {
                    return false;
                }
                for ((left_key, left), (right_key, right)) in left_entries.iter().zip(right_entries)
                {
                    if left_key != right_key {
                        return false;
                    }
                    pending.push((left, right));
                }
                match (left_default, right_default) {
                    (Some(left), Some(right)) => pending.push((left, right)),
                    (None, None) => {}
                    _ => return false,
                }
            }
            _ => return false,
        }
    }
    true
}

fn queue_fields<'a>(
    left: &'a [(Id, Value)],
    right: &'a [(Id, Value)],
    pending: &mut Vec<(&'a Value, &'a Value)>,
) -> bool {
    if left.len() != right.len() {
        return false;
    }
    for ((left_id, left), (right_id, right)) in left.iter().zip(right) {
        if left_id != right_id {
            return false;
        }
        pending.push((left, right));
    }
    true
}

fn same_scalar(left: &Scalar, right: &Scalar, pointer_origins: bool) -> bool {
    match (left, right) {
        (
            Scalar::Pointer {
                addr: left_addr,
                origin: left_origin,
            },
            Scalar::Pointer {
                addr: right_addr,
                origin: right_origin,
            },
        ) => left_addr == right_addr && (!pointer_origins || left_origin == right_origin),
        _ => left == right,
    }
}

static NO_BINDINGS: SymMap<Value> = SymMap::new();

fn const_value(value: &Const) -> Option<Scalar> {
    match value {
        Const::Z(n) => Some(Scalar::Int(n.clone())),
        Const::Bits((sign, width), n) => bits(*sign, *width, n.clone()),
        Const::Bool(value) => Some(Scalar::Bool(*value)),
        Const::Null => Some(Scalar::pointer(0)),
        Const::Pointer { addr, .. } => u64::try_from(addr).ok().map(Scalar::pointer),
        _ => None,
    }
}

fn bool_value(value: bool) -> Value {
    Value::Scalar(Scalar::Bool(value))
}

fn scalar(value: Value) -> Option<Scalar> {
    match value {
        Value::Scalar(value) => Some(value),
        _ => None,
    }
}

pub fn boolean(value: Value) -> Option<bool> {
    match scalar(value)? {
        Scalar::Bool(value) => Some(value),
        _ => None,
    }
}

pub fn integer(value: Value) -> Option<i128> {
    match scalar(value)? {
        Scalar::Int(value) | Scalar::Bits(_, _, value) => i128::try_from(value).ok(),
        _ => None,
    }
}

fn language_eq(left: &Value, right: &Value, module: &SpecModule) -> Option<bool> {
    let mut pending = vec![(left, right)];
    while let Some((left, right)) = pending.pop() {
        match (left, right) {
            (Value::Unit, Value::Unit) => {}
            (Value::Scalar(left), Value::Scalar(right)) => {
                if left != right {
                    return Some(false);
                }
            }
            (
                Value::Constructor {
                    name: left_name,
                    fields: left_fields,
                },
                Value::Constructor {
                    name: right_name,
                    fields: right_fields,
                },
            ) => {
                if left_name != right_name {
                    return Some(false);
                }
                let declaration = constructor_declaration(module, left_name)?;
                if !canonical_fields(left_fields, &declaration.params)
                    || !canonical_fields(right_fields, &declaration.params)
                {
                    return None;
                }
                pending.extend(
                    left_fields
                        .iter()
                        .zip(right_fields)
                        .rev()
                        .map(|((_, left), (_, right))| (left, right)),
                );
            }
            (Value::Record(left), Value::Record(right)) => {
                queue_named_fields(left, right, &mut pending)?;
            }
            (
                Value::Struct {
                    tag: left_tag,
                    fields: left_fields,
                },
                Value::Struct {
                    tag: right_tag,
                    fields: right_fields,
                },
            ) => {
                if left_tag != right_tag {
                    return None;
                }
                queue_named_fields(left_fields, right_fields, &mut pending)?;
            }
            (
                Value::Map {
                    entries: left_entries,
                    default: left_default,
                },
                Value::Map {
                    entries: right_entries,
                    default: right_default,
                },
            ) => match (left_default, right_default) {
                (None, None) => {
                    if !structural_value_eq(left, right, false) {
                        return Some(false);
                    }
                }
                (Some(left_default), Some(right_default)) => {
                    let left_default: &Value = left_default;
                    let right_default: &Value = right_default;
                    let mut keys: Vec<i128> = left_entries
                        .iter()
                        .chain(right_entries)
                        .map(|(key, _)| *key)
                        .collect();
                    keys.sort_unstable();
                    keys.dedup();
                    for key in keys.into_iter().rev() {
                        pending.push((
                            entry_or_default(left_entries, left_default, key),
                            entry_or_default(right_entries, right_default, key),
                        ));
                    }
                    pending.push((left_default, right_default));
                }
                _ => return None,
            },
            _ => return None,
        }
    }
    Some(true)
}

fn queue_named_fields<'a>(
    left: &'a [(Id, Value)],
    right: &'a [(Id, Value)],
    pending: &mut Vec<(&'a Value, &'a Value)>,
) -> Option<()> {
    if left.len() != right.len() || left.iter().any(|(id, _)| field(right, id).is_none()) {
        return None;
    }
    for (id, left_value) in left.iter().rev() {
        pending.push((left_value, field(right, id)?));
    }
    Some(())
}

fn entry_or_default<'a>(entries: &'a [(i128, Value)], default: &'a Value, key: i128) -> &'a Value {
    entries
        .binary_search_by_key(&key, |(entry, _)| *entry)
        .ok()
        .map_or(default, |index| &entries[index].1)
}

fn constructor_declaration<'a>(
    module: &'a SpecModule,
    name: &Sym,
) -> Option<&'a crate::spec::ConstructorDef> {
    module
        .datatypes
        .iter()
        .flat_map(|datatype| &datatype.constructors)
        .find(|constructor| &constructor.name == name)
}

fn canonical_fields(fields: &[(Id, Value)], declaration: &[(Id, BaseType)]) -> bool {
    fields.len() == declaration.len()
        && fields
            .iter()
            .zip(declaration)
            .all(|((actual, _), (expected, _))| actual == expected)
}

pub fn bits(sign: Sign, width: u16, value: impl Into<BigInt>) -> Option<Scalar> {
    normalise(sign, width, value.into()).map(|value| Scalar::Bits(sign, width, value))
}

fn normalise(sign: Sign, width: u16, value: BigInt) -> Option<BigInt> {
    if width == 0 {
        return None;
    }
    let in_range = match sign {
        Sign::Unsigned => {
            value.sign() != num_bigint::Sign::Minus && value.bits() <= u64::from(width)
        }
        Sign::Signed => value.bits() < u64::from(width),
    };
    if in_range {
        return Some(value);
    }
    let modulus = BigInt::from(1u8) << usize::from(width);
    let mut residue = ((value % &modulus) + &modulus) % &modulus;
    if sign == Sign::Signed && residue >= (&modulus >> 1usize) {
        residue -= modulus;
    }
    Some(residue)
}

fn unsigned(width: u16, value: BigInt) -> Option<BigInt> {
    normalise(Sign::Unsigned, width, value)
}

fn unary(op: UnOp, value: Scalar) -> Option<Scalar> {
    match (op, value) {
        (UnOp::Not, Scalar::Bool(value)) => Some(Scalar::Bool(!value)),
        (UnOp::Negate, Scalar::Int(value)) => Some(Scalar::Int(-value)),
        (op, value) => {
            let Scalar::Bits(sign, width, value) = value else {
                return None;
            };
            let out = match op {
                UnOp::Negate => -value,
                UnOp::BwCompl => !value,
                UnOp::BwClz => BigInt::from(width) - BigInt::from(unsigned(width, value)?.bits()),
                UnOp::BwCtz => BigInt::from(
                    unsigned(width, value)?
                        .trailing_zeros()
                        .unwrap_or(u64::from(width)),
                ),
                UnOp::BwFfs => BigInt::from(
                    unsigned(width, value)?
                        .trailing_zeros()
                        .map_or(0, |zeros| zeros + 1),
                ),
                UnOp::BwFls => BigInt::from(unsigned(width, value)?.bits()),
                _ => return None,
            };
            bits(sign, width, out)
        }
    }
}

fn binary(op: BinOp, left: Scalar, right: Scalar) -> Option<Scalar> {
    match (op, left, right) {
        (BinOp::LtPointer, Scalar::Pointer { addr: a, .. }, Scalar::Pointer { addr: b, .. }) => {
            Some(Scalar::Bool(a < b))
        }
        (BinOp::LePointer, Scalar::Pointer { addr: a, .. }, Scalar::Pointer { addr: b, .. }) => {
            Some(Scalar::Bool(a <= b))
        }
        (op, Scalar::Int(a), Scalar::Int(b)) => match op {
            BinOp::Lt => Some(Scalar::Bool(a < b)),
            BinOp::Le => Some(Scalar::Bool(a <= b)),
            _ => int_binary(op, a, b).map(Scalar::Int),
        },
        (op, left, right) => {
            let Scalar::Bits(ls, lw, a) = left else {
                return None;
            };
            let Scalar::Bits(rs, rw, b) = right else {
                return None;
            };
            if lw == 0 || (ls, lw) != (rs, rw) {
                return None;
            }
            if op == BinOp::Lt {
                return Some(Scalar::Bool(a < b));
            }
            if op == BinOp::Le {
                return Some(Scalar::Bool(a <= b));
            }
            bits_binary(op, ls, lw, a, b)
        }
    }
}

fn int_binary(op: BinOp, a: BigInt, b: BigInt) -> Option<BigInt> {
    Some(match op {
        BinOp::Add => a + b,
        BinOp::Sub => a - b,
        BinOp::Mul => a * b,
        BinOp::Div if b.sign() != num_bigint::Sign::NoSign => {
            let quotient = &a / &b;
            let remainder = &a - &quotient * &b;
            if remainder.sign() == num_bigint::Sign::Minus {
                if b.sign() == num_bigint::Sign::Minus {
                    quotient + 1
                } else {
                    quotient - 1
                }
            } else {
                quotient
            }
        }
        BinOp::Rem | BinOp::Mod if b.sign() != num_bigint::Sign::NoSign => {
            let remainder = a % &b;
            let modulus = if remainder.sign() == num_bigint::Sign::Minus {
                if b.sign() == num_bigint::Sign::Minus {
                    remainder - &b
                } else {
                    remainder + &b
                }
            } else {
                remainder
            };
            if op == BinOp::Rem && b.sign() == num_bigint::Sign::Minus {
                -modulus
            } else {
                modulus
            }
        }
        BinOp::Exp => a.pow(u32::try_from(b).ok()?),
        BinOp::Min => a.min(b),
        BinOp::Max => a.max(b),
        BinOp::BwXor => a ^ b,
        BinOp::BwAnd => a & b,
        BinOp::BwOr => a | b,
        BinOp::ShiftLeft => a << usize::try_from(b).ok()?,
        BinOp::ShiftRight => a >> usize::try_from(b).ok()?,
        _ => return None,
    })
}

fn bits_binary(op: BinOp, sign: Sign, width: u16, a: BigInt, b: BigInt) -> Option<Scalar> {
    if width == 0 {
        return None;
    }
    let value = match op {
        BinOp::Div if b.sign() != num_bigint::Sign::NoSign => a / b,
        BinOp::Rem if b.sign() != num_bigint::Sign::NoSign => a % b,
        BinOp::Mod if b.sign() != num_bigint::Sign::NoSign => {
            let remainder = a % &b;
            if remainder.sign() != num_bigint::Sign::NoSign && remainder.sign() != b.sign() {
                remainder + b
            } else {
                remainder
            }
        }
        BinOp::Exp => {
            if b.sign() == num_bigint::Sign::Minus {
                return None;
            }
            let modulus = BigInt::from(1u8) << usize::from(width);
            a.modpow(&b, &modulus)
        }
        BinOp::ShiftLeft | BinOp::ShiftRight => {
            let shift = usize::try_from(b)
                .ok()
                .filter(|s| *s < usize::from(width))?;
            if op == BinOp::ShiftLeft {
                a << shift
            } else {
                a >> shift
            }
        }
        _ => int_binary(op, a, b)?,
    };
    bits(sign, width, value)
}

fn cast(
    target: &BaseType,
    value: Scalar,
    origins: &dyn Fn(u64) -> PointerOrigin,
) -> Option<Scalar> {
    match (value, target) {
        (value @ Scalar::Pointer { .. }, BaseType::Loc(_)) => Some(value),
        (Scalar::Int(n) | Scalar::Bits(_, _, n), BaseType::Integer) => Some(Scalar::Int(n)),
        (Scalar::Int(n) | Scalar::Bits(_, _, n), BaseType::Bits(sign, width)) => {
            bits(*sign, *width, n)
        }
        (Scalar::Pointer { addr, .. }, BaseType::Bits(sign, width)) => bits(*sign, *width, addr),
        (Scalar::Bits(_, _, n), BaseType::Loc(_)) => {
            let addr = u64::try_from(normalise(Sign::Unsigned, 64, n)?).ok()?;
            Some(pointer_scalar(addr, origins))
        }
        _ => None,
    }
}

fn pointer_value(addr: u64, origin: PointerOrigin) -> Value {
    Value::Scalar(Scalar::Pointer { addr, origin })
}

fn unsigned64(value: Scalar) -> Option<u64> {
    match value {
        Scalar::Int(n) | Scalar::Bits(_, _, n) => {
            u64::try_from(normalise(Sign::Unsigned, 64, n)?).ok()
        }
        _ => None,
    }
}

fn number(value: impl Into<BigInt>, bt: &BaseType) -> Option<Scalar> {
    match bt {
        BaseType::Integer => Some(Scalar::Int(value.into())),
        BaseType::Bits(sign, width) => bits(*sign, *width, value),
        _ => None,
    }
}

const MAX_VALUE_CHECK_DEPTH: usize = 64;

fn value_check(
    good: bool,
    ct: &Sctype,
    about: &IndexTerm,
    env: &Bindings,
    context: Context<'_>,
    eval_depth: usize,
    type_depth: usize,
) -> Option<bool> {
    let module = context.module;
    if type_depth >= MAX_VALUE_CHECK_DEPTH {
        return None;
    }
    let deeper = type_depth + 1;
    match ct {
        Sctype::Void | Sctype::Byte => Some(true),
        Sctype::Integer(_) => {
            let value = eval_at(about, env, context, eval_depth)?;
            check_value(good, ct, &value, module, type_depth)
        }
        Sctype::Pointer(_) if good => {
            let value = eval_at(about, env, context, eval_depth)?;
            check_value(good, ct, &value, module, type_depth)
        }
        Sctype::Pointer(_) => Some(true),
        Sctype::Struct(_) => {
            if !value_check_needs_value(good, ct, module, type_depth)? {
                return Some(true);
            }
            let value = eval_at(about, env, context, eval_depth)?;
            check_value(good, ct, &value, module, type_depth)
        }
        Sctype::Array(item, Some(length)) => {
            let length = i128::try_from(*length).ok()?;
            if length == 0 {
                return Some(true);
            }
            if !matches!(about.bt, BaseType::Map(_, _)) {
                return None;
            }
            if !value_check_needs_value(good, item, module, deeper)? {
                return Some(true);
            }
            let value = eval_at(about, env, context, eval_depth)?;
            check_value(good, ct, &value, module, type_depth)
        }
        Sctype::Array(_, None) | Sctype::Function { .. } => None,
    }
}

fn value_check_needs_value(
    good: bool,
    ct: &Sctype,
    module: &SpecModule,
    depth: usize,
) -> Option<bool> {
    if depth >= MAX_VALUE_CHECK_DEPTH {
        return None;
    }
    let deeper = depth + 1;
    match ct {
        Sctype::Void | Sctype::Byte => Some(false),
        Sctype::Integer(_) => Some(true),
        Sctype::Pointer(_) => Some(good),
        Sctype::Struct(tag) => {
            let def = module
                .types
                .iter()
                .find(|def| def.tag.as_ref() == Some(tag))?;
            let mut needed = false;
            for member in &def.fields {
                let member_ct = ctype(module, &member.ty)?;
                member_ct.to_base_type()?;
                needed |= value_check_needs_value(good, &member_ct, module, deeper)?;
            }
            Some(needed)
        }
        Sctype::Array(item, Some(0)) => {
            let _ = item;
            Some(false)
        }
        Sctype::Array(item, Some(_)) => value_check_needs_value(good, item, module, deeper),
        Sctype::Array(_, None) | Sctype::Function { .. } => None,
    }
}

fn check_value(
    good: bool,
    ct: &Sctype,
    value: &Value,
    module: &SpecModule,
    depth: usize,
) -> Option<bool> {
    if depth >= MAX_VALUE_CHECK_DEPTH {
        return None;
    }
    let deeper = depth + 1;
    match ct {
        Sctype::Void | Sctype::Byte => Some(true),
        Sctype::Integer(ty) => {
            let n = match value {
                Value::Scalar(Scalar::Int(n) | Scalar::Bits(_, _, n)) => n,
                _ => return None,
            };
            let (lo, hi) = ty.range()?;
            Some((lo..=hi).contains(n))
        }
        Sctype::Pointer(_) if !good => Some(true),
        Sctype::Pointer(pointee) => {
            let Value::Scalar(Scalar::Pointer { addr, .. }) = value else {
                return None;
            };
            let align = match &**pointee {
                Sctype::Void | Sctype::Function { .. } => 1,
                other => align_of(module, other)?,
            };
            Some(*addr % align == 0)
        }
        Sctype::Struct(tag) => {
            let Value::Struct { fields, .. } = value else {
                return None;
            };
            let def = module
                .types
                .iter()
                .find(|def| def.tag.as_ref() == Some(tag))?;
            for member in &def.fields {
                let member_ct = ctype(module, &member.ty)?;
                member_ct.to_base_type()?;
                if value_check_needs_value(good, &member_ct, module, deeper)? {
                    let member_value = field(fields, &Id(member.name.clone()))?;
                    if !check_value(good, &member_ct, member_value, module, deeper)? {
                        return Some(false);
                    }
                }
            }
            Some(true)
        }
        Sctype::Array(item, Some(length)) => {
            let length = i128::try_from(*length).ok()?;
            if length == 0 {
                return Some(true);
            }
            if !value_check_needs_value(good, item, module, deeper)? {
                return Some(true);
            }
            let Value::Map { entries, default } = value else {
                return None;
            };
            for index in 0..length {
                let item_value = entries
                    .binary_search_by_key(&index, |(key, _)| *key)
                    .ok()
                    .map(|at| &entries[at].1)
                    .or(default.as_deref())?;
                if !check_value(good, item, item_value, module, deeper)? {
                    return Some(false);
                }
            }
            Some(true)
        }
        Sctype::Array(_, None) | Sctype::Function { .. } => None,
    }
}

pub fn check_constraint_with_defaults(
    constraint: &crate::spec::LogicalConstraint,
    env: &SymMap<Value>,
    module: &SpecModule,
    origins: &dyn Fn(u64) -> PointerOrigin,
    defaults: &DefaultChoices,
) -> Option<bool> {
    match constraint {
        crate::spec::LogicalConstraint::T(term) => {
            boolean(eval_with_defaults(term, env, module, origins, defaults)?)
        }
        crate::spec::LogicalConstraint::Forall {
            var,
            permission,
            body,
        } => {
            let bounds = check_quantified_permission(permission, &var.0).ok()?;
            let range = finite_domain_with_defaults(&bounds, env, module, origins, defaults)?;
            let env = Bindings::root(env);
            resolve_calls(module, origins, defaults, |context| {
                forall_range(var, Some(permission), body, range.clone(), &env, context, 0)
            })
        }
    }
}

fn forall_range(
    var: &(Sym, BaseType),
    guard: Option<&IndexTerm>,
    body: &IndexTerm,
    range: std::ops::Range<i128>,
    env: &Bindings,
    context: Context<'_>,
    depth: usize,
) -> Option<bool> {
    for i in range {
        let value = match &var.1 {
            BaseType::Bits(sign, width) => Value::Scalar(bits(*sign, *width, i)?),
            _ => Value::Scalar(Scalar::Int(i.into())),
        };
        let nested = env.push(&var.0, value);
        if let Some(guard) = guard {
            match eval_at(guard, &nested, context, depth).and_then(boolean) {
                Some(true) => {}
                Some(false) => continue,
                None => return None,
            }
        }
        if !boolean(eval_at(body, &nested, context, depth)?)? {
            return Some(false);
        }
    }
    Some(true)
}

pub fn finite_domain_with_defaults(
    bounds: &SymbolicMapBounds,
    env: &SymMap<Value>,
    module: &SpecModule,
    origins: &dyn Fn(u64) -> PointerOrigin,
    defaults: &DefaultChoices,
) -> Option<std::ops::Range<i128>> {
    let mut start = integer(eval_with_defaults(
        &bounds.lower,
        env,
        module,
        origins,
        defaults,
    )?)?;
    if !bounds.lower_inclusive {
        start = start.checked_add(1)?;
    }
    let mut end = integer(eval_with_defaults(
        &bounds.upper,
        env,
        module,
        origins,
        defaults,
    )?)?;
    if bounds.upper_inclusive {
        end = end.checked_add(1)?;
    }
    Some(start..end.max(start))
}

#[derive(Debug, Clone)]
pub struct LayoutMember {
    pub id: Id,
    pub ct: Sctype,
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub size: u64,
    pub align: u64,
    pub members: Vec<LayoutMember>,
}

pub fn size_of(module: &SpecModule, ct: &Sctype) -> Option<u64> {
    size_align(module, ct, 0).map(|v| v.0)
}

fn align_of(module: &SpecModule, ct: &Sctype) -> Option<u64> {
    size_align(module, ct, 0).map(|v| v.1)
}

fn size_align(module: &SpecModule, ct: &Sctype, depth: usize) -> Option<(u64, u64)> {
    if depth >= 64 {
        return None;
    }
    match ct {
        Sctype::Void | Sctype::Function { .. } | Sctype::Array(_, None) => None,
        Sctype::Byte | Sctype::Pointer(_) | Sctype::Integer(_) => {
            ct.byte_size().map(|size| (size, size))
        }
        Sctype::Array(element, Some(length)) => {
            let (size, align) = size_align(module, element, depth + 1)?;
            Some((size.checked_mul(u64::try_from(*length).ok()?)?, align))
        }
        Sctype::Struct(tag) => {
            let layout = struct_layout_at(module, tag, depth + 1)?;
            Some((layout.size, layout.align))
        }
    }
}

pub fn struct_layout(module: &SpecModule, tag: &Sym) -> Option<Layout> {
    struct_layout_at(module, tag, 0)
}

fn struct_layout_at(module: &SpecModule, tag: &Sym, depth: usize) -> Option<Layout> {
    if depth >= 64 {
        return None;
    }
    let def = module
        .types
        .iter()
        .find(|def| def.tag.as_ref() == Some(tag))?;
    let mut members = Vec::with_capacity(def.fields.len());
    let (mut end, mut aggregate_align) = (0u64, 1u64);
    for field in &def.fields {
        let ct = ctype(module, &field.ty)?;
        let (size, align) = size_align(module, &ct, depth + 1)?;
        let offset = align_up(end, align)?;
        members.push(LayoutMember {
            id: Id(field.name.clone()),
            ct,
            offset,
            size,
        });
        end = offset.checked_add(size)?;
        aggregate_align = aggregate_align.max(align);
    }
    Some(Layout {
        size: align_up(end, aggregate_align)?,
        align: aggregate_align,
        members,
    })
}

fn ctype(module: &SpecModule, ct: &CType) -> Option<Sctype> {
    ct.to_sctype(&|name| {
        module
            .types
            .iter()
            .find(|def| def.name == name)
            .and_then(|def| def.tag.clone())
    })
}

fn align_up(value: u64, align: u64) -> Option<u64> {
    match value % align {
        0 => Some(value),
        rem => value.checked_add(align - rem),
    }
}

pub fn decode(
    bytes: &[u8],
    ct: &Sctype,
    expected: &BaseType,
    module: &SpecModule,
    origins: &dyn Fn(u64) -> PointerOrigin,
) -> Option<Value> {
    if let (Sctype::Struct(tag), BaseType::Struct(expected_tag)) = (ct, expected) {
        if tag != expected_tag {
            return None;
        }
        let layout = struct_layout(module, tag)?;
        if bytes.len() as u64 != layout.size {
            return None;
        }
        let fields = layout
            .members
            .iter()
            .map(|member| {
                let start = usize::try_from(member.offset).ok()?;
                let end = start.checked_add(usize::try_from(member.size).ok()?)?;
                let bt = member.ct.to_base_type()?;
                Some((
                    member.id.clone(),
                    decode(bytes.get(start..end)?, &member.ct, &bt, module, origins)?,
                ))
            })
            .collect::<Option<_>>()?;
        return Some(Value::Struct {
            tag: tag.clone(),
            fields,
        });
    }
    if bytes.len() as u64 != size_of(module, ct)? {
        return None;
    }
    match (ct, expected) {
        (Sctype::Array(element, Some(length)), BaseType::Map(key, value))
            if **key == BaseType::Bits(Sign::Unsigned, 64) =>
        {
            let size = usize::try_from(size_of(module, element)?).ok()?;
            let mut entries = Vec::with_capacity(*length);
            for index in 0..*length {
                let start = index.checked_mul(size)?;
                entries.push((
                    index as i128,
                    decode(
                        bytes.get(start..start + size)?,
                        element,
                        value,
                        module,
                        origins,
                    )?,
                ));
            }
            Some(Value::Map {
                entries,
                default: None,
            })
        }
        (Sctype::Integer(IntegerType::Bool), BaseType::Bool) => match bytes {
            [0] => Some(bool_value(false)),
            [1] => Some(bool_value(true)),
            _ => None,
        },
        (Sctype::Integer(ty), BaseType::Bits(sign, width)) => {
            let _ = ty.is_signed()?;
            let n = little_int(bytes, *sign == Sign::Signed)?;
            bits(*sign, *width, n).map(Value::Scalar)
        }
        (Sctype::Integer(ty), BaseType::Integer) => {
            little_int(bytes, ty.is_signed()?).map(|n| Value::Scalar(Scalar::Int(n)))
        }
        (Sctype::Byte, BaseType::MemByte) => {
            bits(Sign::Unsigned, 8, *bytes.first()?).map(Value::Scalar)
        }
        (Sctype::Pointer(_), BaseType::Loc(_)) => {
            let addr = u64::from_le_bytes(bytes.try_into().ok()?);
            Some(Value::Scalar(pointer_scalar(addr, origins)))
        }
        _ => None,
    }
}

pub fn pointer_scalar(addr: u64, origins: &dyn Fn(u64) -> PointerOrigin) -> Scalar {
    if addr == 0 {
        Scalar::pointer(addr)
    } else {
        Scalar::Pointer {
            addr,
            origin: origins(addr),
        }
    }
}

fn little_int(bytes: &[u8], signed: bool) -> Option<BigInt> {
    if bytes.is_empty() || bytes.len() > 8 {
        return None;
    }
    Some(if signed {
        BigInt::from_signed_bytes_le(bytes)
    } else {
        BigInt::from_bytes_le(num_bigint::Sign::Plus, bytes)
    })
}
