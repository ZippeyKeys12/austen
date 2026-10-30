use serde::{Deserialize, Serialize};

use crate::gen_dsl::choice::tree::{Label, ScopeId, ScopeKey};
use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::eval::Evaluator;
use crate::gen_dsl::lang::gen_module::GenModule;
use crate::gen_dsl::lang::index_term::{Const, IndexTerm, Term};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::sctype::Sctype;
use crate::gen_dsl::lang::sym::{Sym, SymGen, SymMap, SymSet};
use crate::gen_dsl::lang::value::Value;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GenTerm {
    pub term: GenTermKind,
    pub bt: BaseType,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GenTermKind {
    Arbitrary,
    WriteChoice {
        label: Label,
        value: IndexTerm,
        rest: Box<GenTerm>,
    },
    Call {
        fsym: Sym,
        args: Vec<IndexTerm>,
    },
    Asgn {
        addr: IndexTerm,
        ct: Sctype,
        value: IndexTerm,
        rest: Box<GenTerm>,
    },
    LetStar {
        var: Sym,
        value: Box<GenTerm>,
        rest: Box<GenTerm>,
    },
    Return(IndexTerm),
    Assert {
        constraint: LogicalConstraint,
        rest: Box<GenTerm>,
    },
    Ite {
        cond: IndexTerm,
        then_: Box<GenTerm>,
        else_: Box<GenTerm>,
    },
    Map {
        var: Sym,
        var_bt: BaseType,
        perm: IndexTerm,
        body: Box<GenTerm>,
    },
    Scope {
        name: ScopeName,
        body: Box<GenTerm>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChooseBoolError {
    #[error("a Boolean choice needs at least one alternative")]
    NoAlternatives,
    #[error("Boolean-choice alternative {at} has base type {actual:?}, not {expected:?}")]
    BranchTypeMismatch {
        at: usize,
        expected: BaseType,
        actual: BaseType,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ScopeName {
    Str(String),
    Index(IndexTerm),
    Sid(ScopeId),
}

impl ScopeName {
    pub fn const_key(&self) -> Option<ScopeKey> {
        match self {
            ScopeName::Str(s) => Some(ScopeKey::Str(s.clone())),
            ScopeName::Sid(sid) => Some(ScopeKey::Sid(sid.clone())),
            ScopeName::Index(it) => Self::const_index(it).map(ScopeKey::Index),
        }
    }

    fn const_index(it: &IndexTerm) -> Option<i128> {
        match &it.term {
            Term::Const(Const::Z(n)) | Term::Const(Const::Bits(_, n)) => i128::try_from(n).ok(),
            _ => None,
        }
    }

    pub fn eval_key(&self, env: &SymMap<Value>, ctx: &GenModule) -> Option<ScopeKey> {
        self.eval_key_with_evaluator(env, &Evaluator::new(ctx))
    }

    pub fn eval_key_with_evaluator(
        &self,
        env: &SymMap<Value>,
        evaluator: &Evaluator<'_>,
    ) -> Option<ScopeKey> {
        match self {
            ScopeName::Str(s) => Some(ScopeKey::Str(s.clone())),
            ScopeName::Sid(sid) => Some(ScopeKey::Sid(sid.clone())),
            ScopeName::Index(it) => evaluator.eval(it, env)?.as_int().map(ScopeKey::Index),
        }
    }

    pub fn free_vars(&self) -> SymSet {
        match self {
            ScopeName::Str(_) | ScopeName::Sid(_) => SymSet::new(),
            ScopeName::Index(it) => it.free_vars(),
        }
    }
}

impl From<&str> for ScopeName {
    fn from(s: &str) -> Self {
        ScopeName::Str(s.to_owned())
    }
}

impl From<ScopeId> for ScopeName {
    fn from(sid: ScopeId) -> Self {
        ScopeName::Sid(sid)
    }
}

impl From<String> for ScopeName {
    fn from(s: String) -> Self {
        ScopeName::Str(s)
    }
}

impl From<IndexTerm> for ScopeName {
    fn from(it: IndexTerm) -> Self {
        ScopeName::Index(it)
    }
}

impl GenTerm {
    pub fn from_parts(term: GenTermKind, bt: BaseType) -> Self {
        Self { term, bt }
    }

    pub fn into_parts(self) -> (GenTermKind, BaseType) {
        (self.term, self.bt)
    }

    pub fn arbitrary(bt: BaseType) -> Self {
        Self::from_parts(GenTermKind::Arbitrary, bt)
    }

    pub fn write_choice(label: Label, value: IndexTerm, rest: Self) -> Self {
        let bt = rest.bt.clone();
        Self::from_parts(
            GenTermKind::WriteChoice {
                label,
                value,
                rest: Box::new(rest),
            },
            bt,
        )
    }

    pub fn recorded_arbitrary(label: Label, var: Sym, bt: BaseType, rest: Self) -> Self {
        let value = IndexTerm::sym(var.clone(), bt.clone());
        Self::let_star(
            (var, Self::arbitrary(bt)),
            Self::write_choice(label, value, rest),
        )
    }

    pub fn recorded_arbitrary_value(label: Label, var: Sym, bt: BaseType) -> Self {
        let value = IndexTerm::sym(var.clone(), bt.clone());
        Self::recorded_arbitrary(label, var, bt, Self::return_(value))
    }

    pub fn call(fsym: Sym, args: Vec<IndexTerm>, bt: BaseType) -> Self {
        Self::from_parts(GenTermKind::Call { fsym, args }, bt)
    }

    pub fn asgn(addr: IndexTerm, ct: Sctype, value: IndexTerm, rest: Self) -> Self {
        let bt = rest.bt.clone();
        Self::from_parts(
            GenTermKind::Asgn {
                addr,
                ct,
                value,
                rest: Box::new(rest),
            },
            bt,
        )
    }

    pub fn let_star((var, value): (Sym, Self), rest: Self) -> Self {
        let bt = rest.bt.clone();
        Self::from_parts(
            GenTermKind::LetStar {
                var,
                value: Box::new(value),
                rest: Box::new(rest),
            },
            bt,
        )
    }

    pub fn return_(it: IndexTerm) -> Self {
        let bt = it.bt.clone();
        Self::from_parts(GenTermKind::Return(it), bt)
    }

    pub fn assert(constraint: LogicalConstraint, rest: Self) -> Self {
        let bt = rest.bt.clone();
        Self::from_parts(
            GenTermKind::Assert {
                constraint,
                rest: Box::new(rest),
            },
            bt,
        )
    }

    pub fn ite(cond: IndexTerm, then_: Self, else_: Self) -> Self {
        assert_eq!(then_.bt, else_.bt, "ITE branches must have equal basetypes");
        Self::rebuild_ite(cond, then_, else_)
    }

    pub(in crate::gen_dsl) fn rebuild_ite(cond: IndexTerm, then_: Self, else_: Self) -> Self {
        debug_assert_eq!(then_.bt, else_.bt, "ITE branches must have equal basetypes");
        let bt = then_.bt.clone();
        Self::from_parts(
            GenTermKind::Ite {
                cond,
                then_: Box::new(then_),
                else_: Box::new(else_),
            },
            bt,
        )
    }

    pub fn map((var, var_bt, perm): (Sym, BaseType, IndexTerm), body: Self) -> Self {
        let bt = BaseType::make_map(var_bt.clone(), body.bt.clone());
        Self::from_parts(
            GenTermKind::Map {
                var,
                var_bt,
                perm,
                body: Box::new(body),
            },
            bt,
        )
    }

    pub fn scoped_map((var, var_bt, perm): (Sym, BaseType, IndexTerm), body: Self) -> Self {
        let index = IndexTerm::sym(var.clone(), var_bt.clone());
        Self::map((var, var_bt, perm), Self::scope(index, body))
    }

    pub fn scope(name: impl Into<ScopeName>, body: Self) -> Self {
        let bt = body.bt.clone();
        Self::from_parts(
            GenTermKind::Scope {
                name: name.into(),
                body: Box::new(body),
            },
            bt,
        )
    }

    pub fn choose_bool(alts: Vec<Self>, syms: &mut SymGen) -> Self {
        Self::try_choose_bool(alts, syms).expect("well-formed Boolean-choice alternatives")
    }

    pub fn try_choose_bool(alts: Vec<Self>, syms: &mut SymGen) -> Result<Self, ChooseBoolError> {
        let Some(first) = alts.first() else {
            return Err(ChooseBoolError::NoAlternatives);
        };
        let bt = first.bt.clone();
        for (at, alt) in alts.iter().enumerate().skip(1) {
            if alt.bt != bt {
                return Err(ChooseBoolError::BranchTypeMismatch {
                    at,
                    expected: bt,
                    actual: alt.bt.clone(),
                });
            }
        }

        fn lower(mut alts: Vec<GenTerm>, syms: &mut SymGen) -> GenTerm {
            if alts.len() == 1 {
                return alts.pop().expect("one alternative");
            }

            let selector = syms.fresh("choose");
            let later = alts.split_off(alts.len().div_ceil(2));
            let earlier = lower(alts, syms);
            let later = lower(later, syms);
            let cond = IndexTerm::sym(selector.clone(), BaseType::Bool);
            GenTerm::let_star(
                (selector, GenTerm::arbitrary(BaseType::Bool)),
                GenTerm::ite(cond, earlier, later),
            )
        }

        Ok(lower(alts, syms))
    }

    pub fn try_ite(
        cond: IndexTerm,
        then_: Self,
        else_: Self,
    ) -> Result<Self, (BaseType, BaseType)> {
        if then_.bt != else_.bt {
            return Err((then_.bt, else_.bt));
        }
        Ok(Self::ite(cond, then_, else_))
    }

    pub fn map_pre<E>(self, f: &mut impl FnMut(Self) -> Result<Self, E>) -> Result<Self, E> {
        let (term, bt) = f(self)?.into_parts();
        Ok(match term {
            GenTermKind::Arbitrary | GenTermKind::Call { .. } | GenTermKind::Return(_) => {
                Self::from_parts(term, bt)
            }
            GenTermKind::WriteChoice { label, value, rest } => {
                Self::write_choice(label, value, (*rest).map_pre(f)?)
            }
            GenTermKind::Asgn {
                addr,
                ct,
                value,
                rest,
            } => Self::asgn(addr, ct, value, (*rest).map_pre(f)?),
            GenTermKind::LetStar { var, value, rest } => {
                Self::let_star((var, (*value).map_pre(f)?), (*rest).map_pre(f)?)
            }
            GenTermKind::Assert { constraint, rest } => {
                Self::assert(constraint, (*rest).map_pre(f)?)
            }
            GenTermKind::Ite { cond, then_, else_ } => {
                let then_ = (*then_).map_pre(f)?;
                let else_ = (*else_).map_pre(f)?;
                Self::rebuild_ite(cond, then_, else_)
            }
            GenTermKind::Map {
                var,
                var_bt,
                perm,
                body,
            } => Self::map((var, var_bt, perm), (*body).map_pre(f)?),
            GenTermKind::Scope { name, body } => Self::scope(name, (*body).map_pre(f)?),
        })
    }

    pub fn map_post<E>(self, f: &mut impl FnMut(Self) -> Result<Self, E>) -> Result<Self, E> {
        let (term, bt) = self.into_parts();
        let rebuilt = match term {
            GenTermKind::Arbitrary | GenTermKind::Call { .. } | GenTermKind::Return(_) => {
                Self::from_parts(term, bt)
            }
            GenTermKind::WriteChoice { label, value, rest } => {
                Self::write_choice(label, value, (*rest).map_post(f)?)
            }
            GenTermKind::Asgn {
                addr,
                ct,
                value,
                rest,
            } => Self::asgn(addr, ct, value, (*rest).map_post(f)?),
            GenTermKind::LetStar { var, value, rest } => {
                Self::let_star((var, (*value).map_post(f)?), (*rest).map_post(f)?)
            }
            GenTermKind::Assert { constraint, rest } => {
                Self::assert(constraint, (*rest).map_post(f)?)
            }
            GenTermKind::Ite { cond, then_, else_ } => {
                let then_ = (*then_).map_post(f)?;
                let else_ = (*else_).map_post(f)?;
                Self::rebuild_ite(cond, then_, else_)
            }
            GenTermKind::Map {
                var,
                var_bt,
                perm,
                body,
            } => Self::map((var, var_bt, perm), (*body).map_post(f)?),
            GenTermKind::Scope { name, body } => Self::scope(name, (*body).map_post(f)?),
        };
        f(rebuilt)
    }
}
