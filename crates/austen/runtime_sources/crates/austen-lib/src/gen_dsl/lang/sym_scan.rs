use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::datatype::DatatypeDef;
use crate::gen_dsl::lang::gen_def::GenDef;
use crate::gen_dsl::lang::gen_module::GenModule;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind, ScopeName};
use crate::gen_dsl::lang::index_term::{Const, IndexTerm, Pattern, PatternKind, Term};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::logical_function::LogicalFunctionDef;
use crate::gen_dsl::lang::sctype::{IntegerType, Sctype};
use crate::gen_dsl::lang::sym::Sym;

pub fn sym_watermark(def: &GenDef, ctx: &GenModule) -> u64 {
    let mut max = 0;
    scan_def(def, &mut max);
    scan_module(ctx, &mut max);
    max.saturating_add(1)
}

pub fn gen_term_watermark(term: &GenTerm, ctx: &GenModule) -> u64 {
    let mut max = 0;
    scan_gen_term(term, &mut max);
    scan_module(ctx, &mut max);
    max.saturating_add(1)
}

fn scan_module(ctx: &GenModule, max: &mut u64) {
    for (name, d) in &ctx.defs {
        bump(max, name);
        scan_def(d, max);
    }
    for (tag, sdef) in &ctx.structs {
        bump(max, tag);
        for (_, ct) in &sdef.members {
            scan_sctype(ct, max);
        }
    }
    for (address, global) in &ctx.globals {
        bump(max, address);
        scan_sctype(&global.ty, max);
    }
    for (name, f) in &ctx.functions {
        bump(max, name);
        scan_logical_function(f, max);
    }
    for (name, datatype) in &ctx.datatypes {
        bump(max, name);
        scan_datatype(datatype, max);
    }
}

pub(crate) fn max_sym_num_in_datatype(datatype: &DatatypeDef) -> u64 {
    let mut max = 0;
    scan_datatype(datatype, &mut max);
    max
}

pub(crate) fn max_sym_num_in_logical_function(f: &LogicalFunctionDef) -> u64 {
    let mut max = 0;
    scan_logical_function(f, &mut max);
    max
}

fn scan_datatype(datatype: &DatatypeDef, max: &mut u64) {
    bump(max, &datatype.name);
    for constructor in &datatype.constructors {
        bump(max, &constructor.name);
        for (_, bt) in &constructor.params {
            scan_base_type(bt, max);
        }
    }
}

fn scan_logical_function(f: &LogicalFunctionDef, max: &mut u64) {
    bump(max, &f.name);
    for (param, bt) in &f.args {
        bump(max, param);
        scan_base_type(bt, max);
    }
    scan_base_type(&f.return_bt, max);
    if let Some(body) = &f.body {
        scan_term(body.term(), max);
    }
}

pub(crate) fn max_sym_num_in_term(it: &IndexTerm) -> u64 {
    let mut max = 0;
    scan_term(it, &mut max);
    max
}

pub(crate) fn max_sym_num_in_base_type(bt: &BaseType) -> u64 {
    let mut max = 0;
    scan_base_type(bt, &mut max);
    max
}

pub(crate) fn max_sym_num_in_sctype(ct: &Sctype) -> u64 {
    let mut max = 0;
    scan_sctype(ct, &mut max);
    max
}

pub(crate) fn max_sym_num_in_constraint(lc: &LogicalConstraint) -> u64 {
    let mut max = 0;
    scan_constraint(lc, &mut max);
    max
}

pub(in crate::gen_dsl) fn bump(max: &mut u64, sym: &Sym) {
    *max = (*max).max(sym.num);
}

fn scan_def(def: &GenDef, max: &mut u64) {
    bump(max, &def.name);
    for (sym, bt) in &def.iargs {
        bump(max, sym);
        scan_base_type(bt, max);
    }
    scan_base_type(&def.oarg, max);
    for (sym, ct) in def.c_types.iter().flatten() {
        bump(max, sym);
        scan_sctype(ct, max);
    }
    for sym in def.size_args().chain(def.heap_args()) {
        bump(max, sym);
    }
    scan_gen_term(&def.body, max);
}

fn scan_gen_term(gt: &GenTerm, max: &mut u64) {
    scan_base_type(&gt.bt, max);
    match &gt.term {
        GenTermKind::Arbitrary => {}
        GenTermKind::WriteChoice { value, rest, .. } => {
            scan_term(value, max);
            scan_gen_term(rest, max);
        }
        GenTermKind::Call { fsym, args } => {
            bump(max, fsym);
            args.iter().for_each(|a| scan_term(a, max));
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => {
            scan_term(addr, max);
            scan_sctype(ct, max);
            scan_term(value, max);
            scan_gen_term(rest, max);
        }
        GenTermKind::LetStar { var, value, rest } => {
            bump(max, var);
            scan_gen_term(value, max);
            scan_gen_term(rest, max);
        }
        GenTermKind::Return(it) => scan_term(it, max),
        GenTermKind::Assert { constraint, rest } => {
            scan_constraint(constraint, max);
            scan_gen_term(rest, max);
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            scan_term(cond, max);
            scan_gen_term(then_, max);
            scan_gen_term(else_, max);
        }
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => {
            bump(max, var);
            scan_base_type(var_bt, max);
            scan_term(perm, max);
            scan_gen_term(body, max);
        }
        GenTermKind::Scope { name, body } => {
            match name {
                ScopeName::Str(_) | ScopeName::Sid(_) => {}
                ScopeName::Index(it) => scan_term(it, max),
            }
            scan_gen_term(body, max);
        }
    }
}

pub(in crate::gen_dsl) fn scan_constraint(lc: &LogicalConstraint, max: &mut u64) {
    match lc {
        LogicalConstraint::T(it) => scan_term(it, max),
        LogicalConstraint::Forall {
            var,
            permission,
            body,
        } => {
            bump(max, &var.0);
            scan_base_type(&var.1, max);
            scan_term(permission, max);
            scan_term(body, max);
        }
    }
}

pub(in crate::gen_dsl) fn scan_base_type(bt: &BaseType, max: &mut u64) {
    match bt {
        BaseType::Unit
        | BaseType::Bool
        | BaseType::Integer
        | BaseType::MemByte
        | BaseType::Bits(_, _)
        | BaseType::Real
        | BaseType::AllocId
        | BaseType::CType => {}
        BaseType::Loc(pointee) => {
            if let Some(ct) = pointee.as_deref() {
                scan_sctype(ct, max);
            }
        }
        BaseType::Struct(sym) | BaseType::Datatype(sym) => bump(max, sym),
        BaseType::Record(members) => members.iter().for_each(|(_, bt)| scan_base_type(bt, max)),
        BaseType::Map(k, v) => {
            scan_base_type(k, max);
            scan_base_type(v, max);
        }
        BaseType::List(inner) | BaseType::Set(inner) | BaseType::Option(inner) => {
            scan_base_type(inner, max)
        }
        BaseType::Tuple(bts) => bts.iter().for_each(|bt| scan_base_type(bt, max)),
    }
}

pub(in crate::gen_dsl) fn scan_sctype(ct: &Sctype, max: &mut u64) {
    match ct {
        Sctype::Void | Sctype::Byte => {}
        Sctype::Integer(it) => scan_integer_type(it, max),
        Sctype::Array(elem, _) | Sctype::Pointer(elem) => scan_sctype(elem, max),
        Sctype::Struct(sym) => bump(max, sym),
        Sctype::Function {
            return_type,
            args,
            variadic: _,
        } => {
            scan_sctype(&return_type.1, max);
            args.iter().for_each(|(arg, _)| scan_sctype(arg, max));
        }
    }
}

pub(in crate::gen_dsl) fn scan_integer_type(it: &IntegerType, max: &mut u64) {
    match it {
        IntegerType::Char
        | IntegerType::Bool
        | IntegerType::Signed(_)
        | IntegerType::Unsigned(_)
        | IntegerType::WcharT
        | IntegerType::WintT
        | IntegerType::SizeT
        | IntegerType::PtrdiffT
        | IntegerType::PtraddrT => {}
        IntegerType::Enum(sym) => bump(max, sym),
    }
}

fn scan_const(c: &Const, max: &mut u64) {
    match c {
        Const::Z(_)
        | Const::Bits(_, _)
        | Const::Q(_)
        | Const::MemByte { .. }
        | Const::Pointer { .. }
        | Const::AllocId(_)
        | Const::Bool(_)
        | Const::Unit
        | Const::Null => {}
        Const::CTypeConst(ct) => scan_sctype(ct, max),
        Const::InputDefault(bt) | Const::Unknown(bt) => scan_base_type(bt, max),
    }
}

fn scan_pattern(pat: &Pattern, max: &mut u64) {
    scan_base_type(&pat.bt, max);
    match &pat.pattern {
        PatternKind::Sym(sym) => bump(max, sym),
        PatternKind::Wild => {}
        PatternKind::Constructor(sym, args) => {
            bump(max, sym);
            args.iter().for_each(|(_, p)| scan_pattern(p, max));
        }
    }
}

pub(in crate::gen_dsl) fn scan_term(it: &IndexTerm, max: &mut u64) {
    scan_base_type(&it.bt, max);
    let mut go = |t: &IndexTerm| scan_term(t, max);
    match &it.term {
        Term::Const(c) => scan_const(c, max),
        Term::Sym(sym) => bump(max, sym),
        Term::Unop(_, t)
        | Term::NthTuple(_, t)
        | Term::StructMember(t, _)
        | Term::RecordMember(t, _)
        | Term::HasAllocId(t)
        | Term::Head(t)
        | Term::Tail(t)
        | Term::CnSome(t)
        | Term::IsSome(t)
        | Term::GetOpt(t) => go(t),
        Term::Binop(_, a, b) | Term::Cons(a, b) | Term::MapGet(a, b) => {
            go(a);
            go(b);
        }
        Term::CopyAllocId { addr, loc } => {
            go(addr);
            go(loc);
        }
        Term::Aligned { t, align } => {
            go(t);
            go(align);
        }
        Term::Ite(a, b, c) | Term::MapSet(a, b, c) => {
            go(a);
            go(b);
            go(c);
        }
        Term::EachI { var, body, .. } => {
            bump(max, &var.0);
            scan_base_type(&var.1, max);
            scan_term(body, max);
        }
        Term::Tuple(ts) => ts.iter().for_each(go),
        Term::Struct(tag, members) | Term::Constructor(tag, members) => {
            bump(max, tag);
            members.iter().for_each(|(_, t)| scan_term(t, max));
        }
        Term::Record(members) => members.iter().for_each(|(_, t)| scan_term(t, max)),
        Term::StructUpdate { target, value, .. } | Term::RecordUpdate { target, value, .. } => {
            go(target);
            go(value);
        }
        Term::MemberShift(base, tag, _) => {
            scan_term(base, max);
            bump(max, tag);
        }
        Term::ArrayShift { base, ct, index } => {
            scan_term(base, max);
            scan_sctype(ct, max);
            scan_term(index, max);
        }
        Term::SizeOf(ct) => scan_sctype(ct, max),
        Term::OffsetOf(tag, _) => bump(max, tag),
        Term::Nil(bt) | Term::CnNone(bt) => scan_base_type(bt, max),
        Term::MapConst(bt, t) | Term::Cast(bt, t) => {
            scan_base_type(bt, max);
            scan_term(t, max);
        }
        Term::Representable(ct, t) | Term::Good(ct, t) => {
            scan_sctype(ct, max);
            scan_term(t, max);
        }
        Term::WrapI(ity, t) => {
            scan_integer_type(ity, max);
            scan_term(t, max);
        }
        Term::MapDef(var, body) => {
            bump(max, &var.0);
            scan_base_type(&var.1, max);
            scan_term(body, max);
        }
        Term::Apply(fsym, args) => {
            bump(max, fsym);
            args.iter().for_each(|t| scan_term(t, max));
        }
        Term::Let((sym, bound), body) => {
            bump(max, sym);
            scan_term(bound, max);
            scan_term(body, max);
        }
        Term::Match(scrutinee, cases) => {
            scan_term(scrutinee, max);
            for (pat, body) in cases {
                scan_pattern(pat, max);
                scan_term(body, max);
            }
        }
    }
}
