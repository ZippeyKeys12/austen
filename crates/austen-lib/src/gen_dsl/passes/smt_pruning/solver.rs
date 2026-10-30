use std::time::Duration;

use crate::gen_dsl::smt::{
    ConstraintSolver, Env, PointerEncoding, Program, SmtExpr, SolverConfig, Translator,
};
use crate::gen_dsl::{BaseType, BinOp, Const, IndexTerm, SymMap, Term, UnOp};

use super::Reasoner;

const QUERY_TIMEOUT: Duration = Duration::from_millis(10);
const QUERY_BUDGET: usize = 1024;
const MAX_QUERY_NODES: usize = 16_384;
const MAX_DEPTH: usize = 96;

pub(super) struct SmtReasoner {
    translator: Translator<'static>,
    solver: ConstraintSolver,
    vars: SymMap<SmtExpr>,
    active: Vec<IndexTerm>,
    remaining: usize,
    last: bool,
    sat_prefix: usize,
}

impl SmtReasoner {
    pub(super) fn new() -> Self {
        Self::with_budget(QUERY_BUDGET)
    }

    fn with_budget(budget: usize) -> Self {
        let config = SolverConfig {
            timeout: Some(QUERY_TIMEOUT),
            memory_max_mb: None,
        };
        Self {
            translator: Translator::with_pointers(Program::default(), PointerEncoding::Address),
            solver: ConstraintSolver::new(&config),
            vars: SymMap::new(),
            active: Vec::new(),
            remaining: budget,
            last: false,
            sat_prefix: 0,
        }
    }
}

impl Reasoner for SmtReasoner {
    fn impossible(&mut self, assumptions: &[IndexTerm]) -> bool {
        let mut supported = Vec::new();
        let mut types = SymMap::new();
        let mut nodes = MAX_QUERY_NODES;
        for assumption in assumptions {
            let Some(local) = supported_symbols_with_budget(assumption, &mut nodes) else {
                if nodes == 0 {
                    return false;
                }
                continue;
            };
            for (sym, bt) in local {
                if types.get(&sym).is_some_and(|previous| previous != &bt) {
                    return false;
                }
                types.insert(sym, bt);
            }
            supported.push(assumption);
        }
        if supported.is_empty() {
            return false;
        }

        let shared = self
            .active
            .iter()
            .zip(&supported)
            .take_while(|(previous, next)| previous == *next)
            .count();
        if shared == supported.len() {
            if shared == self.active.len() {
                return self.last;
            }
            if shared <= self.sat_prefix {
                return false;
            }
        }
        if self.remaining == 0 {
            return false;
        }

        for (sym, bt) in types {
            if self.vars.get(&sym).is_none_or(|value| value.bt() != &bt) {
                let Ok(value) = self.translator.declare(&sym, &bt) else {
                    return false;
                };
                self.vars.insert(sym, value);
            }
        }

        let removed = self.active.len() - shared;
        if removed != 0 {
            self.solver.pop(removed as u32);
            self.active.truncate(shared);
            self.sat_prefix = self.sat_prefix.min(shared);
        }
        for assumption in &supported[shared..] {
            self.solver.push();
            if let Ok(formula) = self.translator.boolean(&Env::Root(&self.vars), assumption) {
                self.solver.assert(&formula);
            }
            self.active.push((*assumption).clone());
        }
        self.remaining -= 1;
        let status = self.solver.check();
        if status.is_sat() {
            self.sat_prefix = self.active.len();
        }
        self.last = status.is_unsat();
        self.last
    }
}

pub(super) fn screen(it: &IndexTerm) -> Option<SymMap<BaseType>> {
    let mut remaining = MAX_QUERY_NODES;
    scalar_symbols(it, &mut remaining)
}

fn supported_symbols_with_budget(
    it: &IndexTerm,
    remaining: &mut usize,
) -> Option<SymMap<BaseType>> {
    if it.bt != BaseType::Bool {
        return None;
    }
    scalar_symbols(it, remaining)
}

fn scalar_symbols(it: &IndexTerm, remaining: &mut usize) -> Option<SymMap<BaseType>> {
    let mut symbols = SymMap::new();
    let mut work = vec![(it, 0)];
    while let Some((it, depth)) = work.pop() {
        if *remaining == 0 || depth >= MAX_DEPTH || !scalar(&it.bt) {
            return None;
        }
        *remaining -= 1;
        match &it.term {
            Term::Const(value) => {
                let consistent = match value {
                    Const::Bool(_) => it.bt == BaseType::Bool,
                    Const::Z(_) => it.bt == BaseType::Integer,
                    Const::Q(_) => it.bt == BaseType::Real,
                    Const::Bits((sign, width), _) => it.bt == BaseType::Bits(*sign, *width),
                    Const::Null | Const::Pointer { .. } => matches!(it.bt, BaseType::Loc(_)),
                    Const::InputDefault(bt) | Const::Unknown(bt) => it.bt == *bt,
                    Const::Unit
                    | Const::MemByte { .. }
                    | Const::AllocId(_)
                    | Const::CTypeConst(_) => false,
                };
                if !consistent {
                    return None;
                }
            }
            Term::Sym(sym) => {
                if symbols.get(sym).is_some_and(|previous| previous != &it.bt) {
                    return None;
                }
                symbols.insert(sym.clone(), it.bt.clone());
            }
            Term::Unop(op, operand) => {
                if it.bt != operand.bt {
                    return None;
                }
                let accepted = match op {
                    UnOp::Not => it.bt == BaseType::Bool,
                    UnOp::Negate => numeric(&it.bt),
                    UnOp::BwCompl | UnOp::BwClz | UnOp::BwCtz | UnOp::BwFfs | UnOp::BwFls => {
                        matches!(it.bt, BaseType::Bits(_, _))
                    }
                };
                if !accepted {
                    return None;
                }
                work.push((operand, depth + 1));
            }
            Term::Binop(op, left, right) => {
                let operands_agree = left.bt == right.bt
                    || matches!((&left.bt, &right.bt), (BaseType::Loc(_), BaseType::Loc(_)));
                if !operands_agree {
                    return None;
                }
                let accepted = match op {
                    BinOp::And | BinOp::Or | BinOp::Implies => {
                        it.bt == BaseType::Bool && left.bt == BaseType::Bool
                    }
                    BinOp::Eq => it.bt == BaseType::Bool,
                    BinOp::Lt | BinOp::Le => it.bt == BaseType::Bool && numeric(&left.bt),
                    BinOp::LtPointer | BinOp::LePointer => {
                        it.bt == BaseType::Bool && matches!(left.bt, BaseType::Loc(_))
                    }
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Min | BinOp::Max => {
                        it.bt == left.bt && numeric(&left.bt)
                    }
                    BinOp::BwXor | BinOp::BwAnd | BinOp::BwOr => {
                        it.bt == left.bt && matches!(left.bt, BaseType::Bits(_, _))
                    }
                    BinOp::Div
                    | BinOp::Rem
                    | BinOp::Mod
                    | BinOp::ShiftLeft
                    | BinOp::ShiftRight
                    | BinOp::Exp
                    | BinOp::SetUnion
                    | BinOp::SetIntersection
                    | BinOp::SetDifference
                    | BinOp::SetMember
                    | BinOp::Subset => false,
                };
                if !accepted {
                    return None;
                }
                work.push((left, depth + 1));
                work.push((right, depth + 1));
            }
            Term::Ite(guard, yes, no) => {
                if guard.bt != BaseType::Bool || yes.bt != no.bt || it.bt != yes.bt {
                    return None;
                }
                work.push((guard, depth + 1));
                work.push((yes, depth + 1));
                work.push((no, depth + 1));
            }
            Term::Cast(target, operand) => {
                let accepted = matches!(
                    (&operand.bt, target),
                    (BaseType::Bits(_, _), BaseType::Bits(_, _))
                        | (BaseType::Bits(_, _), BaseType::Loc(_))
                        | (BaseType::Loc(_), BaseType::Bits(_, _))
                        | (BaseType::Integer, BaseType::Real)
                        | (BaseType::Real, BaseType::Integer)
                );
                if it.bt != *target || !accepted {
                    return None;
                }
                work.push((operand, depth + 1));
            }
            _ => return None,
        }
    }
    Some(symbols)
}

fn numeric(bt: &BaseType) -> bool {
    matches!(
        bt,
        BaseType::Integer | BaseType::Real | BaseType::Bits(_, _)
    )
}

fn scalar(bt: &BaseType) -> bool {
    matches!(
        bt,
        BaseType::Bool
            | BaseType::Integer
            | BaseType::Real
            | BaseType::Loc(_)
            | BaseType::Bits(_, 1..=128)
    )
}
