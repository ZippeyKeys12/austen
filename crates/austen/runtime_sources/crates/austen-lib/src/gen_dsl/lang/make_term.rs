use num_bigint::BigInt;

use crate::gen_dsl::lang::base_type::{BaseType, Sign};
use crate::gen_dsl::lang::id::Id;
use crate::gen_dsl::lang::index_term::{BinOp, Const, IndexTerm, Term, UnOp};
use crate::gen_dsl::lang::sctype::Sctype;
use crate::gen_dsl::lang::sym::Sym;

pub fn uintptr_bt() -> BaseType {
    BaseType::Bits(Sign::Unsigned, 64)
}

fn fits_range(sign: Sign, width: u16, n: &BigInt) -> bool {
    let (min, max) = crate::gen_dsl::lang::base_type::bits_range(sign, width);
    *n >= min && *n <= max
}

impl IndexTerm {
    pub fn sym(sym: Sym, bt: BaseType) -> Self {
        IndexTerm {
            term: Term::Sym(sym),
            bt,
        }
    }

    pub fn unit() -> Self {
        IndexTerm {
            term: Term::Const(Const::Unit),
            bt: BaseType::Unit,
        }
    }

    pub fn null() -> Self {
        IndexTerm {
            term: Term::Const(Const::Null),
            bt: BaseType::Loc(None),
        }
    }

    pub fn boolean(b: bool) -> Self {
        IndexTerm {
            term: Term::Const(Const::Bool(b)),
            bt: BaseType::Bool,
        }
    }

    pub fn num_lit(n: BigInt, bt: &BaseType) -> Option<Self> {
        match bt {
            BaseType::Bits(sign, width) => fits_range(*sign, *width, &n).then(|| IndexTerm {
                term: Term::Const(Const::Bits((*sign, *width), n)),
                bt: bt.clone(),
            }),
            BaseType::Integer => Some(IndexTerm {
                term: Term::Const(Const::Z(n)),
                bt: BaseType::Integer,
            }),
            _ => None,
        }
    }

    pub fn struct_(tag: Sym, members: Vec<(Id, IndexTerm)>) -> Self {
        IndexTerm {
            bt: BaseType::Struct(tag.clone()),
            term: Term::Struct(tag, members),
        }
    }

    pub fn record(members: Vec<(Id, IndexTerm)>) -> Result<Self, Id> {
        let mut seen = std::collections::BTreeSet::new();
        for (id, _) in &members {
            if !seen.insert(id.clone()) {
                return Err(id.clone());
            }
        }
        let bt = BaseType::Record(
            members
                .iter()
                .map(|(id, t)| (id.clone(), t.bt.clone()))
                .collect(),
        );
        Ok(IndexTerm {
            term: Term::Record(members),
            bt,
        })
    }

    pub fn struct_member(base: IndexTerm, member: Id, member_bt: BaseType) -> Self {
        IndexTerm {
            term: Term::StructMember(Box::new(base), member),
            bt: member_bt,
        }
    }

    pub fn member_shift(base: IndexTerm, tag: Sym, member: Id) -> Self {
        IndexTerm {
            term: Term::MemberShift(Box::new(base), tag, member),
            bt: BaseType::Loc(None),
        }
    }

    pub fn array_shift(base: IndexTerm, ct: Sctype, index: IndexTerm) -> Self {
        IndexTerm {
            term: Term::ArrayShift {
                base: Box::new(base),
                ct,
                index: Box::new(IndexTerm::cast(uintptr_bt(), index)),
            },
            bt: BaseType::Loc(None),
        }
    }

    pub fn cast(bt: BaseType, it: IndexTerm) -> Self {
        if it.bt == bt {
            return it;
        }
        IndexTerm {
            term: Term::Cast(bt.clone(), Box::new(it)),
            bt,
        }
    }

    fn cmp(op: BinOp, l: IndexTerm, r: IndexTerm) -> Self {
        debug_assert_eq!(l.bt, r.bt, "{op:?} operands must have equal base types");
        IndexTerm {
            term: Term::Binop(op, Box::new(l), Box::new(r)),
            bt: BaseType::Bool,
        }
    }

    pub fn lt(l: IndexTerm, r: IndexTerm) -> Self {
        IndexTerm::cmp(BinOp::Lt, l, r)
    }

    pub fn le(l: IndexTerm, r: IndexTerm) -> Self {
        IndexTerm::cmp(BinOp::Le, l, r)
    }

    pub fn eq(l: IndexTerm, r: IndexTerm) -> Self {
        IndexTerm::cmp(BinOp::Eq, l, r)
    }

    pub fn and2(l: IndexTerm, r: IndexTerm) -> Self {
        IndexTerm {
            term: Term::Binop(BinOp::And, Box::new(l), Box::new(r)),
            bt: BaseType::Bool,
        }
    }

    pub fn not_(it: IndexTerm) -> Self {
        let bt = it.bt.clone();
        IndexTerm {
            term: Term::Unop(UnOp::Not, Box::new(it)),
            bt,
        }
    }
}
