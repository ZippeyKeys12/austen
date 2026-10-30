use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

mod defaults;
pub use defaults::{DefaultChoice, DefaultChoices, DefaultError, DefaultTypes, DefaultTypesError};

use num_bigint::BigInt;

use crate::gen_dsl::lang::base_type::{BaseType, Sign, normalise_to_range_big};
use crate::gen_dsl::lang::bindings::Bindings;
use crate::gen_dsl::lang::datatype::ConstructorDef;
use crate::gen_dsl::lang::gen_module::{GenDeclarations, GenModule};
use crate::gen_dsl::lang::id::Id;
use crate::gen_dsl::lang::index_term::{BinOp, Const, IndexTerm, Term, UnOp};
use crate::gen_dsl::lang::sym::{Sym, SymMap};
use crate::gen_dsl::lang::value::{PointerOrigin, Scalar, Value};
use crate::gen_dsl::lang::value_check;

pub const MAX_EVAL_DEPTH: usize = 128;

pub const MAX_LOGICAL_CALLS: usize = 4096;

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
            Value::Unit => 8u8.hash(state),
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
struct EvalContext<'a> {
    declarations: GenDeclarations<'a>,
    defaults: &'a dyn Fn(&BaseType) -> Result<Value, DefaultError>,
    calls: Option<&'a RefCell<Calls>>,
}

#[derive(Debug)]
pub struct Evaluator<'a> {
    declarations: GenDeclarations<'a>,
    defaults: RefCell<defaults::Runtime<'a>>,
    default_error: RefCell<Option<DefaultError>>,
}

impl Evaluator<'static> {
    pub const fn empty() -> Self {
        Self::from_declarations(GenDeclarations::empty())
    }
}

impl Default for Evaluator<'static> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<'a> Evaluator<'a> {
    pub fn new(module: &'a GenModule) -> Self {
        Self::from_declarations(module.declarations())
    }

    pub const fn from_declarations(declarations: GenDeclarations<'a>) -> Self {
        Self {
            declarations,
            defaults: RefCell::new(defaults::Runtime::Disabled),
            default_error: RefCell::new(None),
        }
    }

    pub fn eval(&self, it: &IndexTerm, env: &SymMap<Value>) -> Option<Value> {
        let env = Bindings::root(env);
        resolve_calls(self, |context| eval_at(it, &env, context, 0))
    }

    pub fn eval_checked(
        &self,
        it: &IndexTerm,
        env: &SymMap<Value>,
    ) -> Result<Option<Value>, DefaultError> {
        let value = self.eval(it, env);
        match self.default_error() {
            Some(error) => Err(error),
            None => Ok(value),
        }
    }

    pub fn eval_forall(
        &self,
        var: &(Sym, BaseType),
        permission: &IndexTerm,
        body: &IndexTerm,
        range: std::ops::Range<i128>,
        env: &SymMap<Value>,
    ) -> Option<bool> {
        let env = Bindings::root(env);
        resolve_calls(self, |context| {
            bounded_forall(var, Some(permission), body, range.clone(), &env, context, 0)
        })
    }
}

pub fn eval(it: &IndexTerm, env: &SymMap<Value>) -> Option<Value> {
    Evaluator::empty().eval(it, env)
}

pub fn eval_in(it: &IndexTerm, env: &SymMap<Value>, ctx: &GenModule) -> Option<Value> {
    Evaluator::new(ctx).eval(it, env)
}

fn resolve_calls<T>(
    evaluator: &Evaluator<'_>,
    mut root: impl FnMut(EvalContext<'_>) -> Option<T>,
) -> Option<T> {
    let calls = RefCell::new(Calls::default());
    let defaults = |ty: &BaseType| evaluator.resolve_default(ty);
    let context = EvalContext {
        declarations: evaluator.declarations,
        defaults: &defaults,
        calls: Some(&calls),
    };
    root(context)
}

fn drive_call(context: EvalContext<'_>, initial: LogicalCall) -> Option<Value> {
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

fn eval_pending_call(context: EvalContext<'_>) -> Option<Value> {
    let call = context.calls?.borrow().pending.last()?.call.clone();
    let function = context.declarations.find_function(&call.name)?;
    if function.args.len() != call.args.len() {
        return None;
    }
    let body = function.body.as_ref()?.term();
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

fn eval_at(it: &IndexTerm, env: &Bindings, ctx: EvalContext<'_>, depth: usize) -> Option<Value> {
    if depth >= MAX_EVAL_DEPTH {
        return None;
    }
    let deeper = depth + 1;
    match &it.term {
        Term::Const(Const::InputDefault(bt)) => (ctx.defaults)(bt).ok(),
        Term::Const(c) => eval_const(c),
        Term::Sym(s) => env.get(s).cloned(),
        Term::Unop(op, t) => {
            eval_unop(*op, eval_at(t, env, ctx, deeper)?.as_scalar()?).map(Value::Scalar)
        }
        Term::Binop(op, a, b) => eval_binop(*op, a, b, env, ctx, deeper),
        Term::Ite(c, a, b) => match eval_at(c, env, ctx, deeper)?.as_bool()? {
            true => eval_at(a, env, ctx, deeper),
            false => eval_at(b, env, ctx, deeper),
        },
        Term::EachI {
            start,
            var,
            end,
            body,
        } => eval_each_i(*start, var, *end, body, env, ctx, deeper),
        Term::Let((sym, bound), body) => {
            let value = eval_at(bound, env, ctx, deeper)?;
            eval_at(body, &env.push(sym, value), ctx, deeper)
        }
        Term::Struct(tag, fields) => {
            let fields = fields
                .iter()
                .map(|(id, t)| Some((id.clone(), eval_at(t, env, ctx, deeper)?)))
                .collect::<Option<Vec<_>>>()?;
            Some(Value::Struct {
                tag: tag.clone(),
                fields,
            })
        }
        Term::StructMember(t, id) => eval_at(t, env, ctx, deeper)?.field(id).cloned(),
        Term::StructUpdate {
            target,
            member,
            value,
        } => {
            let mut aggregate = eval_at(target, env, ctx, deeper)?;
            *aggregate.field_mut(member)? = eval_at(value, env, ctx, deeper)?;
            Some(aggregate)
        }
        Term::Record(fields) => {
            let fields = fields
                .iter()
                .map(|(id, t)| Some((id.clone(), eval_at(t, env, ctx, deeper)?)))
                .collect::<Option<Vec<_>>>()?;
            Some(Value::Record(fields))
        }
        Term::RecordMember(t, id) => eval_at(t, env, ctx, deeper)?.record_field(id).cloned(),
        Term::RecordUpdate {
            target,
            member,
            value,
        } => {
            let mut aggregate = eval_at(target, env, ctx, deeper)?;
            *aggregate.record_field_mut(member)? = eval_at(value, env, ctx, deeper)?;
            Some(aggregate)
        }

        Term::Cast(target, e) => {
            eval_cast(target, eval_at(e, env, ctx, deeper)?.as_scalar()?).map(Value::Scalar)
        }
        Term::SizeOf(ct) => num_at(
            i128::from(ct.byte_size_in(ctx.declarations.structs())?),
            &it.bt,
        )
        .map(Value::Scalar),
        Term::OffsetOf(tag, member) => num_at(
            i128::from(offset_of(ctx.declarations, tag, member)?),
            &it.bt,
        )
        .map(Value::Scalar),
        Term::MemberShift(base, tag, member) => {
            let (addr, origin) = pointer_at(base, env, ctx, deeper)?;
            let offset = offset_of(ctx.declarations, tag, member)?;
            Some(pointer(addr.wrapping_add(offset), origin))
        }
        Term::ArrayShift { base, ct, index } => {
            let (addr, origin) = pointer_at(base, env, ctx, deeper)?;
            let size = ct.byte_size_in(ctx.declarations.structs())?;
            let index = as_u64(eval_at(index, env, ctx, deeper)?.as_scalar()?)?;
            Some(pointer(addr.wrapping_add(size.wrapping_mul(index)), origin))
        }
        Term::Representable(ct, about) => {
            let desugared =
                value_check::representable(ct, about, ctx.declarations.structs()).ok()?;
            eval_at(&desugared, env, ctx, deeper)
        }
        Term::Good(ct, about) => {
            let desugared = value_check::good(ct, about, ctx.declarations.structs()).ok()?;
            eval_at(&desugared, env, ctx, deeper)
        }
        Term::Aligned { t, align } => {
            let desugared = value_check::divisible(t, align);
            eval_at(&desugared, env, ctx, deeper)
        }

        Term::Apply(f, args) => eval_apply(f, args, env, ctx, deeper),

        Term::Constructor(name, members) => eval_constructor(name, members, env, ctx, deeper),
        Term::Match(scrutinee, arms) => {
            let v = eval_at(scrutinee, env, ctx, deeper)?;
            if !matches!(v, Value::Constructor { .. }) {
                return None;
            }
            let (bindings, body) = arms.iter().find_map(|(pattern, body)| {
                let mut bindings = Vec::new();
                pattern_bindings(pattern, &v, &mut bindings).then_some((bindings, body))
            })?;
            eval_bound(&bindings, body, env, ctx, deeper)
        }

        Term::MapConst(_, body) => Some(Value::Map {
            entries: Vec::new(),
            default: Some(Box::new(eval_at(body, env, ctx, deeper)?)),
        }),
        Term::MapSet(m, k, v) => {
            let Value::Map {
                mut entries,
                default,
            } = eval_at(m, env, ctx, deeper)?
            else {
                return None;
            };
            let key = eval_at(k, env, ctx, deeper)?.as_int()?;
            let value = eval_at(v, env, ctx, deeper)?;
            match entries.binary_search_by_key(&key, |(k, _)| *k) {
                Ok(i) => entries[i].1 = value,
                Err(i) => entries.insert(i, (key, value)),
            }
            Some(Value::Map { entries, default })
        }
        Term::MapGet(m, k) => {
            let key = eval_at(k, env, ctx, deeper)?.as_int()?;
            match &m.term {
                Term::Sym(s) => env.get(s)?.map_get(key).cloned(),
                _ => eval_at(m, env, ctx, deeper)?.map_get(key).cloned(),
            }
        }

        _ => None,
    }
}

pub(in crate::gen_dsl) fn declared_constructor<'a>(
    declarations: GenDeclarations<'a>,
    name: &Sym,
    members: &[(Id, IndexTerm)],
) -> Option<&'a ConstructorDef> {
    let decl = declarations.find_constructor(name)?;
    (members.len() == decl.params.len()
        && decl
            .params
            .iter()
            .all(|(id, _)| members.iter().any(|(member, _)| member == id)))
    .then_some(decl)
}

fn eval_constructor(
    name: &Sym,
    members: &[(Id, IndexTerm)],
    env: &Bindings,
    ctx: EvalContext<'_>,
    depth: usize,
) -> Option<Value> {
    let decl = declared_constructor(ctx.declarations, name, members)?;
    let fields = decl
        .params
        .iter()
        .map(|(id, _)| {
            let (_, term) = members.iter().find(|(m, _)| m == id)?;
            Some((id.clone(), eval_at(term, env, ctx, depth)?))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Value::Constructor {
        name: name.clone(),
        fields,
    })
}

fn pattern_bindings<'a, 'v>(
    p: &'a crate::gen_dsl::lang::index_term::Pattern,
    v: &'v Value,
    out: &mut Vec<(&'a Sym, &'v Value)>,
) -> bool {
    use crate::gen_dsl::lang::index_term::PatternKind;
    match &p.pattern {
        PatternKind::Sym(s) => {
            out.push((s, v));
            true
        }
        PatternKind::Wild => true,
        PatternKind::Constructor(cname, subs) => match v {
            Value::Constructor { name, fields } if name == cname => {
                subs.iter()
                    .all(|(id, sub)| match fields.iter().find(|(f, _)| f == id) {
                        Some((_, fv)) => pattern_bindings(sub, fv, out),
                        None => false,
                    })
            }
            _ => false,
        },
    }
}

fn eval_bound(
    bindings: &[(&Sym, &Value)],
    body: &IndexTerm,
    env: &Bindings,
    ctx: EvalContext<'_>,
    depth: usize,
) -> Option<Value> {
    if bindings.is_empty() {
        return eval_at(body, env, ctx, depth);
    }
    let arm = env.batch(
        bindings
            .iter()
            .map(|(sym, value)| (*sym, (*value).clone()))
            .collect(),
    );
    eval_at(body, &arm, ctx, depth)
}

fn eval_apply(
    f: &Sym,
    args: &[IndexTerm],
    env: &Bindings,
    ctx: EvalContext<'_>,
    depth: usize,
) -> Option<Value> {
    let def = ctx.declarations.find_function(f)?;
    let body = def.body.as_ref()?.term();
    if args.len() != def.args.len() {
        return None;
    }
    let mut values = Vec::with_capacity(args.len());
    for arg in args {
        values.push(eval_at(arg, env, ctx, depth)?);
    }

    if let Some(calls) = ctx.calls {
        let call = LogicalCall {
            name: f.clone(),
            args: values,
        };
        if let Some(value) = calls.borrow().results().get(&call).cloned() {
            return Some(value);
        }
        if calls.borrow().driving {
            calls.borrow_mut().requested = Some(call);
            return None;
        }
        return drive_call(ctx, call);
    }

    let formals = def
        .args
        .iter()
        .zip(values)
        .map(|((param, _), value)| (param, value))
        .collect();
    let root = Bindings::root(&NO_BINDINGS);
    let callee = root.batch(formals);
    eval_at(body, &callee, ctx, depth)
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
    let mut pending = vec![(left, right)];
    while let Some((left, right)) = pending.pop() {
        match (left, right) {
            (Value::Scalar(left), Value::Scalar(right)) if same_scalar(left, right) => {}
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
                if !queue_exact_fields(left_fields, right_fields, &mut pending) {
                    return false;
                }
            }
            (Value::Record(left), Value::Record(right)) => {
                if !queue_exact_fields(left, right, &mut pending) {
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
                if !queue_exact_fields(left_fields, right_fields, &mut pending) {
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

fn queue_exact_fields<'a>(
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

fn same_scalar(left: &Scalar, right: &Scalar) -> bool {
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
        ) => left_addr == right_addr && left_origin == right_origin,
        _ => left == right,
    }
}

static NO_BINDINGS: SymMap<Value> = SymMap::new();

fn pointer_at(
    t: &IndexTerm,
    env: &Bindings,
    ctx: EvalContext<'_>,
    depth: usize,
) -> Option<(u64, PointerOrigin)> {
    eval_at(t, env, ctx, depth)?.as_scalar()?.as_pointer()
}

fn pointer(addr: u64, origin: PointerOrigin) -> Value {
    Value::Scalar(Scalar::Pointer { addr, origin })
}

fn as_u64(v: Scalar) -> Option<u64> {
    u64::try_from(&to_unsigned(64, v.as_integer()?)?).ok()
}

fn num_at(n: i128, bt: &BaseType) -> Option<Scalar> {
    match bt {
        BaseType::Bits(sign, width) => Scalar::bits(*sign, *width, n),
        BaseType::Integer => Some(Scalar::Int(n.into())),
        _ => None,
    }
}

fn offset_of(ctx: GenDeclarations<'_>, tag: &Sym, member: &Id) -> Option<u64> {
    let def = ctx.find_struct(tag)?;
    let at = def.members.iter().position(|(id, _)| id == member)?;
    def.layout_in(ctx.structs())?.offsets.get(at).copied()
}

fn eval_cast(target: &BaseType, v: Scalar) -> Option<Scalar> {
    match (v, target) {
        (v @ Scalar::Int(_), BaseType::Integer) => Some(v),
        (Scalar::Bits(_, _, n), BaseType::Integer) => Some(Scalar::Int(n)),
        (Scalar::Int(n) | Scalar::Bits(_, _, n), BaseType::Bits(sign, width)) => {
            Scalar::bits(*sign, *width, n)
        }
        (v @ Scalar::Pointer { .. }, BaseType::Loc(_)) => Some(v),
        (Scalar::Pointer { addr, .. }, BaseType::Bits(s, w)) => Scalar::bits(*s, *w, addr),
        (Scalar::Bits(_, _, n), BaseType::Loc(_)) => {
            let addr = u64::try_from(Scalar::bits(Sign::Unsigned, 64, n)?.as_integer()?).ok()?;
            Some(Scalar::pointer(addr))
        }
        _ => None,
    }
}

fn eval_const(c: &Const) -> Option<Value> {
    match c {
        Const::Unit => Some(Value::Unit),
        Const::Z(n) => Some(Value::Scalar(Scalar::Int(n.clone()))),
        Const::Bits((sign, width), n) => Scalar::bits(*sign, *width, n.clone()).map(Value::Scalar),
        Const::Bool(b) => Some(Value::Scalar(Scalar::Bool(*b))),
        Const::Null => Some(Value::Scalar(Scalar::pointer(0))),
        Const::Pointer { addr, .. } => u64::try_from(addr)
            .ok()
            .map(Scalar::pointer)
            .map(Value::Scalar),
        _ => None,
    }
}

fn to_unsigned(width: u16, value: BigInt) -> Option<BigInt> {
    (width != 0).then(|| normalise_to_range_big(Sign::Unsigned, width, &value))
}

fn eval_unop(op: UnOp, v: Scalar) -> Option<Scalar> {
    use Scalar::{Bits, Bool, Int};
    let v = match (op, v) {
        (UnOp::Not, Bool(b)) => return Some(Bool(!b)),
        (UnOp::Negate, Int(n)) => return Some(Int(-n)),
        (_, v) => v,
    };
    let Bits(sign, width, n) = v else {
        return None;
    };
    let bits = |value: BigInt| Scalar::bits(sign, width, value);
    match op {
        UnOp::Negate => bits(-n),
        UnOp::BwCompl => bits(!n),
        UnOp::BwClz | UnOp::BwCtz | UnOp::BwFfs | UnOp::BwFls => {
            let u = to_unsigned(width, n)?;
            let bitlen = u.bits();
            let count = match op {
                UnOp::BwCtz => u.trailing_zeros().unwrap_or(u64::from(width)),
                UnOp::BwClz => u64::from(width) - bitlen,
                UnOp::BwFfs => u.trailing_zeros().map_or(0, |zeros| zeros + 1),
                _ => bitlen,
            };
            bits(count.into())
        }
        _ => None,
    }
}

fn eval_binop(
    op: BinOp,
    a: &IndexTerm,
    b: &IndexTerm,
    env: &Bindings,
    ctx: EvalContext<'_>,
    depth: usize,
) -> Option<Value> {
    let vbool = |v: bool| Some(Value::Scalar(Scalar::Bool(v)));
    let rhs = |env| vbool(eval_at(b, env, ctx, depth)?.as_bool()?);
    match op {
        BinOp::And => match eval_at(a, env, ctx, depth)?.as_bool()? {
            false => vbool(false),
            true => rhs(env),
        },
        BinOp::Or => match eval_at(a, env, ctx, depth)?.as_bool()? {
            true => vbool(true),
            false => rhs(env),
        },
        BinOp::Implies => match eval_at(a, env, ctx, depth)?.as_bool()? {
            false => vbool(true),
            true => rhs(env),
        },
        BinOp::Eq => {
            let a = eval_at(a, env, ctx, depth)?;
            let b = eval_at(b, env, ctx, depth)?;
            vbool(a.language_eq(&b)?)
        }
        _ => eval_strict_binop(
            op,
            eval_at(a, env, ctx, depth)?.as_scalar()?,
            eval_at(b, env, ctx, depth)?.as_scalar()?,
        )
        .map(Value::Scalar),
    }
}

fn eval_strict_binop(op: BinOp, a: Scalar, b: Scalar) -> Option<Scalar> {
    use BinOp::{Le, Lt};
    use Scalar::{Bits, Bool, Int};
    match (op, a, b) {
        (BinOp::LtPointer, Scalar::Pointer { addr: x, .. }, Scalar::Pointer { addr: y, .. }) => {
            Some(Bool(x < y))
        }
        (BinOp::LePointer, Scalar::Pointer { addr: x, .. }, Scalar::Pointer { addr: y, .. }) => {
            Some(Bool(x <= y))
        }
        (op, Int(a), Int(b)) => match op {
            Lt => Some(Bool(a < b)),
            Le => Some(Bool(a <= b)),
            _ => int_binop(op, a, b).map(Int),
        },
        (op, Bits(s1, w1, x), Bits(s2, w2, y)) if (s1, w1) == (s2, w2) => match op {
            Lt => Some(Bool(x < y)),
            Le => Some(Bool(x <= y)),
            _ => bits_binop(op, s1, w1, x, y),
        },
        _ => None,
    }
}

fn int_binop(op: BinOp, a: BigInt, b: BigInt) -> Option<BigInt> {
    use BinOp::*;
    let zero = num_bigint::Sign::NoSign;
    match op {
        Add => Some(a + b),
        Sub => Some(a - b),
        Mul => Some(a * b),
        Div if b.sign() != zero => {
            let quotient = &a / &b;
            Some(if (a % &b).sign() == num_bigint::Sign::Minus {
                quotient
                    + if b.sign() == num_bigint::Sign::Minus {
                        1
                    } else {
                        -1
                    }
            } else {
                quotient
            })
        }
        Rem | Mod if b.sign() != zero => {
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
            Some(if op == Rem && b.sign() == num_bigint::Sign::Minus {
                -modulus
            } else {
                modulus
            })
        }
        Exp => Some(a.pow(u32::try_from(b).ok()?)),
        Min => Some(a.min(b)),
        Max => Some(a.max(b)),
        BwXor => Some(a ^ b),
        BwAnd => Some(a & b),
        BwOr => Some(a | b),
        ShiftLeft => Some(a << usize::try_from(b).ok()?),
        ShiftRight => Some(a >> usize::try_from(b).ok()?),
        _ => None,
    }
}

fn bits_binop(op: BinOp, sign: Sign, width: u16, a: BigInt, b: BigInt) -> Option<Scalar> {
    use BinOp::*;
    let bits = |v: BigInt| Scalar::bits(sign, width, v);
    match op {
        Div if b.sign() != num_bigint::Sign::NoSign => bits(a / b),
        Rem if b.sign() != num_bigint::Sign::NoSign => bits(a % b),
        Mod if b.sign() != num_bigint::Sign::NoSign => {
            let remainder = a % &b;
            bits(
                if remainder.sign() != num_bigint::Sign::NoSign && remainder.sign() != b.sign() {
                    remainder + b
                } else {
                    remainder
                },
            )
        }
        Exp if b.sign() != num_bigint::Sign::Minus => {
            bits(a.modpow(&b, &(BigInt::from(1u8) << usize::from(width))))
        }
        ShiftLeft | ShiftRight => {
            let amount = usize::try_from(&b)
                .ok()
                .filter(|n| *n < usize::from(width))?;
            match op {
                ShiftLeft => bits(a << amount),
                _ => bits(a >> amount),
            }
        }
        _ => bits(int_binop(op, a, b)?),
    }
}

fn bounded_forall(
    var: &(Sym, BaseType),
    guard: Option<&IndexTerm>,
    body: &IndexTerm,
    range: std::ops::Range<i128>,
    env: &Bindings,
    ctx: EvalContext<'_>,
    depth: usize,
) -> Option<bool> {
    let (sym, bt) = var;
    for i in range {
        let v = match bt {
            BaseType::Bits(s, w) => Scalar::bits(*s, *w, i)?,
            _ => Scalar::Int(i.into()),
        };
        let env = env.push(sym, Value::Scalar(v));
        if let Some(g) = guard
            && !eval_at(g, &env, ctx, depth)?.as_bool()?
        {
            continue;
        }
        if !eval_at(body, &env, ctx, depth)?.as_bool()? {
            return Some(false);
        }
    }
    Some(true)
}

fn eval_each_i(
    start: i64,
    var: &(Sym, BaseType),
    end: i64,
    body: &IndexTerm,
    env: &Bindings,
    ctx: EvalContext<'_>,
    depth: usize,
) -> Option<Value> {
    let range = i128::from(start)..i128::from(end) + 1;
    bounded_forall(var, None, body, range, env, ctx, depth).map(|b| Value::Scalar(Scalar::Bool(b)))
}

pub fn eval_forall_in(
    var: &(Sym, BaseType),
    permission: &IndexTerm,
    body: &IndexTerm,
    range: std::ops::Range<i128>,
    env: &SymMap<Value>,
    ctx: &GenModule,
) -> Option<bool> {
    Evaluator::new(ctx).eval_forall(var, permission, body, range, env)
}

const MAX_REDUCTION_DEPTH: usize = 32;

pub fn reduce(it: &IndexTerm, env: &SymMap<IndexTerm>) -> IndexTerm {
    reduce_at(it, env, 0)
}

fn reduce_at(it: &IndexTerm, env: &SymMap<IndexTerm>, depth: usize) -> IndexTerm {
    if depth >= MAX_REDUCTION_DEPTH {
        return it.clone();
    }
    let project = |fields: &[(Id, IndexTerm)], member: &Id| {
        fields
            .iter()
            .find(|(id, _)| id == member)
            .map(|(_, value)| reduce_at(value, env, depth + 1))
    };
    match &it.term {
        Term::Sym(sym) => match env.get(sym) {
            Some(bound) => reduce_at(bound, env, depth + 1),
            None => it.clone(),
        },
        Term::StructMember(base, member) => match &reduce_at(base, env, depth + 1).term {
            Term::Struct(_, fields) => project(fields, member),
            _ => None,
        }
        .unwrap_or_else(|| it.clone()),
        Term::RecordMember(base, member) => match &reduce_at(base, env, depth + 1).term {
            Term::Record(fields) => project(fields, member),
            _ => None,
        }
        .unwrap_or_else(|| it.clone()),
        _ => it.clone(),
    }
}
