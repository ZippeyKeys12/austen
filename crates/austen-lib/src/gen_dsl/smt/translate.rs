use std::collections::BTreeMap;
use std::rc::Rc;

use crate::gen_dsl::{
    BaseType, BinOp, Const, ConstructorDef, FunctionBody, Id, IndexTerm, LogicalConstraint,
    LogicalFunctionDef, Pattern, PatternKind, Sctype, Sign, StructDef, StructLayout, Sym, SymMap,
    Term, UnOp, divisible, uintptr_bt,
};
use num_bigint::BigInt;
use z3::ast::{Array, Ast, BV, Bool, Dynamic, Int, Real, Set};
use z3::{RecFuncDecl, Sort};

use super::error::SmtError;
use super::model::ModelValue;
use super::names;
use super::pointers::PointerEncoding;
use super::sorts::{Program, Sorts, variant};

mod checked;
pub use checked::{CHECKED_TRANSLATOR_REVISION, CheckedExpr, MAX_CHECKED_EXPANSION};

pub const MAX_EACHI_EXPANSION: u128 = 4096;

pub const EXP_EXPANSION_LIMIT: u128 = 4096;

pub const MAX_TERM_DEPTH: usize = crate::gen_dsl::lang::eval::MAX_EVAL_DEPTH;

#[derive(Debug, Clone)]
pub struct SmtExpr {
    ast: Dynamic,
    bt: BaseType,
}

impl SmtExpr {
    pub fn new(ast: Dynamic, bt: BaseType) -> Self {
        SmtExpr { ast, bt }
    }

    pub fn ast(&self) -> &Dynamic {
        &self.ast
    }

    pub fn into_ast(self) -> Dynamic {
        self.ast
    }

    pub fn bt(&self) -> &BaseType {
        &self.bt
    }

    pub fn expect_bool(&self) -> Result<Bool, SmtError> {
        self.ast.as_bool().ok_or_else(|| SmtError::TypeMismatch {
            expected: BaseType::Bool,
            actual: self.bt.clone(),
        })
    }

    fn expect_bv(&self) -> Result<BV, SmtError> {
        self.ast.as_bv().ok_or_else(|| SmtError::TypeMismatch {
            expected: uintptr_bt(),
            actual: self.bt.clone(),
        })
    }

    fn expect_int(&self) -> Result<Int, SmtError> {
        self.ast.as_int().ok_or_else(|| SmtError::TypeMismatch {
            expected: BaseType::Integer,
            actual: self.bt.clone(),
        })
    }

    fn expect_real(&self) -> Result<Real, SmtError> {
        self.ast.as_real().ok_or_else(|| SmtError::TypeMismatch {
            expected: BaseType::Real,
            actual: self.bt.clone(),
        })
    }

    fn expect_set(&self) -> Result<Set, SmtError> {
        self.ast.as_set().ok_or_else(|| SmtError::TypeMismatch {
            expected: BaseType::Set(Box::new(self.bt.clone())),
            actual: self.bt.clone(),
        })
    }

    fn expect_array(&self) -> Result<Array, SmtError> {
        self.ast.as_array().ok_or_else(|| SmtError::TypeMismatch {
            expected: BaseType::make_map(self.bt.clone(), self.bt.clone()),
            actual: self.bt.clone(),
        })
    }
}

pub enum Env<'a> {
    Empty,
    Root(&'a SymMap<SmtExpr>),
    Frame {
        parent: &'a Env<'a>,
        sym: &'a Sym,
        value: SmtExpr,
    },
}

impl<'a> Env<'a> {
    pub fn push(&'a self, sym: &'a Sym, value: SmtExpr) -> Env<'a> {
        Env::Frame {
            parent: self,
            sym,
            value,
        }
    }

    pub fn get(&self, sym: &Sym) -> Option<&SmtExpr> {
        match self {
            Env::Empty => None,
            Env::Root(map) => map.get(sym),
            Env::Frame {
                parent,
                sym: bound,
                value,
            } => {
                if *bound == sym {
                    Some(value)
                } else {
                    parent.get(sym)
                }
            }
        }
    }
}

pub struct Translator<'p> {
    sorts: Sorts<'p>,
    declared: BTreeMap<Sym, Rc<RecFuncDecl>>,
    input_defaults: BTreeMap<BaseType, SmtExpr>,
    default_parameters: BTreeMap<BaseType, SmtExpr>,
    function_defaults: BTreeMap<Sym, std::collections::BTreeSet<BaseType>>,
    binders: u64,
    depth: usize,
    layouts: BTreeMap<Sym, StructLayout>,
}

impl<'p> Translator<'p> {
    pub fn new(program: Program<'p>) -> Self {
        Self::with_pointers(program, PointerEncoding::default())
    }

    pub fn over_structs(structs: &'p [(Sym, StructDef)]) -> Self {
        Self::new(Program::new(structs))
    }

    pub fn with_pointers(program: Program<'p>, pointers: PointerEncoding) -> Self {
        Translator {
            sorts: Sorts::with_pointers(program, pointers),
            declared: BTreeMap::new(),
            input_defaults: BTreeMap::new(),
            default_parameters: BTreeMap::new(),
            function_defaults: BTreeMap::new(),
            binders: 0,
            depth: 0,
            layouts: BTreeMap::new(),
        }
    }

    pub fn pointers(&self) -> PointerEncoding {
        self.sorts.pointers()
    }

    pub fn sort(&mut self, bt: &BaseType) -> Result<Sort, SmtError> {
        self.sorts.sort(bt)
    }

    pub fn declare(&mut self, sym: &Sym, bt: &BaseType) -> Result<SmtExpr, SmtError> {
        let sort = self.sort(bt)?;
        let ast = Dynamic::new_const(names::sym_name(sym), &sort);
        Ok(SmtExpr::new(ast, bt.clone()))
    }

    pub fn declare_all(&mut self, vars: &SymMap<BaseType>) -> Result<SymMap<SmtExpr>, SmtError> {
        vars.iter()
            .map(|(sym, bt)| Ok((sym.clone(), self.declare(sym, bt)?)))
            .collect()
    }

    pub fn constraint(&mut self, env: &Env<'_>, lc: &LogicalConstraint) -> Result<Bool, SmtError> {
        match lc {
            LogicalConstraint::T(it) => self.boolean(env, it),
            LogicalConstraint::Forall {
                var,
                permission,
                body,
            } => {
                let (sym, bt) = var;
                let sort = self.sort(bt)?;
                let name = names::binder_name(sym, self.take_binder());
                let bound = Dynamic::new_const(name.as_str(), &sort);
                let proposition = {
                    let env = env.push(sym, SmtExpr::new(bound.clone(), bt.clone()));
                    let permission = self.boolean(&env, permission)?;
                    let body = self.boolean(&env, body)?;
                    permission.implies(&body)
                };
                Ok(z3::ast::forall_const(&[&bound], &[], &proposition))
            }
        }
    }

    pub fn boolean(&mut self, env: &Env<'_>, it: &IndexTerm) -> Result<Bool, SmtError> {
        self.term(env, it)?.expect_bool()
    }

    pub fn term(&mut self, env: &Env<'_>, it: &IndexTerm) -> Result<SmtExpr, SmtError> {
        self.enter()?;
        let out = self.term_at_depth(env, it);
        self.depth -= 1;
        out
    }

    fn enter(&mut self) -> Result<(), SmtError> {
        if self.depth >= MAX_TERM_DEPTH {
            return Err(SmtError::ExpansionLimit {
                what: "term nesting",
                count: self.depth as u128 + 1,
                limit: MAX_TERM_DEPTH as u128,
            });
        }
        self.depth += 1;
        Ok(())
    }

    fn term_at_depth(&mut self, env: &Env<'_>, it: &IndexTerm) -> Result<SmtExpr, SmtError> {
        match &it.term {
            Term::Const(c) => self.constant(c),
            Term::Sym(s) => env
                .get(s)
                .cloned()
                .ok_or_else(|| SmtError::UnboundSymbol { sym: s.clone() }),
            Term::Unop(op, e) => self.unop(env, *op, e),
            Term::Binop(op, a, b) => self.binop(env, *op, a, b),
            Term::Ite(c, a, b) => self.ite(env, c, a, b),
            Term::EachI {
                start,
                var,
                end,
                body,
            } => self.each_i(env, *start, var, *end, body),
            Term::Let((sym, bound), body) => {
                let value = self.term(env, bound)?;
                let env = env.push(sym, value);
                self.term(&env, body)
            }
            Term::Tuple(es) => self.tuple(env, es),
            Term::NthTuple(n, e) => self.nth_tuple(env, *n, e),
            Term::Struct(tag, fields) => self.struct_(env, tag, fields),
            Term::StructMember(e, member) => self.struct_member(env, e, member),
            Term::StructUpdate {
                target,
                member,
                value,
            } => self.struct_update(env, target, member, value),
            Term::Record(fields) => self.record(env, fields),
            Term::RecordMember(e, member) => self.record_member(env, e, member),
            Term::RecordUpdate {
                target,
                member,
                value,
            } => self.record_update(env, target, member, value),
            Term::MemberShift(base, tag, member) => self.member_shift(env, base, tag, member),
            Term::ArrayShift { base, ct, index } => self.array_shift(env, base, ct, index),
            Term::SizeOf(ct) => {
                let size = self.size_of(ct)?;
                self.numeric_literal(BigInt::from(size), &it.bt)
            }
            Term::OffsetOf(tag, member) => {
                let offset = self.offset_of(tag, member)?;
                self.numeric_literal(BigInt::from(offset), &it.bt)
            }
            Term::Nil(elem) => self.nullary_variant(BaseType::List(Box::new(elem.clone()))),
            Term::Cons(head, tail) => self.cons(env, head, tail),
            Term::Head(e) => self.list_field(env, e, true),
            Term::Tail(e) => self.list_field(env, e, false),
            Term::CnNone(elem) => self.nullary_variant(BaseType::Option(Box::new(elem.clone()))),
            Term::CnSome(e) => self.cn_some(env, e),
            Term::IsSome(e) => self.is_some(env, e),
            Term::GetOpt(e) => self.get_opt(env, e),
            Term::MapConst(key_bt, e) => self.map_const(env, key_bt, e),
            Term::MapSet(map, k, v) => self.map_set(env, map, k, v),
            Term::MapGet(map, k) => self.map_get(env, map, k),
            Term::Cast(target, e) => self.cast(env, target, e),
            Term::Representable(ct, about) => {
                let desugared = self.value_check(false, ct, about)?;
                self.term(env, &desugared)
            }
            Term::Good(ct, about) => {
                let desugared = self.value_check(true, ct, about)?;
                self.term(env, &desugared)
            }
            Term::Aligned { t, align } => {
                let desugared = divisible(t, align);
                self.term(env, &desugared)
            }
            Term::MapDef(_, _) => Err(SmtError::unsupported("MapDef", &it.bt)),
            Term::WrapI(_, _) => Err(SmtError::unsupported("WrapI", &it.bt)),
            Term::Apply(name, args) => self.apply(env, name, args),
            Term::Constructor(con, members) => self.constructor(env, &it.bt, con, members),
            Term::Match(scrutinee, arms) => self.match_(env, scrutinee, arms, &it.bt),
            Term::HasAllocId(e) => self.has_alloc_id(env, e),
            Term::CopyAllocId { addr, loc } => self.copy_alloc_id(env, addr, loc),
        }
    }

    pub fn input_default(&mut self, bt: &BaseType) -> Result<SmtExpr, SmtError> {
        crate::gen_dsl::DefaultChoice::width_for(bt)
            .map_err(|_| SmtError::UnsupportedBaseType(bt.clone()))?;
        if let Some(value) = self
            .default_parameters
            .get(bt)
            .or(self.input_defaults.get(bt))
        {
            return Ok(value.clone());
        }
        let sort = self.sort(bt)?;
        let value = SmtExpr::new(
            Dynamic::new_const(format!("austen-input-default-{bt:?}"), &sort),
            bt.clone(),
        );
        self.input_defaults.insert(bt.clone(), value.clone());
        Ok(value)
    }

    pub fn input_default_variables(&self) -> Vec<Dynamic> {
        self.input_defaults
            .values()
            .map(|value| value.ast.clone())
            .collect()
    }

    fn constant(&mut self, c: &Const) -> Result<SmtExpr, SmtError> {
        match c {
            Const::Z(n) => Ok(SmtExpr::new(
                Self::int_literal(n)?.into(),
                BaseType::Integer,
            )),
            Const::Bits((sign, width), n) => {
                let bv = Self::bv_literal(*width, n)?;
                Ok(SmtExpr::new(bv.into(), BaseType::Bits(*sign, *width)))
            }
            Const::Q(q) => {
                let num = q.numer().to_str_radix(10);
                let den = q.denom().to_str_radix(10);
                let r =
                    Real::from_rational_str(&num, &den).ok_or_else(|| SmtError::BadNumeral {
                        kind: "real",
                        value: format!("{num}/{den}"),
                    })?;
                Ok(SmtExpr::new(r.into(), BaseType::Real))
            }
            Const::Bool(b) => Ok(SmtExpr::new(Bool::from_bool(*b).into(), BaseType::Bool)),
            Const::Unit => self.build(BaseType::Unit, variant::FIRST, &[]),
            Const::Null => self.null(),
            Const::Pointer { alloc_id, addr } => self.pointer(alloc_id, addr),
            Const::InputDefault(bt) => self.input_default(bt),
            Const::Unknown(bt) => self.default_of(bt),
            Const::MemByte { .. } => Err(SmtError::UnsupportedBaseType(BaseType::MemByte)),
            Const::AllocId(n) => {
                if self.pointers().is_provenance() {
                    Ok(SmtExpr::new(
                        Self::int_literal(n)?.into(),
                        BaseType::AllocId,
                    ))
                } else {
                    Err(SmtError::NeedsProvenance { what: "AllocId" })
                }
            }
            Const::CTypeConst(_) => Err(SmtError::UnsupportedBaseType(BaseType::CType)),
        }
    }

    fn null(&mut self) -> Result<SmtExpr, SmtError> {
        match self.pointers() {
            PointerEncoding::Provenance => self.build(BaseType::Loc(None), variant::FIRST, &[]),
            PointerEncoding::Address => self.address(&BigInt::ZERO),
        }
    }

    fn pointer(&mut self, alloc_id: &BigInt, addr: &BigInt) -> Result<SmtExpr, SmtError> {
        match self.pointers() {
            PointerEncoding::Provenance => {
                let id = SmtExpr::new(Self::int_literal(alloc_id)?.into(), BaseType::AllocId);
                let bits = Self::bv_literal(64, addr)?;
                Ok(SmtExpr::new(self.aia(&id, &bits)?, BaseType::Loc(None)))
            }
            PointerEncoding::Address => self.address(addr),
        }
    }

    fn address(&mut self, addr: &BigInt) -> Result<SmtExpr, SmtError> {
        let bv = Self::bv_literal(64, addr)?;
        Ok(SmtExpr::new(bv.into(), BaseType::Loc(None)))
    }

    fn default_of(&mut self, bt: &BaseType) -> Result<SmtExpr, SmtError> {
        let none = self.nullary_variant(BaseType::Option(Box::new(bt.clone())))?;
        self.project(&none, variant::SECOND, 0, bt.clone())
    }

    fn int_literal(n: &BigInt) -> Result<Int, SmtError> {
        if let Ok(small) = i64::try_from(n) {
            return Ok(Int::from_i64(small));
        }
        let text = n.to_str_radix(10);
        text.parse::<Int>().map_err(|_| SmtError::BadNumeral {
            kind: "integer",
            value: text,
        })
    }

    fn bv_literal(width: u16, n: &BigInt) -> Result<BV, SmtError> {
        if width == 0 {
            return Err(SmtError::ZeroWidthBitvector);
        }
        let residue = crate::gen_dsl::bits_residue(width, n);
        if let Ok(small) = u64::try_from(&residue) {
            return Ok(BV::from_u64(small, u32::from(width)));
        }
        let text = residue.to_str_radix(10);
        BV::from_str(u32::from(width), &text).ok_or(SmtError::BadNumeral {
            kind: "bitvector",
            value: text,
        })
    }

    fn numeric_literal(&mut self, n: BigInt, bt: &BaseType) -> Result<SmtExpr, SmtError> {
        match bt {
            BaseType::Integer => Ok(SmtExpr::new(
                Self::int_literal(&n)?.into(),
                BaseType::Integer,
            )),
            BaseType::Bits(sign, width) => Ok(SmtExpr::new(
                Self::bv_literal(*width, &n)?.into(),
                BaseType::Bits(*sign, *width),
            )),
            other => Err(SmtError::UnsupportedBaseType(other.clone())),
        }
    }

    fn unop(&mut self, env: &Env<'_>, op: UnOp, e: &IndexTerm) -> Result<SmtExpr, SmtError> {
        let v = self.term(env, e)?;
        self.apply_unop(op, v)
    }

    fn apply_unop(&mut self, op: UnOp, v: SmtExpr) -> Result<SmtExpr, SmtError> {
        let bt = v.bt.clone();
        let same = |ast: Dynamic| Ok(SmtExpr::new(ast, bt.clone()));
        match (op, &bt) {
            (UnOp::Not, BaseType::Bool) => same(v.expect_bool()?.not().into()),
            (UnOp::Negate, BaseType::Bits(_, _)) => same(v.expect_bv()?.bvneg().into()),
            (UnOp::Negate, BaseType::Integer) => same(v.expect_int()?.unary_minus().into()),
            (UnOp::Negate, BaseType::Real) => same(v.expect_real()?.unary_minus().into()),
            (UnOp::BwCompl, BaseType::Bits(_, _)) => same(v.expect_bv()?.bvnot().into()),
            (UnOp::BwClz | UnOp::BwCtz | UnOp::BwFfs | UnOp::BwFls, BaseType::Bits(_, width)) => {
                let width = *width;
                if width == 0 {
                    return Err(SmtError::ZeroWidthBitvector);
                }
                let x = v.expect_bv()?;
                same(Self::bit_scan(op, width, &x)?.into())
            }
            _ => Err(SmtError::UnsupportedOperator {
                op: unop_name(op),
                bt,
            }),
        }
    }

    fn bit_scan(op: UnOp, width: u16, x: &BV) -> Result<BV, SmtError> {
        let lit = |k: u64| BV::from_u64(k, u32::from(width));
        Ok(match op {
            UnOp::BwClz => count_leading_zeros(width, width, x),
            UnOp::BwCtz => count_trailing_zeros(width, width, x),
            UnOp::BwFfs => {
                let ctz = count_trailing_zeros(width, width, x);
                x.eq(lit(0)).ite(&lit(0), &ctz.bvadd(lit(1)))
            }
            UnOp::BwFls => {
                let clz = count_leading_zeros(width, width, x);
                x.eq(lit(0))
                    .ite(&lit(0), &lit(u64::from(width)).bvsub(&clz))
            }
            other => {
                return Err(SmtError::UnsupportedOperator {
                    op: unop_name(other),
                    bt: BaseType::Bits(Sign::Unsigned, width),
                });
            }
        })
    }

    fn binop(
        &mut self,
        env: &Env<'_>,
        op: BinOp,
        a: &IndexTerm,
        b: &IndexTerm,
    ) -> Result<SmtExpr, SmtError> {
        if op == BinOp::SetMember {
            let (x, s) = (self.term(env, a)?, self.term(env, b)?);
            let BaseType::Set(element) = s.bt.clone() else {
                return Err(SmtError::TypeMismatch {
                    expected: BaseType::Set(Box::new(x.bt.clone())),
                    actual: s.bt,
                });
            };
            check_type(&element, &x.bt)?;
            let set = s.expect_set()?;
            return Ok(SmtExpr::new(set.member(x.ast()).into(), BaseType::Bool));
        }

        let (l, r) = (self.term(env, a)?, self.term(env, b)?);
        self.apply_binop(op, a, b, l, r)
    }

    #[inline(never)]
    fn apply_binop(
        &mut self,
        op: BinOp,
        a: &IndexTerm,
        b: &IndexTerm,
        l: SmtExpr,
        r: SmtExpr,
    ) -> Result<SmtExpr, SmtError> {
        check_type(&l.bt, &r.bt)?;
        let bt = l.bt.clone();
        let same = |ast: Dynamic| Ok(SmtExpr::new(ast, bt.clone()));
        let prop = |ast: Dynamic| Ok(SmtExpr::new(ast, BaseType::Bool));
        let bad = || SmtError::UnsupportedOperator {
            op: binop_name(op),
            bt: bt.clone(),
        };

        match op {
            BinOp::And => prop(Bool::and(&[&l.expect_bool()?, &r.expect_bool()?]).into()),
            BinOp::Or => prop(Bool::or(&[&l.expect_bool()?, &r.expect_bool()?]).into()),
            BinOp::Implies => prop(l.expect_bool()?.implies(&r.expect_bool()?).into()),

            BinOp::Add | BinOp::Sub | BinOp::Mul => match &bt {
                BaseType::Bits(_, _) => {
                    let (x, y) = (l.expect_bv()?, r.expect_bv()?);
                    same(match op {
                        BinOp::Add => x.bvadd(&y).into(),
                        BinOp::Sub => x.bvsub(&y).into(),
                        _ => x.bvmul(&y).into(),
                    })
                }
                BaseType::Integer => {
                    let (x, y) = (l.expect_int()?, r.expect_int()?);
                    same(match op {
                        BinOp::Add => Int::add(&[&x, &y]).into(),
                        BinOp::Sub => Int::sub(&[&x, &y]).into(),
                        _ => Int::mul(&[&x, &y]).into(),
                    })
                }
                BaseType::Real => {
                    let (x, y) = (l.expect_real()?, r.expect_real()?);
                    same(match op {
                        BinOp::Add => Real::add(&[&x, &y]).into(),
                        BinOp::Sub => Real::sub(&[&x, &y]).into(),
                        _ => Real::mul(&[&x, &y]).into(),
                    })
                }
                _ => Err(bad()),
            },

            BinOp::Div => match &bt {
                BaseType::Bits(Sign::Signed, _) => {
                    same(l.expect_bv()?.bvsdiv(&r.expect_bv()?).into())
                }
                BaseType::Bits(Sign::Unsigned, _) => {
                    same(l.expect_bv()?.bvudiv(&r.expect_bv()?).into())
                }
                BaseType::Integer => same(l.expect_int()?.div(&r.expect_int()?).into()),
                BaseType::Real => same(l.expect_real()?.div(&r.expect_real()?).into()),
                _ => Err(bad()),
            },
            BinOp::Rem => match &bt {
                BaseType::Bits(Sign::Signed, _) => {
                    same(l.expect_bv()?.bvsrem(&r.expect_bv()?).into())
                }
                BaseType::Bits(Sign::Unsigned, _) => {
                    same(l.expect_bv()?.bvurem(&r.expect_bv()?).into())
                }
                BaseType::Integer => same(l.expect_int()?.rem(&r.expect_int()?).into()),
                _ => Err(bad()),
            },
            BinOp::Mod => match &bt {
                BaseType::Bits(Sign::Signed, _) => {
                    same(l.expect_bv()?.bvsmod(&r.expect_bv()?).into())
                }
                BaseType::Bits(Sign::Unsigned, _) => {
                    same(l.expect_bv()?.bvurem(&r.expect_bv()?).into())
                }
                BaseType::Integer => same(l.expect_int()?.modulo(&r.expect_int()?).into()),
                _ => Err(bad()),
            },

            BinOp::Exp => self.exp(a, b, &l),

            BinOp::BwXor | BinOp::BwAnd | BinOp::BwOr => match &bt {
                BaseType::Bits(_, _) => {
                    let (x, y) = (l.expect_bv()?, r.expect_bv()?);
                    same(match op {
                        BinOp::BwXor => x.bvxor(&y).into(),
                        BinOp::BwAnd => x.bvand(&y).into(),
                        _ => x.bvor(&y).into(),
                    })
                }
                _ => Err(bad()),
            },
            BinOp::ShiftLeft => match &bt {
                BaseType::Bits(_, _) => same(l.expect_bv()?.bvshl(&r.expect_bv()?).into()),
                _ => Err(bad()),
            },
            BinOp::ShiftRight => match &bt {
                BaseType::Bits(Sign::Signed, _) => {
                    same(l.expect_bv()?.bvashr(&r.expect_bv()?).into())
                }
                BaseType::Bits(Sign::Unsigned, _) => {
                    same(l.expect_bv()?.bvlshr(&r.expect_bv()?).into())
                }
                _ => Err(bad()),
            },

            BinOp::Lt | BinOp::Le => prop(compare(op, &l, &r)?.into()),
            BinOp::Min | BinOp::Max => {
                let cond = if op == BinOp::Min {
                    compare(BinOp::Le, &l, &r)?
                } else {
                    compare(BinOp::Le, &r, &l)?
                };
                same(cond.ite(l.ast(), r.ast()))
            }
            BinOp::Eq => {
                let eq = l
                    .ast()
                    .safe_eq(r.ast())
                    .map_err(|_| SmtError::TypeMismatch {
                        expected: l.bt.clone(),
                        actual: r.bt.clone(),
                    })?;
                prop(eq.into())
            }
            BinOp::LtPointer | BinOp::LePointer => match &bt {
                BaseType::Loc(_) => {
                    let (x, y) = (self.as_address(&l)?, self.as_address(&r)?);
                    prop(
                        if op == BinOp::LtPointer {
                            x.bvult(&y)
                        } else {
                            x.bvule(&y)
                        }
                        .into(),
                    )
                }
                _ => Err(bad()),
            },

            BinOp::SetUnion | BinOp::SetIntersection | BinOp::SetDifference => {
                let (x, y) = (l.expect_set()?, r.expect_set()?);
                same(match op {
                    BinOp::SetUnion => Set::set_union(&[&x, &y]).into(),
                    BinOp::SetIntersection => Set::intersect(&[&x, &y]).into(),
                    _ => x.difference(&y).into(),
                })
            }
            BinOp::Subset => prop(l.expect_set()?.set_subset(&r.expect_set()?).into()),
            BinOp::SetMember => Err(bad()),
        }
    }

    fn exp(
        &mut self,
        base_term: &IndexTerm,
        exponent_term: &IndexTerm,
        base: &SmtExpr,
    ) -> Result<SmtExpr, SmtError> {
        let n = literal_integer(exponent_term).ok_or(SmtError::UnsupportedSymbolicExponent {
            found: term_name(&exponent_term.term),
        })?;
        if n < BigInt::ZERO {
            return Err(SmtError::UnsupportedSymbolicExponent {
                found: "a negative literal",
            });
        }
        let bt = base.bt.clone();
        match &bt {
            BaseType::Bits(_, width) => {
                if let Some(b) = literal_integer(base_term) {
                    let modulus = BigInt::from(1) << *width;
                    let value = b.modpow(&n, &modulus);
                    return self.numeric_literal(value, &bt);
                }
                self.expand_exp(base, &n, &bt)
            }
            BaseType::Integer => {
                let big = u128::try_from(&n).unwrap_or(u128::MAX);
                if big > EXP_EXPANSION_LIMIT {
                    return Err(SmtError::ExpansionLimit {
                        what: "Exp",
                        count: big,
                        limit: EXP_EXPANSION_LIMIT,
                    });
                }
                if let Some(b) = literal_integer(base_term) {
                    let value = b.pow(u32::try_from(big).unwrap_or(u32::MAX));
                    return self.numeric_literal(value, &bt);
                }
                self.expand_exp(base, &n, &bt)
            }
            other => Err(SmtError::UnsupportedOperator {
                op: "Exp",
                bt: other.clone(),
            }),
        }
    }

    fn expand_exp(
        &mut self,
        base: &SmtExpr,
        n: &BigInt,
        bt: &BaseType,
    ) -> Result<SmtExpr, SmtError> {
        let count = u128::try_from(n).unwrap_or(u128::MAX);
        if count > EXP_EXPANSION_LIMIT {
            return Err(SmtError::ExpansionLimit {
                what: "Exp",
                count,
                limit: EXP_EXPANSION_LIMIT,
            });
        }
        let mut acc = self.numeric_literal(BigInt::from(1), bt)?;
        for _ in 0..count {
            acc = match bt {
                BaseType::Bits(_, _) => SmtExpr::new(
                    acc.expect_bv()?.bvmul(&base.expect_bv()?).into(),
                    bt.clone(),
                ),
                _ => SmtExpr::new(
                    Int::mul(&[&acc.expect_int()?, &base.expect_int()?]).into(),
                    bt.clone(),
                ),
            };
        }
        Ok(acc)
    }

    fn apply(
        &mut self,
        env: &Env<'_>,
        name: &Sym,
        args: &[IndexTerm],
    ) -> Result<SmtExpr, SmtError> {
        let def = self.sorts.program().function(name)?;
        if def.args.len() != args.len() {
            return Err(SmtError::FunctionArity {
                name: name.clone(),
                expected: def.args.len(),
                actual: args.len(),
            });
        }
        let Some(body) = def.body.as_ref() else {
            return Err(SmtError::UninterpretedFunction { name: name.clone() });
        };
        let mut actuals = args
            .iter()
            .map(|a| self.term(env, a))
            .collect::<Result<Vec<_>, _>>()?;
        for ((_, declared), actual) in def.args.iter().zip(&actuals) {
            expect_type(declared, actual.bt())?;
        }
        let (FunctionBody::Def(body) | FunctionBody::RecDef(body)) = body;
        let defaults = self
            .function_defaults
            .entry(name.clone())
            .or_insert_with(|| super::input_default_types(body, self.sorts.program().functions()))
            .clone();
        for bt in &defaults {
            actuals.push(self.input_default(bt)?);
        }
        let decl = self.declare_function(name, def, body, &defaults)?;
        let refs: Vec<&dyn Ast> = actuals.iter().map(|a| a.ast() as &dyn Ast).collect();
        Ok(SmtExpr::new(decl.apply(&refs), def.return_bt.clone()))
    }

    fn declare_function(
        &mut self,
        name: &Sym,
        def: &LogicalFunctionDef,
        body: &IndexTerm,
        defaults: &std::collections::BTreeSet<BaseType>,
    ) -> Result<Rc<RecFuncDecl>, SmtError> {
        if let Some(d) = self.declared.get(name) {
            return Ok(Rc::clone(d));
        }
        let mut domain = def
            .args
            .iter()
            .map(|(_, bt)| self.sort(bt))
            .collect::<Result<Vec<_>, _>>()?;
        for bt in defaults {
            domain.push(self.sort(bt)?);
        }
        let range = self.sort(&def.return_bt)?;
        let decl = Rc::new(RecFuncDecl::new(
            names::fn_name(name).as_str(),
            &domain.iter().collect::<Vec<_>>(),
            &range,
        ));
        self.declared.insert(name.clone(), Rc::clone(&decl));

        match self.define_function(def, body, &domain, &decl, defaults) {
            Ok(()) => Ok(decl),
            Err(e) => {
                self.declared.remove(name);
                Err(e)
            }
        }
    }

    fn define_function(
        &mut self,
        def: &LogicalFunctionDef,
        body: &IndexTerm,
        domain: &[Sort],
        decl: &RecFuncDecl,
        defaults: &std::collections::BTreeSet<BaseType>,
    ) -> Result<(), SmtError> {
        let mut params: Vec<Dynamic> = def
            .args
            .iter()
            .zip(domain)
            .map(|((p, _), sort)| {
                let n = self.take_binder();
                Dynamic::new_const(names::binder_name(p, n), sort)
            })
            .collect();
        let mut default_parameters = BTreeMap::new();
        for (bt, sort) in defaults.iter().zip(&domain[def.args.len()..]) {
            let n = self.take_binder();
            let value = Dynamic::new_const(format!("austen-default-param-{n}"), sort);
            default_parameters.insert(bt.clone(), SmtExpr::new(value.clone(), bt.clone()));
            params.push(value);
        }
        let frame: SymMap<SmtExpr> = def
            .args
            .iter()
            .zip(&params)
            .map(|((p, bt), c)| (p.clone(), SmtExpr::new(c.clone(), bt.clone())))
            .collect();
        let outer = std::mem::replace(&mut self.default_parameters, default_parameters);
        let translated = self.term(&Env::Root(&frame), body);
        self.default_parameters = outer;
        let translated = translated?;
        expect_type(&def.return_bt, &translated.bt)?;
        let refs: Vec<&dyn Ast> = params.iter().map(|p| p as &dyn Ast).collect();
        decl.add_def(&refs, translated.ast());
        Ok(())
    }

    fn ite(
        &mut self,
        env: &Env<'_>,
        cond: &IndexTerm,
        then_: &IndexTerm,
        else_: &IndexTerm,
    ) -> Result<SmtExpr, SmtError> {
        let c = self.boolean(env, cond)?;
        let (a, b) = (self.term(env, then_)?, self.term(env, else_)?);
        check_type(&a.bt, &b.bt)?;
        let bt = a.bt.clone();
        Ok(SmtExpr::new(c.ite(a.ast(), b.ast()), bt))
    }

    fn each_i(
        &mut self,
        env: &Env<'_>,
        start: i64,
        var: &(Sym, BaseType),
        end: i64,
        body: &IndexTerm,
    ) -> Result<SmtExpr, SmtError> {
        if start > end {
            return Ok(SmtExpr::new(Bool::from_bool(true).into(), BaseType::Bool));
        }
        let count = (i128::from(end) - i128::from(start)) as u128 + 1;
        if count > MAX_EACHI_EXPANSION {
            return Err(SmtError::ExpansionLimit {
                what: "EachI",
                count,
                limit: MAX_EACHI_EXPANSION,
            });
        }
        let (sym, bt) = var;
        let mut conjuncts = Vec::with_capacity(count as usize);
        for i in start..=end {
            let index = self.numeric_literal(BigInt::from(i), bt)?;
            let scoped = env.push(sym, index);
            conjuncts.push(self.boolean(&scoped, body)?);
        }
        Ok(SmtExpr::new(Bool::and(&conjuncts).into(), BaseType::Bool))
    }

    fn build(
        &mut self,
        bt: BaseType,
        variant_index: usize,
        parts: &[SmtExpr],
    ) -> Result<SmtExpr, SmtError> {
        let decl = self.sorts.datatype(&bt)?;
        let variant = &decl.variants[variant_index];
        if variant.accessors.len() != parts.len() {
            return Err(SmtError::TypeMismatch {
                expected: bt.clone(),
                actual: bt.clone(),
            });
        }
        let args: Vec<&dyn Ast> = parts.iter().map(|p| p.ast() as &dyn Ast).collect();
        Ok(SmtExpr::new(variant.constructor.apply(&args), bt))
    }

    fn project(
        &mut self,
        value: &SmtExpr,
        variant_index: usize,
        index: usize,
        result: BaseType,
    ) -> Result<SmtExpr, SmtError> {
        let decl = self.sorts.datatype(&value.bt)?;
        let ast = decl.variants[variant_index].accessors[index].apply(&[value.ast()]);
        Ok(SmtExpr::new(ast, result))
    }

    fn tester(&mut self, value: &SmtExpr, variant_index: usize) -> Result<Bool, SmtError> {
        let decl = self.sorts.datatype(&value.bt)?;
        Ok(decl.variants[variant_index]
            .tester
            .apply(&[value.ast()])
            .as_bool()
            .expect("a datatype recognizer is Bool-sorted"))
    }

    fn constructor(
        &mut self,
        env: &Env<'_>,
        bt: &BaseType,
        name: &Sym,
        members: &[(Id, IndexTerm)],
    ) -> Result<SmtExpr, SmtError> {
        let tag = datatype_tag(bt)?;
        let (variant_index, declaration) = self.constructor_index(tag, name)?;
        if declaration.params.len() != members.len() {
            return Err(SmtError::ConstructorArity {
                tag: tag.clone(),
                constructor: name.clone(),
                expected: declaration.params.len(),
                actual: members.len(),
            });
        }
        for (member, _) in members {
            if !declaration
                .params
                .iter()
                .any(|(declared, _)| declared == member)
            {
                return Err(SmtError::UnknownConstructorMember {
                    tag: tag.clone(),
                    constructor: name.clone(),
                    member: member.clone(),
                });
            }
        }

        let mut parts = Vec::with_capacity(declaration.params.len());
        for (member, declared_bt) in &declaration.params {
            let term = members
                .iter()
                .find(|(candidate, _)| candidate == member)
                .map(|(_, term)| term)
                .ok_or_else(|| SmtError::MissingConstructorMember {
                    tag: tag.clone(),
                    constructor: name.clone(),
                    member: member.clone(),
                })?;
            let value = self.term(env, term)?;
            check_type(declared_bt, value.bt())?;
            parts.push(value);
        }
        self.build(bt.clone(), variant_index, &parts)
    }

    fn match_(
        &mut self,
        env: &Env<'_>,
        scrutinee: &IndexTerm,
        arms: &[(Pattern, IndexTerm)],
        expected: &BaseType,
    ) -> Result<SmtExpr, SmtError> {
        let value = self.term(env, scrutinee)?;
        self.sorts
            .program()
            .datatype_def(datatype_tag(&value.bt)?)?;

        let mut translated = Vec::with_capacity(arms.len());
        let mut fallback = None;
        for (pattern, body) in arms {
            let mut bindings = Vec::new();
            let condition = self.pattern_condition(pattern, &value, &mut bindings)?;
            let body = self.term_with_bindings(env, &bindings, body)?;
            check_type(expected, body.bt())?;
            match condition {
                Some(condition) => translated.push((condition, body)),
                None => {
                    fallback = Some(body);
                    break;
                }
            }
        }

        let mut result = match fallback {
            Some(fallback) => fallback,
            None => self.default_of(expected)?,
        };
        while let Some((condition, body)) = translated.pop() {
            let bt = body.bt.clone();
            result = SmtExpr::new(condition.ite(body.ast(), result.ast()), bt);
        }
        Ok(result)
    }

    fn pattern_condition(
        &mut self,
        pattern: &Pattern,
        value: &SmtExpr,
        bindings: &mut Vec<(Sym, SmtExpr)>,
    ) -> Result<Option<Bool>, SmtError> {
        self.enter()?;
        let out = self.pattern_condition_at_depth(pattern, value, bindings);
        self.depth -= 1;
        out
    }

    fn pattern_condition_at_depth(
        &mut self,
        pattern: &Pattern,
        value: &SmtExpr,
        bindings: &mut Vec<(Sym, SmtExpr)>,
    ) -> Result<Option<Bool>, SmtError> {
        match &pattern.pattern {
            PatternKind::Sym(sym) => {
                bindings.push((sym.clone(), value.clone()));
                Ok(None)
            }
            PatternKind::Wild => Ok(None),
            PatternKind::Constructor(name, members) => {
                let tag = datatype_tag(&value.bt)?;
                let (variant_index, declaration) = self.constructor_index(tag, name)?;
                let mut conditions = vec![self.tester(value, variant_index)?];
                for (member, subpattern) in members {
                    let (field_index, (_, field_bt)) = declaration
                        .params
                        .iter()
                        .enumerate()
                        .find(|(_, (candidate, _))| candidate == member)
                        .ok_or_else(|| SmtError::UnknownConstructorMember {
                            tag: tag.clone(),
                            constructor: name.clone(),
                            member: member.clone(),
                        })?;
                    let field =
                        self.project(value, variant_index, field_index, field_bt.clone())?;
                    if let Some(condition) = self.pattern_condition(subpattern, &field, bindings)? {
                        conditions.push(condition);
                    }
                }
                Ok(Some(Bool::and(&conditions)))
            }
        }
    }

    fn term_with_bindings(
        &mut self,
        env: &Env<'_>,
        bindings: &[(Sym, SmtExpr)],
        body: &IndexTerm,
    ) -> Result<SmtExpr, SmtError> {
        let Some(((sym, value), rest)) = bindings.split_first() else {
            return self.term(env, body);
        };
        let scoped = env.push(sym, value.clone());
        self.term_with_bindings(&scoped, rest, body)
    }

    fn replace_field(
        &mut self,
        base: &SmtExpr,
        replaced: usize,
        new: &SmtExpr,
    ) -> Result<SmtExpr, SmtError> {
        let decl = self.sorts.datatype(&base.bt)?;
        let variant = &decl.variants[variant::FIRST];
        let parts: Vec<Dynamic> = (0..variant.accessors.len())
            .map(|i| {
                if i == replaced {
                    new.ast().clone()
                } else {
                    variant.accessors[i].apply(&[base.ast()])
                }
            })
            .collect();
        let args: Vec<&dyn Ast> = parts.iter().map(|p| p as &dyn Ast).collect();
        let ast = variant.constructor.apply(&args);
        Ok(SmtExpr::new(ast, base.bt.clone()))
    }

    fn tuple(&mut self, env: &Env<'_>, es: &[IndexTerm]) -> Result<SmtExpr, SmtError> {
        let parts = es
            .iter()
            .map(|e| self.term(env, e))
            .collect::<Result<Vec<_>, _>>()?;
        let bt = BaseType::Tuple(parts.iter().map(|p| p.bt.clone()).collect());
        self.build(bt, variant::FIRST, &parts)
    }

    fn nth_tuple(
        &mut self,
        env: &Env<'_>,
        index: usize,
        e: &IndexTerm,
    ) -> Result<SmtExpr, SmtError> {
        let v = self.term(env, e)?;
        let BaseType::Tuple(components) = &v.bt else {
            return Err(SmtError::TypeMismatch {
                expected: BaseType::Tuple(vec![]),
                actual: v.bt.clone(),
            });
        };
        let component = components
            .get(index)
            .ok_or(SmtError::TupleIndexOutOfRange {
                index,
                arity: components.len(),
            })?
            .clone();
        self.project(&v, variant::FIRST, index, component)
    }

    fn struct_(
        &mut self,
        env: &Env<'_>,
        tag: &Sym,
        fields: &[(Id, IndexTerm)],
    ) -> Result<SmtExpr, SmtError> {
        let program = self.sorts.program();
        let members = &program.struct_def(tag)?.members;
        for (id, _) in fields {
            if !members.iter().any(|(m, _)| m == id) {
                return Err(SmtError::UnknownStructMember {
                    tag: tag.clone(),
                    member: id.clone(),
                });
            }
        }
        let mut parts = Vec::with_capacity(members.len());
        for (id, ct) in members {
            let term = fields
                .iter()
                .find(|(f, _)| f == id)
                .map(|(_, t)| t)
                .ok_or_else(|| SmtError::UnknownStructMember {
                    tag: tag.clone(),
                    member: id.clone(),
                })?;
            let value = self.term(env, term)?;
            let declared = ct
                .to_base_type()
                .ok_or_else(|| SmtError::UnsizedCtype(ct.clone()))?;
            check_type(&declared, &value.bt)?;
            parts.push(value);
        }
        self.build(BaseType::Struct(tag.clone()), variant::FIRST, &parts)
    }

    fn struct_member(
        &mut self,
        env: &Env<'_>,
        e: &IndexTerm,
        member: &Id,
    ) -> Result<SmtExpr, SmtError> {
        let v = self.term(env, e)?;
        let BaseType::Struct(tag) = v.bt.clone() else {
            return Err(SmtError::TypeMismatch {
                expected: BaseType::Struct(Sym { name: None, num: 0 }),
                actual: v.bt.clone(),
            });
        };
        let (index, member_bt) = self.struct_member_index(&tag, member)?;
        self.project(&v, variant::FIRST, index, member_bt)
    }

    fn constructor_index(
        &self,
        tag: &Sym,
        name: &Sym,
    ) -> Result<(usize, &'p ConstructorDef), SmtError> {
        self.sorts
            .program()
            .datatype_def(tag)?
            .constructors
            .iter()
            .enumerate()
            .find(|(_, constructor)| &constructor.name == name)
            .ok_or_else(|| SmtError::UnknownConstructor {
                tag: tag.clone(),
                constructor: name.clone(),
            })
    }

    fn struct_member_index(&self, tag: &Sym, member: &Id) -> Result<(usize, BaseType), SmtError> {
        let def = self.sorts.program().struct_def(tag)?;
        let (index, (_, ct)) = def
            .members
            .iter()
            .enumerate()
            .find(|(_, (m, _))| m == member)
            .ok_or_else(|| SmtError::UnknownStructMember {
                tag: tag.clone(),
                member: member.clone(),
            })?;
        let bt = ct
            .to_base_type()
            .ok_or_else(|| SmtError::UnsizedCtype(ct.clone()))?;
        Ok((index, bt))
    }

    fn struct_update(
        &mut self,
        env: &Env<'_>,
        target: &IndexTerm,
        member: &Id,
        value: &IndexTerm,
    ) -> Result<SmtExpr, SmtError> {
        let base = self.term(env, target)?;
        let BaseType::Struct(tag) = base.bt.clone() else {
            return Err(SmtError::TypeMismatch {
                expected: BaseType::Struct(Sym { name: None, num: 0 }),
                actual: base.bt.clone(),
            });
        };
        let (replaced, member_bt) = self.struct_member_index(&tag, member)?;
        let new = self.term(env, value)?;
        check_type(&member_bt, &new.bt)?;
        self.replace_field(&base, replaced, &new)
    }

    fn record(&mut self, env: &Env<'_>, fields: &[(Id, IndexTerm)]) -> Result<SmtExpr, SmtError> {
        let mut parts = Vec::with_capacity(fields.len());
        let mut members = Vec::with_capacity(fields.len());
        for (id, t) in fields {
            let v = self.term(env, t)?;
            members.push((id.clone(), v.bt.clone()));
            parts.push(v);
        }
        self.build(BaseType::Record(members), variant::FIRST, &parts)
    }

    fn record_member(
        &mut self,
        env: &Env<'_>,
        e: &IndexTerm,
        member: &Id,
    ) -> Result<SmtExpr, SmtError> {
        let v = self.term(env, e)?;
        let (index, member_bt) = record_member_index(&v.bt, member)?;
        self.project(&v, variant::FIRST, index, member_bt)
    }

    fn record_update(
        &mut self,
        env: &Env<'_>,
        target: &IndexTerm,
        member: &Id,
        value: &IndexTerm,
    ) -> Result<SmtExpr, SmtError> {
        let base = self.term(env, target)?;
        let (replaced, member_bt) = record_member_index(&base.bt, member)?;
        let new = self.term(env, value)?;
        check_type(&member_bt, &new.bt)?;
        self.replace_field(&base, replaced, &new)
    }

    fn nullary_variant(&mut self, bt: BaseType) -> Result<SmtExpr, SmtError> {
        self.build(bt, variant::FIRST, &[])
    }

    fn cons(
        &mut self,
        env: &Env<'_>,
        head: &IndexTerm,
        tail: &IndexTerm,
    ) -> Result<SmtExpr, SmtError> {
        let (h, t) = (self.term(env, head)?, self.term(env, tail)?);
        check_type(&BaseType::List(Box::new(h.bt.clone())), &t.bt)?;
        let bt = t.bt.clone();
        self.build(bt, variant::SECOND, &[h, t])
    }

    fn list_field(
        &mut self,
        env: &Env<'_>,
        e: &IndexTerm,
        head: bool,
    ) -> Result<SmtExpr, SmtError> {
        let v = self.term(env, e)?;
        let BaseType::List(elem) = v.bt.clone() else {
            return Err(SmtError::TypeMismatch {
                expected: BaseType::List(Box::new(BaseType::Unit)),
                actual: v.bt.clone(),
            });
        };
        let (index, result) = if head { (0, *elem) } else { (1, v.bt.clone()) };
        self.project(&v, variant::SECOND, index, result)
    }

    fn cn_some(&mut self, env: &Env<'_>, e: &IndexTerm) -> Result<SmtExpr, SmtError> {
        let v = self.term(env, e)?;
        let bt = BaseType::Option(Box::new(v.bt.clone()));
        self.build(bt, variant::SECOND, &[v])
    }

    fn is_some(&mut self, env: &Env<'_>, e: &IndexTerm) -> Result<SmtExpr, SmtError> {
        let v = self.term(env, e)?;
        if !matches!(v.bt, BaseType::Option(_)) {
            return Err(SmtError::TypeMismatch {
                expected: BaseType::Option(Box::new(BaseType::Unit)),
                actual: v.bt,
            });
        }
        let ast = self.tester(&v, variant::SECOND)?;
        Ok(SmtExpr::new(ast.into(), BaseType::Bool))
    }

    fn get_opt(&mut self, env: &Env<'_>, e: &IndexTerm) -> Result<SmtExpr, SmtError> {
        let v = self.term(env, e)?;
        let BaseType::Option(elem) = v.bt.clone() else {
            return Err(SmtError::TypeMismatch {
                expected: BaseType::Option(Box::new(BaseType::Unit)),
                actual: v.bt,
            });
        };
        self.project(&v, variant::SECOND, 0, *elem)
    }

    fn map_const(
        &mut self,
        env: &Env<'_>,
        key_bt: &BaseType,
        e: &IndexTerm,
    ) -> Result<SmtExpr, SmtError> {
        let v = self.term(env, e)?;
        let key_sort = self.sort(key_bt)?;
        let ast = Array::const_array(&key_sort, v.ast());
        Ok(SmtExpr::new(
            ast.into(),
            BaseType::make_map(key_bt.clone(), v.bt),
        ))
    }

    fn map_set(
        &mut self,
        env: &Env<'_>,
        map: &IndexTerm,
        k: &IndexTerm,
        v: &IndexTerm,
    ) -> Result<SmtExpr, SmtError> {
        let m = self.term(env, map)?;
        let (key, value) = map_types(&m.bt)?;
        let (k, v) = (self.term(env, k)?, self.term(env, v)?);
        check_type(&key, &k.bt)?;
        check_type(&value, &v.bt)?;
        let ast = m.expect_array()?.store(k.ast(), v.ast());
        Ok(SmtExpr::new(ast.into(), m.bt))
    }

    fn map_get(
        &mut self,
        env: &Env<'_>,
        map: &IndexTerm,
        k: &IndexTerm,
    ) -> Result<SmtExpr, SmtError> {
        let m = self.term(env, map)?;
        let (key, value) = map_types(&m.bt)?;
        let k = self.term(env, k)?;
        check_type(&key, &k.bt)?;
        Ok(SmtExpr::new(m.expect_array()?.select(k.ast()), value))
    }

    fn layout(&mut self, tag: &Sym) -> Result<&StructLayout, SmtError> {
        if !self.layouts.contains_key(tag) {
            let program = self.sorts.program();
            let computed = program
                .struct_def(tag)?
                .layout_in(program.structs())
                .ok_or_else(|| SmtError::UnsizedCtype(Sctype::Struct(tag.clone())))?;
            self.layouts.insert(tag.clone(), computed);
        }
        Ok(&self.layouts[tag])
    }

    fn size_of(&self, ct: &Sctype) -> Result<u64, SmtError> {
        ct.byte_size_in(self.sorts.program().structs())
            .ok_or_else(|| SmtError::UnsizedCtype(ct.clone()))
    }

    fn offset_of(&mut self, tag: &Sym, member: &Id) -> Result<u64, SmtError> {
        let at = self
            .sorts
            .program()
            .struct_def(tag)?
            .members
            .iter()
            .position(|(m, _)| m == member);
        let layout = self.layout(tag)?;
        at.and_then(|at| layout.offsets.get(at).copied())
            .ok_or_else(|| SmtError::UnknownStructMember {
                tag: tag.clone(),
                member: member.clone(),
            })
    }

    fn member_shift(
        &mut self,
        env: &Env<'_>,
        base: &IndexTerm,
        tag: &Sym,
        member: &Id,
    ) -> Result<SmtExpr, SmtError> {
        let offset = self.offset_of(tag, member)?;
        let p = self.term(env, base)?;
        let offset = BV::from_u64(offset, 64);
        self.shift(&p, &offset)
    }

    fn array_shift(
        &mut self,
        env: &Env<'_>,
        base: &IndexTerm,
        ct: &Sctype,
        index: &IndexTerm,
    ) -> Result<SmtExpr, SmtError> {
        let size = self.size_of(ct)?;
        let p = self.term(env, base)?;
        let i = self.term(env, index)?;
        let i = match &i.bt {
            BaseType::Bits(sign, width) => bv_cast(*sign, *width, 64, &i.expect_bv()?),
            other => {
                return Err(SmtError::TypeMismatch {
                    expected: uintptr_bt(),
                    actual: other.clone(),
                });
            }
        };
        let offset = BV::from_u64(size, 64).bvmul(&i);
        self.shift(&p, &offset)
    }

    fn shift(&mut self, p: &SmtExpr, offset: &BV) -> Result<SmtExpr, SmtError> {
        if !matches!(p.bt, BaseType::Loc(_)) {
            return Err(SmtError::TypeMismatch {
                expected: BaseType::Loc(None),
                actual: p.bt.clone(),
            });
        }
        let ast = match self.pointers() {
            PointerEncoding::Provenance => {
                let addr = self.addr_field(p)?.expect_bv()?.bvadd(offset);
                let alloc_id = self.alloc_id_field(p)?;
                self.aia(&alloc_id, &addr)?
            }
            PointerEncoding::Address => p.expect_bv()?.bvadd(offset).into(),
        };
        Ok(SmtExpr::new(ast, BaseType::Loc(None)))
    }

    fn aia(&mut self, alloc_id: &SmtExpr, addr: &BV) -> Result<Dynamic, SmtError> {
        let addr = SmtExpr::new(addr.clone().into(), uintptr_bt());
        Ok(self
            .build(
                BaseType::Loc(None),
                variant::SECOND,
                &[alloc_id.clone(), addr],
            )?
            .into_ast())
    }

    fn is_aia(&mut self, p: &SmtExpr) -> Result<Bool, SmtError> {
        self.tester(p, variant::SECOND)
    }

    fn alloc_id_field(&mut self, p: &SmtExpr) -> Result<SmtExpr, SmtError> {
        self.project(p, variant::SECOND, 0, BaseType::AllocId)
    }

    fn addr_field(&mut self, p: &SmtExpr) -> Result<SmtExpr, SmtError> {
        self.project(p, variant::SECOND, 1, uintptr_bt())
    }

    fn match_pointer<T: Ast>(
        &mut self,
        p: &SmtExpr,
        aia_case: &T,
        null_case: &T,
    ) -> Result<T, SmtError> {
        Ok(self.is_aia(p)?.ite(aia_case, null_case))
    }

    pub fn as_address(&mut self, e: &SmtExpr) -> Result<BV, SmtError> {
        match (&e.bt, self.pointers()) {
            (BaseType::Loc(_), PointerEncoding::Provenance) => {
                let addr = self.addr_field(e)?.expect_bv()?;
                let zero = BV::from_u64(0, 64);
                self.match_pointer(e, &addr, &zero)
            }
            (BaseType::Loc(_), PointerEncoding::Address) => e.expect_bv(),
            (other, _) => Err(SmtError::TypeMismatch {
                expected: BaseType::Loc(None),
                actual: other.clone(),
            }),
        }
    }

    fn has_alloc_id(&mut self, env: &Env<'_>, e: &IndexTerm) -> Result<SmtExpr, SmtError> {
        if !self.pointers().is_provenance() {
            return Err(SmtError::NeedsProvenance { what: "HasAllocId" });
        }
        let p = self.term(env, e)?;
        if !matches!(p.bt, BaseType::Loc(_)) {
            return Err(SmtError::TypeMismatch {
                expected: BaseType::Loc(None),
                actual: p.bt,
            });
        }
        Ok(SmtExpr::new(self.is_aia(&p)?.into(), BaseType::Bool))
    }

    fn copy_alloc_id(
        &mut self,
        env: &Env<'_>,
        addr: &IndexTerm,
        loc: &IndexTerm,
    ) -> Result<SmtExpr, SmtError> {
        if !self.pointers().is_provenance() {
            return Err(SmtError::NeedsProvenance {
                what: "CopyAllocId",
            });
        }
        let p = self.term(env, loc)?;
        let new_addr = self.term(env, addr)?;
        if !matches!(p.bt, BaseType::Loc(_)) {
            return Err(SmtError::TypeMismatch {
                expected: BaseType::Loc(None),
                actual: p.bt,
            });
        }
        check_type(&uintptr_bt(), &new_addr.bt)?;
        let new_addr = new_addr.expect_bv()?;
        let alloc_id = self.alloc_id_field(&p)?;
        Ok(SmtExpr::new(
            self.aia(&alloc_id, &new_addr)?,
            BaseType::Loc(None),
        ))
    }

    fn cast(
        &mut self,
        env: &Env<'_>,
        target: &BaseType,
        e: &IndexTerm,
    ) -> Result<SmtExpr, SmtError> {
        let v = self.term(env, e)?;
        self.apply_cast(target, v)
    }

    fn apply_cast(&mut self, target: &BaseType, v: SmtExpr) -> Result<SmtExpr, SmtError> {
        self.sort(target)?;
        let from = v.bt.clone();
        let cast_err = || SmtError::UnsupportedCast {
            from: from.clone(),
            to: target.clone(),
        };
        match (&from, target) {
            (a, b) if Sorts::same_type(a, b) => Ok(SmtExpr::new(v.into_ast(), target.clone())),
            (BaseType::Bits(sign, w1), BaseType::Bits(_, w2)) => Ok(SmtExpr::new(
                bv_cast(*sign, *w1, *w2, &v.expect_bv()?).into(),
                target.clone(),
            )),
            (BaseType::Bits(sign, _), BaseType::Integer) => Ok(SmtExpr::new(
                v.expect_bv()?.to_int(*sign == Sign::Signed).into(),
                BaseType::Integer,
            )),
            (BaseType::Integer, BaseType::Bits(_, width)) => Ok(SmtExpr::new(
                BV::from_int(&v.expect_int()?, u32::from(*width)).into(),
                target.clone(),
            )),
            (BaseType::Bits(sign, w), BaseType::Loc(_)) => {
                let bits = bv_cast(*sign, *w, 64, &v.expect_bv()?);
                match self.pointers() {
                    PointerEncoding::Provenance => {
                        let alloc_id = self.default_of(&BaseType::AllocId)?;
                        let aia = self.aia(&alloc_id, &bits)?;
                        let null = self.null()?;
                        let zero = BV::from_u64(0, 64);
                        Ok(SmtExpr::new(
                            bits.eq(&zero).ite(null.ast(), &aia),
                            target.clone(),
                        ))
                    }
                    PointerEncoding::Address => Ok(SmtExpr::new(bits.into(), target.clone())),
                }
            }
            (BaseType::Loc(_), BaseType::Bits(_, w)) => {
                let addr = self.as_address(&v)?;
                Ok(SmtExpr::new(
                    bv_cast(Sign::Unsigned, 64, *w, &addr).into(),
                    target.clone(),
                ))
            }
            (BaseType::Loc(_), BaseType::AllocId) => self.alloc_id_field(&v),
            (BaseType::Integer, BaseType::Real) => Ok(SmtExpr::new(
                v.expect_int()?.to_real().into(),
                BaseType::Real,
            )),
            (BaseType::Real, BaseType::Integer) => Ok(SmtExpr::new(
                v.expect_real()?.to_int().into(),
                BaseType::Integer,
            )),
            _ => Err(cast_err()),
        }
    }

    fn value_check(
        &mut self,
        good: bool,
        ct: &Sctype,
        about: &IndexTerm,
    ) -> Result<IndexTerm, SmtError> {
        self.enter()?;
        let out = crate::gen_dsl::value_check(good, ct, about, self.sorts.program().structs());
        self.depth -= 1;
        Ok(out?)
    }

    fn take_binder(&mut self) -> u64 {
        let n = self.binders;
        self.binders += 1;
        n
    }

    pub fn decode(&mut self, model: &z3::Model, expr: &SmtExpr) -> Result<ModelValue, SmtError> {
        if !matches!(expr.bt(), BaseType::Loc(_)) || !self.pointers().is_provenance() {
            return super::model::decode(model, expr);
        }
        let bad = |why: &str| SmtError::ModelDecode {
            bt: BaseType::Loc(None),
            why: why.to_owned(),
        };

        let tester = SmtExpr::new(self.is_aia(expr)?.into(), BaseType::Bool);
        match super::model::decode(model, &tester)? {
            ModelValue::Bool(false) => return Ok(ModelValue::Null),
            ModelValue::Bool(true) => {}
            _ => return Err(bad("the model did not say which constructor")),
        }

        let addr = self.addr_field(expr)?;
        let addr = match super::model::decode(model, &addr)? {
            ModelValue::Bits { unsigned, .. } => {
                u64::try_from(&unsigned).expect("a 64-bit read fits u64")
            }
            _ => return Err(bad("the address did not decode as a bitvector")),
        };
        let id = self.alloc_id_field(expr)?;
        let alloc_id = match super::model::decode(model, &id)? {
            ModelValue::AllocId(n) => Some(n),
            _ => return Err(bad("the allocation id did not decode as an integer")),
        };
        Ok(ModelValue::Pointer { alloc_id, addr })
    }
}

fn compare(op: BinOp, l: &SmtExpr, r: &SmtExpr) -> Result<Bool, SmtError> {
    let strict = op == BinOp::Lt;
    match &l.bt {
        BaseType::Bits(Sign::Signed, _) => {
            let (x, y) = (l.expect_bv()?, r.expect_bv()?);
            Ok(if strict { x.bvslt(&y) } else { x.bvsle(&y) })
        }
        BaseType::Bits(Sign::Unsigned, _) => {
            let (x, y) = (l.expect_bv()?, r.expect_bv()?);
            Ok(if strict { x.bvult(&y) } else { x.bvule(&y) })
        }
        BaseType::Integer => {
            let (x, y) = (l.expect_int()?, r.expect_int()?);
            Ok(if strict { x.lt(&y) } else { x.le(&y) })
        }
        BaseType::Real => {
            let (x, y) = (l.expect_real()?, r.expect_real()?);
            Ok(if strict { x.lt(&y) } else { x.le(&y) })
        }
        other => Err(SmtError::UnsupportedOperator {
            op: binop_name(op),
            bt: other.clone(),
        }),
    }
}

fn expect_type(expected: &BaseType, actual: &BaseType) -> Result<(), SmtError> {
    if Sorts::same_type(expected, actual) {
        Ok(())
    } else {
        Err(SmtError::TypeMismatch {
            expected: expected.clone(),
            actual: actual.clone(),
        })
    }
}

fn bv_cast(from_sign: Sign, from_width: u16, to_width: u16, x: &BV) -> BV {
    if to_width == from_width {
        x.clone()
    } else if to_width < from_width {
        x.extract(u32::from(to_width) - 1, 0)
    } else if from_sign == Sign::Signed {
        x.sign_ext(u32::from(to_width - from_width))
    } else {
        x.zero_ext(u32::from(to_width - from_width))
    }
}

fn count_leading_zeros(result_width: u16, width: u16, e: &BV) -> BV {
    let result = |k: u64| BV::from_u64(k, u32::from(result_width));
    if width == 1 {
        let zero = BV::from_u64(0, 1);
        return e.eq(&zero).ite(&result(1), &result(0));
    }
    let top_w = width / 2;
    let bot_w = width - top_w;
    let top = e.extract(u32::from(width) - 1, u32::from(width - top_w));
    let bot = e.extract(u32::from(bot_w) - 1, 0);
    let top_zero = top.eq(BV::from_u64(0, u32::from(top_w)));
    let then_ = count_leading_zeros(result_width, bot_w, &bot).bvadd(result(u64::from(top_w)));
    let else_ = count_leading_zeros(result_width, top_w, &top);
    top_zero.ite(&then_, &else_)
}

fn count_trailing_zeros(result_width: u16, width: u16, e: &BV) -> BV {
    let result = |k: u64| BV::from_u64(k, u32::from(result_width));
    if width == 1 {
        let zero = BV::from_u64(0, 1);
        return e.eq(&zero).ite(&result(1), &result(0));
    }
    let top_w = width / 2;
    let bot_w = width - top_w;
    let top = e.extract(u32::from(width) - 1, u32::from(width - top_w));
    let bot = e.extract(u32::from(bot_w) - 1, 0);
    let bot_zero = bot.eq(BV::from_u64(0, u32::from(bot_w)));
    let then_ = count_trailing_zeros(result_width, top_w, &top).bvadd(result(u64::from(bot_w)));
    let else_ = count_trailing_zeros(result_width, bot_w, &bot);
    bot_zero.ite(&then_, &else_)
}

fn map_types(bt: &BaseType) -> Result<(BaseType, BaseType), SmtError> {
    match bt {
        BaseType::Map(k, v) => Ok(((**k).clone(), (**v).clone())),
        other => Err(SmtError::TypeMismatch {
            expected: BaseType::make_map(BaseType::Unit, BaseType::Unit),
            actual: other.clone(),
        }),
    }
}

fn datatype_tag(bt: &BaseType) -> Result<&Sym, SmtError> {
    match bt {
        BaseType::Datatype(tag) => Ok(tag),
        _ => Err(SmtError::TypeMismatch {
            expected: BaseType::Datatype(Sym { name: None, num: 0 }),
            actual: bt.clone(),
        }),
    }
}

fn check_type(expected: &BaseType, actual: &BaseType) -> Result<(), SmtError> {
    if Sorts::same_type(expected, actual) {
        Ok(())
    } else {
        Err(SmtError::TypeMismatch {
            expected: expected.clone(),
            actual: actual.clone(),
        })
    }
}

fn record_member_index(bt: &BaseType, member: &Id) -> Result<(usize, BaseType), SmtError> {
    let BaseType::Record(members) = bt else {
        return Err(SmtError::TypeMismatch {
            expected: BaseType::Record(vec![]),
            actual: bt.clone(),
        });
    };
    members
        .iter()
        .enumerate()
        .find(|(_, (m, _))| m == member)
        .map(|(i, (_, t))| (i, t.clone()))
        .ok_or_else(|| SmtError::UnknownRecordMember {
            member: member.clone(),
        })
}

fn literal_integer(it: &IndexTerm) -> Option<BigInt> {
    match &it.term {
        Term::Const(Const::Z(n)) => Some(n.clone()),
        Term::Const(Const::Bits((sign, width), n)) => {
            Some(crate::gen_dsl::normalise_to_range_big(*sign, *width, n))
        }
        _ => None,
    }
}

fn unop_name(op: UnOp) -> &'static str {
    match op {
        UnOp::Not => "Not",
        UnOp::Negate => "Negate",
        UnOp::BwClz => "BwClz",
        UnOp::BwCtz => "BwCtz",
        UnOp::BwFfs => "BwFfs",
        UnOp::BwFls => "BwFls",
        UnOp::BwCompl => "BwCompl",
    }
}

fn binop_name(op: BinOp) -> &'static str {
    match op {
        BinOp::And => "And",
        BinOp::Or => "Or",
        BinOp::Implies => "Implies",
        BinOp::Add => "Add",
        BinOp::Sub => "Sub",
        BinOp::Mul => "Mul",
        BinOp::Div => "Div",
        BinOp::Exp => "Exp",
        BinOp::Rem => "Rem",
        BinOp::Mod => "Mod",
        BinOp::BwXor => "BwXor",
        BinOp::BwAnd => "BwAnd",
        BinOp::BwOr => "BwOr",
        BinOp::ShiftLeft => "ShiftLeft",
        BinOp::ShiftRight => "ShiftRight",
        BinOp::Lt => "Lt",
        BinOp::Le => "Le",
        BinOp::Min => "Min",
        BinOp::Max => "Max",
        BinOp::Eq => "Eq",
        BinOp::LtPointer => "LtPointer",
        BinOp::LePointer => "LePointer",
        BinOp::SetUnion => "SetUnion",
        BinOp::SetIntersection => "SetIntersection",
        BinOp::SetDifference => "SetDifference",
        BinOp::SetMember => "SetMember",
        BinOp::Subset => "Subset",
    }
}

fn term_name(term: &Term) -> &'static str {
    match term {
        Term::Const(_) => "a constant",
        Term::Sym(_) => "a variable",
        Term::Unop(_, _) => "a unary operation",
        Term::Binop(_, _, _) => "a binary operation",
        Term::Ite(_, _, _) => "an ite",
        Term::EachI { .. } => "an each",
        Term::Tuple(_) => "a tuple",
        Term::NthTuple(_, _) => "a tuple projection",
        Term::Struct(_, _) => "a struct",
        Term::StructMember(_, _) => "a struct projection",
        Term::StructUpdate { .. } => "a struct update",
        Term::Record(_) => "a record",
        Term::RecordMember(_, _) => "a record projection",
        Term::RecordUpdate { .. } => "a record update",
        Term::Constructor(_, _) => "a constructor",
        Term::MemberShift(_, _, _) => "a member shift",
        Term::ArrayShift { .. } => "an array shift",
        Term::CopyAllocId { .. } => "a copy_alloc_id",
        Term::HasAllocId(_) => "a has_alloc_id",
        Term::SizeOf(_) => "a sizeof",
        Term::OffsetOf(_, _) => "an offsetof",
        Term::Nil(_) => "nil",
        Term::Cons(_, _) => "a cons",
        Term::Head(_) => "a head",
        Term::Tail(_) => "a tail",
        Term::Representable(_, _) => "a representable",
        Term::Good(_, _) => "a good",
        Term::Aligned { .. } => "an aligned",
        Term::WrapI(_, _) => "a wrapI",
        Term::MapConst(_, _) => "a map constant",
        Term::MapSet(_, _, _) => "a map store",
        Term::MapGet(_, _) => "a map select",
        Term::MapDef(_, _) => "a map definition",
        Term::Apply(_, _) => "a function application",
        Term::Let(_, _) => "a let",
        Term::Match(_, _) => "a match",
        Term::Cast(_, _) => "a cast",
        Term::CnNone(_) => "none",
        Term::CnSome(_) => "some",
        Term::IsSome(_) => "an is_some",
        Term::GetOpt(_) => "a get_opt",
    }
}
