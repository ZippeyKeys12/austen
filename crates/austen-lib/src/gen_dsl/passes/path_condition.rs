use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::index_term::{BinOp, Const, IndexTerm, Term, UnOp};
use crate::gen_dsl::lang::sym::{Sym, SymSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Truth {
    True,
    False,
    Unknown,
}

#[derive(Clone)]
pub(super) struct Guard {
    key: Rc<IndexTerm>,
    negative: Rc<IndexTerm>,
}

impl Guard {
    pub(super) fn of(condition: &IndexTerm) -> Self {
        let key = normalize(condition);
        let negative = negate_normalized(key.clone());
        Self {
            key: Rc::new(key),
            negative: Rc::new(negative),
        }
    }

    pub(super) fn negated(&self) -> Self {
        Self {
            key: Rc::clone(&self.negative),
            negative: Rc::clone(&self.key),
        }
    }
}

enum Change {
    Added(Rc<IndexTerm>, SymSet),
    Removed(Rc<IndexTerm>, SymSet),
}

#[derive(Default)]
pub(super) struct PathConditions {
    facts: BTreeSet<Rc<IndexTerm>>,
    free_symbols: BTreeMap<Sym, usize>,
    journal: Vec<Change>,
}

impl PathConditions {
    pub(super) fn mark(&self) -> usize {
        self.journal.len()
    }

    pub(super) fn rewind(&mut self, mark: usize) {
        while self.journal.len() > mark {
            match self.journal.pop().unwrap() {
                Change::Added(key, vars) => {
                    self.facts.remove(&key);
                    self.remove_symbols(&vars);
                }
                Change::Removed(key, vars) => {
                    self.facts.insert(key);
                    self.add_symbols(&vars);
                }
            }
        }
    }

    pub(super) fn shadow(&mut self, sym: &Sym) {
        if !self.free_symbols.contains_key(sym) {
            return;
        }
        let removed: Vec<_> = self
            .facts
            .iter()
            .filter_map(|key| {
                let vars = key.free_vars();
                vars.contains(sym).then(|| (Rc::clone(key), vars))
            })
            .collect();
        for (key, vars) in removed {
            self.facts.remove(&key);
            self.remove_symbols(&vars);
            self.journal.push(Change::Removed(key, vars));
        }
    }

    fn query_key(&self, key: &IndexTerm, negative: &IndexTerm) -> Truth {
        if key.is_true() || self.facts.contains(key) {
            return Truth::True;
        }
        if key.is_false() || self.facts.contains(negative) {
            return Truth::False;
        }
        match (&key.term, &negative.term) {
            (Term::Binop(BinOp::And, a, b), Term::Binop(BinOp::Or, na, nb)) => {
                match (self.query_key(a, na), self.query_key(b, nb)) {
                    (Truth::False, _) | (_, Truth::False) => Truth::False,
                    (Truth::True, Truth::True) => Truth::True,
                    _ => Truth::Unknown,
                }
            }
            (Term::Binop(BinOp::Or, a, b), Term::Binop(BinOp::And, na, nb)) => {
                match (self.query_key(a, na), self.query_key(b, nb)) {
                    (Truth::True, _) | (_, Truth::True) => Truth::True,
                    (Truth::False, Truth::False) => Truth::False,
                    _ => Truth::Unknown,
                }
            }
            _ => Truth::Unknown,
        }
    }

    pub(super) fn assume(&mut self, condition: &IndexTerm) -> bool {
        self.assume_guard(&Guard::of(condition))
    }

    pub(super) fn assume_guard(&mut self, guard: &Guard) -> bool {
        if self.query_key(&guard.key, &guard.negative) == Truth::False {
            return false;
        }
        let mark = self.mark();
        self.insert_fact(Rc::clone(&guard.key));
        if self.assume_conjuncts(&guard.key, &guard.negative) {
            true
        } else {
            self.rewind(mark);
            false
        }
    }

    fn assume_conjuncts(&mut self, key: &IndexTerm, negative: &IndexTerm) -> bool {
        if let (Term::Binop(BinOp::And, a, b), Term::Binop(BinOp::Or, na, nb)) =
            (&key.term, &negative.term)
        {
            return self.assume_conjuncts(a, na) && self.assume_conjuncts(b, nb);
        }
        if self.query_key(key, negative) == Truth::False {
            return false;
        }
        if !self.facts.contains(key) {
            self.insert_fact(Rc::new(key.clone()));
        }
        true
    }

    fn insert_fact(&mut self, key: Rc<IndexTerm>) {
        if key.is_true() || !self.facts.insert(Rc::clone(&key)) {
            return;
        }
        let vars = key.free_vars();
        self.add_symbols(&vars);
        self.journal.push(Change::Added(key, vars));
    }

    fn add_symbols(&mut self, vars: &SymSet) {
        for sym in vars {
            *self.free_symbols.entry(sym.clone()).or_default() += 1;
        }
    }

    fn remove_symbols(&mut self, vars: &SymSet) {
        for sym in vars {
            let count = self.free_symbols.get_mut(sym).expect("active fact symbol");
            *count -= 1;
            if *count == 0 {
                self.free_symbols.remove(sym);
            }
        }
    }
}

fn binary(op: BinOp, a: IndexTerm, b: IndexTerm, bt: BaseType) -> IndexTerm {
    let replacement = match op {
        BinOp::And if a.is_false() || b.is_false() => Some(IndexTerm::boolean(false)),
        BinOp::Or if a.is_true() || b.is_true() => Some(IndexTerm::boolean(true)),
        BinOp::And if a.is_true() => Some(b.clone()),
        BinOp::And if b.is_true() => Some(a.clone()),
        BinOp::Or if a.is_false() => Some(b.clone()),
        BinOp::Or if b.is_false() => Some(a.clone()),
        _ => None,
    };
    if let Some(mut result) = replacement {
        result.bt = bt;
        result
    } else {
        IndexTerm {
            term: Term::Binop(op, Box::new(a), Box::new(b)),
            bt,
        }
    }
}

fn negate_normalized(it: IndexTerm) -> IndexTerm {
    let (term, bt) = it.into_parts();
    match term {
        Term::Const(Const::Bool(value)) => IndexTerm {
            term: Term::Const(Const::Bool(!value)),
            bt,
        },
        Term::Unop(UnOp::Not, inner) => {
            let mut inner = *inner;
            inner.bt = bt;
            inner
        }
        Term::Binop(BinOp::And, a, b) => {
            binary(BinOp::Or, negate_normalized(*a), negate_normalized(*b), bt)
        }
        Term::Binop(BinOp::Or, a, b) => {
            binary(BinOp::And, negate_normalized(*a), negate_normalized(*b), bt)
        }
        term => IndexTerm::not_(IndexTerm { term, bt }),
    }
}

fn normalize(condition: &IndexTerm) -> IndexTerm {
    condition.clone().map_post(&mut |it| {
        let (term, bt) = it.into_parts();
        match term {
            Term::Unop(UnOp::Not, a) if bt == BaseType::Bool && a.bt == BaseType::Bool => {
                let mut result = negate_normalized(*a);
                result.bt = bt;
                result
            }
            Term::Binop(BinOp::Implies, a, b)
                if bt == BaseType::Bool && a.bt == bt && b.bt == bt =>
            {
                binary(BinOp::Or, negate_normalized(*a), *b, bt)
            }
            Term::Binop(op @ (BinOp::And | BinOp::Or), a, b)
                if bt == BaseType::Bool && a.bt == bt && b.bt == bt =>
            {
                binary(op, *a, *b, bt)
            }
            Term::Binop(BinOp::Eq, a, b) if b < a => binary(BinOp::Eq, *b, *a, bt),
            Term::Binop(BinOp::Le, a, b)
                if bt == BaseType::Bool
                    && a.bt == b.bt
                    && matches!(
                        a.bt,
                        BaseType::Integer | BaseType::Bits(..) | BaseType::Real
                    ) =>
            {
                IndexTerm::not_(binary(BinOp::Lt, *b, *a, bt))
            }
            term => IndexTerm { term, bt },
        }
    })
}
