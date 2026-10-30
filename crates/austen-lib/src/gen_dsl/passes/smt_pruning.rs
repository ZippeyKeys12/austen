use crate::gen_dsl::smt::SolverConfig;
use crate::gen_dsl::{
    BaseType, GenTerm, GenTermKind, IndexTerm, LogicalConstraint, Sym, SymGen, SymMap,
};

use super::branch_pruning::{failed, ite_from_arms};

mod solver;

trait Reasoner {
    fn impossible(&mut self, assumptions: &[IndexTerm]) -> bool;
}

pub fn prune_smt(iargs: &[(Sym, BaseType)], body: GenTerm) -> GenTerm {
    let bt = body.bt.clone();
    prune(iargs, body).unwrap_or_else(|| failed(bt))
}

fn prune(iargs: &[(Sym, BaseType)], body: GenTerm) -> Option<GenTerm> {
    SolverConfig::default().scope(|| {
        let mut walk = Walk::new(solver::SmtReasoner::new());
        for (sym, bt) in iargs {
            walk.bind(sym, Some(bt));
        }
        walk.visit(body, 0)
    })
}

struct Walk<R> {
    reasoner: R,
    facts: Vec<IndexTerm>,
    bindings: SymMap<Option<IndexTerm>>,
    journal: Vec<(Sym, Option<Option<IndexTerm>>)>,
    names: SymGen,
}

impl<R: Reasoner> Walk<R> {
    fn new(reasoner: R) -> Self {
        Self {
            reasoner,
            facts: Vec::new(),
            bindings: SymMap::new(),
            journal: Vec::new(),
            names: SymGen::starting_at(0),
        }
    }

    fn bind(&mut self, sym: &Sym, bt: Option<&BaseType>) -> Option<IndexTerm> {
        let fresh = bt.map(|bt| IndexTerm::sym(self.names.fresh_anon(), bt.clone()));
        let previous = self.bindings.insert(sym.clone(), fresh.clone());
        self.journal.push((sym.clone(), previous));
        fresh
    }

    fn prepare(&self, condition: &IndexTerm) -> Option<IndexTerm> {
        (condition.bt == BaseType::Bool)
            .then(|| self.prepare_value(condition))
            .flatten()
    }

    fn prepare_value(&self, value: &IndexTerm) -> Option<IndexTerm> {
        let mut substitution = SymMap::new();
        for (sym, bt) in solver::screen(value)? {
            let replacement = self.bindings.get(&sym)?.as_ref()?;
            if replacement.bt != bt {
                return None;
            }
            substitution.insert(sym, replacement.clone());
        }
        Some(value.subst(&substitution))
    }

    fn assume(&mut self, condition: &IndexTerm) {
        self.facts.extend(self.prepare(condition));
    }

    fn impossible(&mut self) -> bool {
        self.reasoner.impossible(&self.facts)
    }

    fn arm(&mut self, guard: Option<IndexTerm>, body: GenTerm, depth: usize) -> Option<GenTerm> {
        let mark = self.facts.len();
        self.facts.extend(guard);
        let result = self.visit(body, depth);
        self.facts.truncate(mark);
        result
    }

    fn visit(&mut self, term: GenTerm, depth: usize) -> Option<GenTerm> {
        if depth >= 256 {
            return Some(term);
        }
        let facts = self.facts.len();
        let bindings = self.journal.len();
        let result = self.descend(term, depth + 1);
        self.facts.truncate(facts);
        while self.journal.len() > bindings {
            let (sym, previous) = self.journal.pop().unwrap();
            if let Some(previous) = previous {
                self.bindings.insert(sym, previous);
            } else {
                self.bindings.remove(&sym);
            }
        }
        result
    }

    fn descend(&mut self, term: GenTerm, depth: usize) -> Option<GenTerm> {
        let (kind, bt) = term.into_parts();
        let kind = match kind {
            leaf @ (GenTermKind::Arbitrary | GenTermKind::Return(_) | GenTermKind::Call { .. }) => {
                return (!self.impossible()).then(|| GenTerm::from_parts(leaf, bt));
            }
            GenTermKind::Assert { constraint, rest } => {
                if let LogicalConstraint::T(condition) = &constraint {
                    self.assume(condition);
                }
                GenTermKind::Assert {
                    constraint,
                    rest: Box::new(self.visit(*rest, depth)?),
                }
            }
            GenTermKind::Ite { cond, then_, else_ } => {
                if self.impossible() {
                    return None;
                }
                let guard = self.prepare(&cond);
                let then_ = self.arm(guard.clone(), *then_, depth);
                let else_ = self.arm(guard.map(IndexTerm::not_), *else_, depth);
                return ite_from_arms(cond, then_, else_);
            }
            GenTermKind::LetStar { var, value, rest } => {
                let returned = if let GenTermKind::Return(it) = &value.term {
                    (value.bt == it.bt)
                        .then(|| self.prepare_value(it))
                        .flatten()
                } else {
                    None
                };
                let known_type = matches!(value.term, GenTermKind::Arbitrary) || returned.is_some();
                let value = self.visit(*value, depth)?;
                let fresh = self.bind(&var, known_type.then_some(&value.bt));
                if let (Some(fresh), Some(returned)) = (fresh, returned) {
                    self.facts.push(IndexTerm::eq(fresh, returned));
                }
                GenTermKind::LetStar {
                    var,
                    value: Box::new(value),
                    rest: Box::new(self.visit(*rest, depth)?),
                }
            }
            GenTermKind::Map {
                var,
                var_bt,
                perm,
                body,
            } => {
                if self.impossible() {
                    return None;
                }
                self.bind(&var, Some(&var_bt));
                let body_bt = body.bt.clone();
                let permission = self.prepare(&perm);
                let body = self
                    .arm(permission, *body, depth)
                    .unwrap_or_else(|| failed(body_bt));
                GenTermKind::Map {
                    var,
                    var_bt,
                    perm,
                    body: Box::new(body),
                }
            }
            GenTermKind::Scope { name, body } => GenTermKind::Scope {
                name,
                body: Box::new(self.visit(*body, depth)?),
            },
            GenTermKind::WriteChoice { label, value, rest } => GenTermKind::WriteChoice {
                label,
                value,
                rest: Box::new(self.visit(*rest, depth)?),
            },
            GenTermKind::Asgn {
                addr,
                ct,
                value,
                rest,
            } => {
                if matches!(addr.bt, BaseType::Loc(_))
                    && let Some(prepared) = self.prepare_value(&addr)
                {
                    self.facts
                        .push(IndexTerm::not_(IndexTerm::eq(prepared, IndexTerm::null())));
                }
                GenTermKind::Asgn {
                    addr,
                    ct,
                    value,
                    rest: Box::new(self.visit(*rest, depth)?),
                }
            }
        };
        Some(GenTerm::from_parts(kind, bt))
    }
}
