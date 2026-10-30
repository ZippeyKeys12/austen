use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::index_term::{Const, IndexTerm, Term};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;

use super::path_condition::{Guard, PathConditions};

pub(super) fn failed(bt: BaseType) -> GenTerm {
    GenTerm::assert(
        LogicalConstraint::T(IndexTerm::boolean(false)),
        GenTerm::return_(IndexTerm {
            term: Term::Const(Const::Unknown(bt.clone())),
            bt,
        }),
    )
}

pub(super) fn ite_from_arms(
    cond: IndexTerm,
    then_: Option<GenTerm>,
    else_: Option<GenTerm>,
) -> Option<GenTerm> {
    Some(match (then_, else_) {
        (Some(then_), Some(else_)) => GenTerm::ite(cond, then_, else_),
        (Some(then_), None) if cond.is_true() => then_,
        (None, Some(else_)) if cond.is_false() => else_,
        (Some(then_), None) => GenTerm::assert(LogicalConstraint::T(cond), then_),
        (None, Some(else_)) => GenTerm::assert(LogicalConstraint::T(IndexTerm::not_(cond)), else_),
        (None, None) => return None,
    })
}

struct Outcome {
    possible: bool,
    rebuilt: Option<GenTerm>,
}

impl Outcome {
    fn new<const BUILD: bool>(
        possible: bool,
        bt: &BaseType,
        rebuild: impl FnOnce() -> GenTerm,
    ) -> Self {
        Self {
            possible,
            rebuilt: BUILD.then(|| {
                if possible {
                    rebuild()
                } else {
                    failed(bt.clone())
                }
            }),
        }
    }

    fn term(self) -> GenTerm {
        self.rebuilt
            .expect("only rebuilding traversals request syntax")
    }
}

fn arm<const BUILD: bool>(guard: &Guard, term: &GenTerm, facts: &mut PathConditions) -> Outcome {
    let mark = facts.mark();
    let out = if facts.assume_guard(guard) {
        walk::<BUILD>(term, facts)
    } else {
        Outcome::new::<BUILD>(false, &term.bt, || unreachable!())
    };
    facts.rewind(mark);
    out
}

fn walk<const BUILD: bool>(term: &GenTerm, facts: &mut PathConditions) -> Outcome {
    let mark = facts.mark();
    let out = match &term.term {
        GenTermKind::Arbitrary | GenTermKind::Call { .. } | GenTermKind::Return(_) => {
            Outcome::new::<BUILD>(true, &term.bt, || term.clone())
        }
        GenTermKind::WriteChoice { label, value, rest } => {
            let rest = walk::<BUILD>(rest, facts);
            Outcome::new::<BUILD>(rest.possible, &term.bt, || {
                GenTerm::write_choice(*label, value.clone(), rest.term())
            })
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => {
            if matches!(addr.term, Term::Const(Const::Null)) {
                Outcome::new::<BUILD>(false, &term.bt, || unreachable!())
            } else {
                let rest = walk::<BUILD>(rest, facts);
                Outcome::new::<BUILD>(rest.possible, &term.bt, || {
                    GenTerm::asgn(addr.clone(), ct.clone(), value.clone(), rest.term())
                })
            }
        }
        GenTermKind::LetStar { var, value, rest } => {
            let value = walk::<BUILD>(value, facts);
            if value.possible {
                facts.shadow(var);
                let rest = walk::<BUILD>(rest, facts);
                Outcome::new::<BUILD>(rest.possible, &term.bt, || {
                    GenTerm::let_star((var.clone(), value.term()), rest.term())
                })
            } else {
                Outcome::new::<BUILD>(false, &term.bt, || unreachable!())
            }
        }
        GenTermKind::Assert { constraint, rest } => {
            let consistent = match constraint {
                LogicalConstraint::T(condition) => facts.assume(condition),
                LogicalConstraint::Forall {
                    permission, body, ..
                } => !(permission.is_true() && body.is_false()),
            };
            if consistent {
                let rest = walk::<BUILD>(rest, facts);
                Outcome::new::<BUILD>(rest.possible, &term.bt, || {
                    GenTerm::assert(constraint.clone(), rest.term())
                })
            } else {
                Outcome::new::<BUILD>(false, &term.bt, || unreachable!())
            }
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            let guard = Guard::of(cond);
            let then_ = arm::<BUILD>(&guard, then_, facts);
            if !BUILD && then_.possible {
                return Outcome::new::<BUILD>(true, &term.bt, || unreachable!());
            }
            let else_ = arm::<BUILD>(&guard.negated(), else_, facts);
            Outcome::new::<BUILD>(then_.possible || else_.possible, &term.bt, || {
                let then_ = then_.possible.then(|| then_.term());
                let else_ = else_.possible.then(|| else_.term());
                ite_from_arms(cond.clone(), then_, else_).expect("one arm is possible")
            })
        }
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => {
            if !BUILD {
                return Outcome::new::<BUILD>(true, &term.bt, || unreachable!());
            }
            facts.shadow(var);
            let body = arm::<BUILD>(&Guard::of(perm), body, facts);
            Outcome::new::<BUILD>(true, &term.bt, || {
                GenTerm::map((var.clone(), var_bt.clone(), perm.clone()), body.term())
            })
        }
        GenTermKind::Scope { name, body } => {
            let body = walk::<BUILD>(body, facts);
            Outcome::new::<BUILD>(body.possible, &term.bt, || {
                GenTerm::scope(name.clone(), body.term())
            })
        }
    };
    facts.rewind(mark);
    out
}

pub(super) fn possible(term: &GenTerm, facts: &mut PathConditions) -> bool {
    walk::<false>(term, facts).possible
}

pub(super) fn prune_under(term: &GenTerm, facts: &mut PathConditions) -> GenTerm {
    walk::<true>(term, facts).term()
}

fn always_backtracks(gt: &GenTerm) -> bool {
    match &gt.term {
        GenTermKind::Arbitrary | GenTermKind::Call { .. } | GenTermKind::Return(_) => false,
        GenTermKind::WriteChoice { rest, .. } => always_backtracks(rest),
        GenTermKind::Asgn { addr, rest, .. } => {
            matches!(addr.term, Term::Const(Const::Null)) || always_backtracks(rest)
        }
        GenTermKind::LetStar { value, rest, .. } => {
            always_backtracks(value) || always_backtracks(rest)
        }
        GenTermKind::Assert { constraint, rest } => match constraint {
            LogicalConstraint::T(it) if it.is_false() => true,
            LogicalConstraint::Forall {
                permission, body, ..
            } if permission.is_true() && body.is_false() => true,
            _ => always_backtracks(rest),
        },
        GenTermKind::Ite { then_, else_, .. } => {
            always_backtracks(then_) && always_backtracks(else_)
        }
        GenTermKind::Map { .. } => false,
        GenTermKind::Scope { body, .. } => always_backtracks(body),
    }
}

fn drop_doomed(term: GenTerm) -> GenTerm {
    let out: Result<GenTerm, std::convert::Infallible> = term.map_post(&mut |node| {
        Ok(match node.term {
            GenTermKind::Ite { cond, then_, else_ } => {
                if always_backtracks(&else_) {
                    GenTerm::assert(LogicalConstraint::T(cond), *then_)
                } else if always_backtracks(&then_) {
                    GenTerm::assert(LogicalConstraint::T(IndexTerm::not_(cond)), *else_)
                } else {
                    GenTerm::from_parts(GenTermKind::Ite { cond, then_, else_ }, node.bt)
                }
            }
            term => GenTerm::from_parts(term, node.bt),
        })
    });
    out.unwrap()
}

fn unwrap_trivial(term: GenTerm) -> GenTerm {
    let out: Result<GenTerm, std::convert::Infallible> = term.map_pre(&mut |node| {
        Ok(match node.term {
            GenTermKind::Ite { cond, then_, .. } if cond.is_true() => *then_,
            GenTermKind::Ite { cond, else_, .. } if cond.is_false() => *else_,
            term => GenTerm::from_parts(term, node.bt),
        })
    });
    out.unwrap()
}

pub fn prune_branches(term: GenTerm) -> GenTerm {
    unwrap_trivial(drop_doomed(term))
}

pub fn prune_branches_with_paths(term: GenTerm) -> GenTerm {
    prune_under(&term, &mut PathConditions::default())
}
