use num_bigint::BigInt;
use num_rational::BigRational;
use serde::{Deserialize, Serialize};

use crate::gen_dsl::lang::base_type::{BaseType, Sign};
use crate::gen_dsl::lang::id::Id;
use crate::gen_dsl::lang::sctype::{IntegerType, Sctype};
use crate::gen_dsl::lang::sym::Sym;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Const {
    Z(BigInt),
    Bits((Sign, u16), BigInt),
    Q(BigRational),
    MemByte {
        alloc_id: Option<BigInt>,
        value: BigInt,
    },
    Pointer {
        alloc_id: BigInt,
        addr: BigInt,
    },
    AllocId(BigInt),
    Bool(bool),
    Unit,
    Null,
    CTypeConst(Sctype),
    #[serde(rename = "Default")]
    InputDefault(BaseType),
    Unknown(BaseType),
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    strum::EnumIter,
)]
pub enum UnOp {
    Not,
    Negate,
    BwClz,
    BwCtz,
    BwFfs,
    BwFls,
    BwCompl,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    strum::EnumIter,
)]
pub enum BinOp {
    And,
    Or,
    Implies,
    Add,
    Sub,
    Mul,
    Div,
    Exp,
    Rem,
    Mod,
    BwXor,
    BwAnd,
    BwOr,
    ShiftLeft,
    ShiftRight,
    Lt,
    Le,
    Min,
    Max,
    Eq,
    LtPointer,
    LePointer,
    SetUnion,
    SetIntersection,
    SetDifference,
    SetMember,
    Subset,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PatternKind {
    Sym(Sym),
    Wild,
    Constructor(Sym, Vec<(Id, Pattern)>),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Pattern {
    pub pattern: PatternKind,
    pub bt: BaseType,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct IndexTerm {
    pub term: Term,
    pub bt: BaseType,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, strum::IntoStaticStr,
)]
pub enum Term {
    Const(Const),
    Sym(Sym),
    Unop(UnOp, Box<IndexTerm>),
    Binop(BinOp, Box<IndexTerm>, Box<IndexTerm>),
    Ite(Box<IndexTerm>, Box<IndexTerm>, Box<IndexTerm>),
    EachI {
        start: i64,
        var: (Sym, BaseType),
        end: i64,
        body: Box<IndexTerm>,
    },
    Tuple(Vec<IndexTerm>),
    NthTuple(usize, Box<IndexTerm>),
    Struct(Sym, Vec<(Id, IndexTerm)>),
    StructMember(Box<IndexTerm>, Id),
    StructUpdate {
        target: Box<IndexTerm>,
        member: Id,
        value: Box<IndexTerm>,
    },
    Record(Vec<(Id, IndexTerm)>),
    RecordMember(Box<IndexTerm>, Id),
    RecordUpdate {
        target: Box<IndexTerm>,
        member: Id,
        value: Box<IndexTerm>,
    },
    Constructor(Sym, Vec<(Id, IndexTerm)>),
    MemberShift(Box<IndexTerm>, Sym, Id),
    ArrayShift {
        base: Box<IndexTerm>,
        ct: Sctype,
        index: Box<IndexTerm>,
    },
    CopyAllocId {
        addr: Box<IndexTerm>,
        loc: Box<IndexTerm>,
    },
    HasAllocId(Box<IndexTerm>),
    SizeOf(Sctype),
    OffsetOf(Sym, Id),
    Nil(BaseType),
    Cons(Box<IndexTerm>, Box<IndexTerm>),
    Head(Box<IndexTerm>),
    Tail(Box<IndexTerm>),
    Representable(Sctype, Box<IndexTerm>),
    Good(Sctype, Box<IndexTerm>),
    Aligned {
        t: Box<IndexTerm>,
        align: Box<IndexTerm>,
    },
    WrapI(IntegerType, Box<IndexTerm>),
    MapConst(BaseType, Box<IndexTerm>),
    MapSet(Box<IndexTerm>, Box<IndexTerm>, Box<IndexTerm>),
    MapGet(Box<IndexTerm>, Box<IndexTerm>),
    MapDef((Sym, BaseType), Box<IndexTerm>),
    Apply(Sym, Vec<IndexTerm>),
    Let((Sym, Box<IndexTerm>), Box<IndexTerm>),
    Match(Box<IndexTerm>, Vec<(Pattern, IndexTerm)>),
    Cast(BaseType, Box<IndexTerm>),
    CnNone(BaseType),
    CnSome(Box<IndexTerm>),
    IsSome(Box<IndexTerm>),
    GetOpt(Box<IndexTerm>),
}

impl Term {
    fn owns_terms(&self) -> bool {
        !matches!(
            self,
            Term::Const(_)
                | Term::Sym(_)
                | Term::SizeOf(_)
                | Term::OffsetOf(_, _)
                | Term::Nil(_)
                | Term::CnNone(_)
        )
    }

    fn take_children_into(self, stack: &mut Vec<Term>) {
        fn drain(it: &mut IndexTerm, stack: &mut Vec<Term>) {
            if it.term.owns_terms() {
                stack.push(std::mem::replace(&mut it.term, Term::Const(Const::Unit)));
            }
        }

        match self {
            Term::Const(_)
            | Term::Sym(_)
            | Term::SizeOf(_)
            | Term::OffsetOf(_, _)
            | Term::Nil(_)
            | Term::CnNone(_) => {}
            Term::Unop(_, mut a)
            | Term::NthTuple(_, mut a)
            | Term::StructMember(mut a, _)
            | Term::RecordMember(mut a, _)
            | Term::MemberShift(mut a, _, _)
            | Term::HasAllocId(mut a)
            | Term::Head(mut a)
            | Term::Tail(mut a)
            | Term::Representable(_, mut a)
            | Term::Good(_, mut a)
            | Term::WrapI(_, mut a)
            | Term::MapConst(_, mut a)
            | Term::MapDef(_, mut a)
            | Term::Cast(_, mut a)
            | Term::CnSome(mut a)
            | Term::IsSome(mut a)
            | Term::GetOpt(mut a)
            | Term::EachI { body: mut a, .. } => drain(&mut a, stack),
            Term::Binop(_, mut a, mut b)
            | Term::Cons(mut a, mut b)
            | Term::MapGet(mut a, mut b)
            | Term::CopyAllocId {
                addr: mut a,
                loc: mut b,
            }
            | Term::Aligned {
                t: mut a,
                align: mut b,
            }
            | Term::StructUpdate {
                target: mut a,
                value: mut b,
                ..
            }
            | Term::RecordUpdate {
                target: mut a,
                value: mut b,
                ..
            }
            | Term::ArrayShift {
                base: mut a,
                index: mut b,
                ..
            }
            | Term::Let((_, mut a), mut b) => {
                drain(&mut a, stack);
                drain(&mut b, stack);
            }
            Term::Ite(mut a, mut b, mut c) | Term::MapSet(mut a, mut b, mut c) => {
                drain(&mut a, stack);
                drain(&mut b, stack);
                drain(&mut c, stack);
            }
            Term::Tuple(ts) | Term::Apply(_, ts) => {
                for mut t in ts {
                    drain(&mut t, stack);
                }
            }
            Term::Struct(_, ms) | Term::Record(ms) | Term::Constructor(_, ms) => {
                for (_, mut t) in ms {
                    drain(&mut t, stack);
                }
            }
            Term::Match(mut scrutinee, arms) => {
                drain(&mut scrutinee, stack);
                for (_, mut t) in arms {
                    drain(&mut t, stack);
                }
            }
        }
    }
}

impl Drop for IndexTerm {
    fn drop(&mut self) {
        if !self.term.owns_terms() {
            return;
        }
        let mut stack: Vec<Term> = Vec::new();
        std::mem::replace(&mut self.term, Term::Const(Const::Unit)).take_children_into(&mut stack);
        while let Some(t) = stack.pop() {
            t.take_children_into(&mut stack);
        }
    }
}

impl IndexTerm {
    pub fn into_parts(mut self) -> (Term, BaseType) {
        let term = std::mem::replace(&mut self.term, Term::Const(Const::Unit));
        let bt = std::mem::replace(&mut self.bt, BaseType::Unit);
        (term, bt)
    }

    pub fn as_sym(&self) -> Option<&Sym> {
        match &self.term {
            Term::Sym(s) => Some(s),
            _ => None,
        }
    }

    pub fn is_true(&self) -> bool {
        matches!(self.term, Term::Const(Const::Bool(true)))
    }

    pub fn is_false(&self) -> bool {
        matches!(self.term, Term::Const(Const::Bool(false)))
    }

    pub fn map_post(self, f: &mut impl FnMut(IndexTerm) -> IndexTerm) -> IndexTerm {
        fn go<F: FnMut(IndexTerm) -> IndexTerm>(
            mut t: Box<IndexTerm>,
            f: &mut F,
        ) -> Box<IndexTerm> {
            let taken = std::mem::replace(&mut *t, IndexTerm::unit());
            *t = taken.map_post(f);
            t
        }
        fn go_all<F: FnMut(IndexTerm) -> IndexTerm>(
            ts: Vec<IndexTerm>,
            f: &mut F,
        ) -> Vec<IndexTerm> {
            ts.into_iter().map(|t| t.map_post(f)).collect()
        }
        fn go_members<F: FnMut(IndexTerm) -> IndexTerm>(
            ms: Vec<(Id, IndexTerm)>,
            f: &mut F,
        ) -> Vec<(Id, IndexTerm)> {
            ms.into_iter().map(|(id, t)| (id, t.map_post(f))).collect()
        }

        let (term, bt) = self.into_parts();
        let term = match term {
            leaf @ (Term::Const(_)
            | Term::Sym(_)
            | Term::SizeOf(_)
            | Term::OffsetOf(_, _)
            | Term::Nil(_)
            | Term::CnNone(_)) => leaf,
            Term::Unop(op, a) => Term::Unop(op, go(a, f)),
            Term::Binop(op, a, b) => Term::Binop(op, go(a, f), go(b, f)),
            Term::Ite(c, a, b) => Term::Ite(go(c, f), go(a, f), go(b, f)),
            Term::EachI {
                start,
                var,
                end,
                body,
            } => Term::EachI {
                start,
                var,
                end,
                body: go(body, f),
            },
            Term::Tuple(ts) => Term::Tuple(go_all(ts, f)),
            Term::NthTuple(i, t) => Term::NthTuple(i, go(t, f)),
            Term::Struct(tag, ms) => Term::Struct(tag, go_members(ms, f)),
            Term::StructMember(t, id) => Term::StructMember(go(t, f), id),
            Term::StructUpdate {
                target,
                member,
                value,
            } => Term::StructUpdate {
                target: go(target, f),
                member,
                value: go(value, f),
            },
            Term::Record(ms) => Term::Record(go_members(ms, f)),
            Term::RecordMember(t, id) => Term::RecordMember(go(t, f), id),
            Term::RecordUpdate {
                target,
                member,
                value,
            } => Term::RecordUpdate {
                target: go(target, f),
                member,
                value: go(value, f),
            },
            Term::Constructor(tag, ms) => Term::Constructor(tag, go_members(ms, f)),
            Term::MemberShift(t, tag, id) => Term::MemberShift(go(t, f), tag, id),
            Term::ArrayShift { base, ct, index } => Term::ArrayShift {
                base: go(base, f),
                ct,
                index: go(index, f),
            },
            Term::CopyAllocId { addr, loc } => Term::CopyAllocId {
                addr: go(addr, f),
                loc: go(loc, f),
            },
            Term::HasAllocId(t) => Term::HasAllocId(go(t, f)),
            Term::Cons(h, t) => Term::Cons(go(h, f), go(t, f)),
            Term::Head(t) => Term::Head(go(t, f)),
            Term::Tail(t) => Term::Tail(go(t, f)),
            Term::Representable(ct, t) => Term::Representable(ct, go(t, f)),
            Term::Good(ct, t) => Term::Good(ct, go(t, f)),
            Term::Aligned { t, align } => Term::Aligned {
                t: go(t, f),
                align: go(align, f),
            },
            Term::WrapI(ity, t) => Term::WrapI(ity, go(t, f)),
            Term::MapConst(index_bt, t) => Term::MapConst(index_bt, go(t, f)),
            Term::MapSet(m, k, v) => Term::MapSet(go(m, f), go(k, f), go(v, f)),
            Term::MapGet(m, k) => Term::MapGet(go(m, f), go(k, f)),
            Term::MapDef(var, body) => Term::MapDef(var, go(body, f)),
            Term::Apply(name, args) => Term::Apply(name, go_all(args, f)),
            Term::Let((x, v), body) => Term::Let((x, go(v, f)), go(body, f)),
            Term::Match(scrutinee, arms) => Term::Match(
                go(scrutinee, f),
                arms.into_iter().map(|(p, t)| (p, t.map_post(f))).collect(),
            ),
            Term::Cast(target, t) => Term::Cast(target, go(t, f)),
            Term::CnSome(t) => Term::CnSome(go(t, f)),
            Term::IsSome(t) => Term::IsSome(go(t, f)),
            Term::GetOpt(t) => Term::GetOpt(go(t, f)),
        };
        f(IndexTerm { term, bt })
    }
}
