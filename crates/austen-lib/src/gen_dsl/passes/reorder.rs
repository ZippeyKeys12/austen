use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::index_term::{BinOp, IndexTerm, Term};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::sctype::Sctype;
use crate::gen_dsl::lang::sym::{Sym, SymMap, SymSet};
use crate::gen_dsl::passes::TransformError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Stmt {
    Asgn {
        addr: IndexTerm,
        ct: Sctype,
        value: IndexTerm,
    },
    LetStar {
        var: Sym,
        value: GenTerm,
    },
    WriteChoice {
        label: crate::gen_dsl::Label,
        value: IndexTerm,
    },
    Assert(LogicalConstraint),
}

pub(super) fn stmts_of(mut gt: GenTerm) -> (Vec<Stmt>, GenTerm) {
    let mut stmts = Vec::new();
    loop {
        let (term, bt) = gt.into_parts();
        gt = match term {
            GenTermKind::Asgn {
                addr,
                ct,
                value,
                rest,
            } => {
                stmts.push(Stmt::Asgn { addr, ct, value });
                *rest
            }
            GenTermKind::LetStar { var, value, rest } => {
                stmts.push(Stmt::LetStar { var, value: *value });
                *rest
            }
            GenTermKind::WriteChoice { label, value, rest } => {
                stmts.push(Stmt::WriteChoice { label, value });
                *rest
            }
            GenTermKind::Assert { constraint, rest } => {
                stmts.push(Stmt::Assert(constraint));
                *rest
            }
            term @ (GenTermKind::Arbitrary
            | GenTermKind::Call { .. }
            | GenTermKind::Return(_)
            | GenTermKind::Ite { .. }
            | GenTermKind::Map { .. }
            | GenTermKind::Scope { .. }) => return (stmts, GenTerm::from_parts(term, bt)),
        };
    }
}

pub(super) fn rebuild(stmts: Vec<Stmt>, last: GenTerm) -> GenTerm {
    stmts.into_iter().rev().fold(last, |rest, stmt| match stmt {
        Stmt::Asgn { addr, ct, value } => GenTerm::asgn(addr, ct, value, rest),
        Stmt::LetStar { var, value } => GenTerm::let_star((var, value), rest),
        Stmt::WriteChoice { label, value } => GenTerm::write_choice(label, value, rest),
        Stmt::Assert(constraint) => GenTerm::assert(constraint, rest),
    })
}

#[derive(Default)]
struct Digraph {
    verts: SymSet,
    succs: SymMap<SymSet>,
}

impl Digraph {
    fn add_vertex(&mut self, sym: &Sym) {
        if !self.verts.contains(sym) {
            self.verts.insert(sym.clone());
        }
    }

    fn add_edge(&mut self, before: &Sym, after: &Sym) {
        self.add_vertex(before);
        self.add_vertex(after);
        self.succs
            .entry(before.clone())
            .or_default()
            .insert(after.clone());
    }

    fn reaches(&self, from: &Sym, to: &Sym) -> bool {
        if from == to {
            return true;
        }
        let mut seen = SymSet::new();
        let mut stack = vec![from];
        while let Some(cur) = stack.pop() {
            for next in self.succs.get(cur).into_iter().flatten() {
                if next == to {
                    return true;
                }
                if seen.insert(next.clone()) {
                    stack.push(next);
                }
            }
        }
        false
    }

    fn add_edge_no_cycle(&mut self, before: &Sym, after: &Sym) {
        if self.reaches(after, before) {
            return;
        }
        self.add_edge(before, after);
    }

    fn topo_sort(&self) -> Result<Vec<Sym>, Sym> {
        let mut indegree: SymMap<usize> = self.verts.iter().map(|s| (s.clone(), 0)).collect();
        for afters in self.succs.values() {
            for after in afters {
                *indegree
                    .get_mut(after)
                    .expect("edge endpoints are vertices") += 1;
            }
        }
        let mut ready: SymSet = indegree
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(s, _)| s.clone())
            .collect();
        let mut out = Vec::with_capacity(self.verts.len());
        while let Some(next) = ready.pop_first() {
            for after in self.succs.get(&next).into_iter().flatten() {
                let d = indegree
                    .get_mut(after)
                    .expect("edge endpoints are vertices");
                *d -= 1;
                if *d == 0 {
                    ready.insert(after.clone());
                }
            }
            out.push(next);
        }
        if out.len() < self.verts.len() {
            let stuck = self
                .verts
                .iter()
                .find(|s| !out.contains(s))
                .expect("a vertex the sort never freed exists")
                .clone();
            return Err(stuck);
        }
        Ok(out)
    }
}

fn collect_dependencies(iargs: &SymSet, stmts: &[Stmt], g: &mut Digraph) {
    for stmt in stmts {
        let Stmt::LetStar { var, value } = stmt else {
            continue;
        };
        g.add_vertex(var);
        for read in value.free_vars() {
            if !iargs.contains(&read) {
                g.add_edge(&read, var);
            }
        }
    }
}

fn eq_target(side: &IndexTerm) -> Option<&Sym> {
    match &side.term {
        Term::Sym(x) => Some(x),
        Term::Cast(_, inner) => match &inner.term {
            Term::Sym(x) => Some(x),
            _ => None,
        },
        _ => None,
    }
}

fn consider_equalities(iargs: &SymSet, stmts: &[Stmt], g: &mut Digraph) {
    for stmt in stmts {
        match stmt {
            Stmt::LetStar { var, .. } => g.add_vertex(var),
            Stmt::Assert(LogicalConstraint::T(it)) => {
                let Term::Binop(BinOp::Eq, lhs, rhs) = &it.term else {
                    continue;
                };
                let (x, expr) = match eq_target(lhs) {
                    Some(x) if !rhs.free_vars().contains(x) => (x, &**rhs),
                    _ => match eq_target(rhs) {
                        Some(x) if !lhs.free_vars().contains(x) => (x, &**lhs),
                        _ => continue,
                    },
                };
                for read in expr.free_vars() {
                    if iargs.contains(&read) || read == *x {
                        continue;
                    }
                    g.add_edge_no_cycle(&read, x);
                }
            }
            Stmt::Assert(LogicalConstraint::Forall { .. })
            | Stmt::Asgn { .. }
            | Stmt::WriteChoice { .. } => {}
        }
    }
}

fn contains_call(gt: &GenTerm) -> bool {
    match &gt.term {
        GenTermKind::Call { .. } => true,
        GenTermKind::Arbitrary | GenTermKind::Return(_) => false,
        GenTermKind::Asgn { rest, .. }
        | GenTermKind::WriteChoice { rest, .. }
        | GenTermKind::Assert { rest, .. } => contains_call(rest),
        GenTermKind::LetStar { value, rest, .. } => contains_call(value) || contains_call(rest),
        GenTermKind::Ite { then_, else_, .. } => contains_call(then_) || contains_call(else_),
        GenTermKind::Map { body, .. } | GenTermKind::Scope { body, .. } => contains_call(body),
    }
}

fn consider_constrained_calls(from_calls: &SymSet, stmts: &[Stmt], g: &mut Digraph) {
    let Some((first, rest)) = stmts.split_first() else {
        return;
    };
    match first {
        Stmt::LetStar { var, value } if contains_call(value) => {
            let mut grown = from_calls.clone();
            grown.insert(var.clone());
            consider_constrained_calls(&grown, rest, g);
        }
        Stmt::LetStar { .. } | Stmt::Asgn { .. } | Stmt::WriteChoice { .. } => {
            consider_constrained_calls(from_calls, rest, g)
        }
        Stmt::Assert(lc) => {
            consider_constrained_calls(from_calls, rest, g);
            let reads: SymSet = lc.free_vars_bts().into_keys().collect();
            for constrained in reads.difference(from_calls) {
                for from_call in reads.intersection(from_calls) {
                    g.add_edge_no_cycle(from_call, constrained);
                }
            }
        }
    }
}

fn consider_original_ordering(stmts: &[Stmt], g: &mut Digraph) {
    let mut defined = SymSet::new();
    for stmt in stmts {
        let Stmt::LetStar { var, .. } = stmt else {
            continue;
        };
        for earlier in &defined {
            g.add_edge_no_cycle(earlier, var);
        }
        defined.insert(var.clone());
    }
}

fn variable_ordering(iargs: &SymSet, stmts: &[Stmt]) -> Result<Vec<Sym>, TransformError> {
    let mut g = Digraph::default();
    collect_dependencies(iargs, stmts, &mut g);
    consider_equalities(iargs, stmts, &mut g);
    consider_constrained_calls(&SymSet::new(), stmts, &mut g);
    consider_original_ordering(stmts, &mut g);
    g.topo_sort()
        .map_err(|sym| TransformError::CyclicSpineDependency { sym })
}

pub(super) fn stmt_reads(stmt: &Stmt) -> SymSet {
    match stmt {
        Stmt::Asgn { addr, value, .. } => {
            let mut reads = addr.free_vars();
            reads.extend(value.free_vars());
            reads
        }
        Stmt::Assert(lc) => lc.free_vars(),
        Stmt::LetStar { value, .. } => value.free_vars(),
        Stmt::WriteChoice { value, .. } => value.free_vars(),
    }
}

pub(super) fn stmt_binds(stmt: &Stmt) -> Option<&Sym> {
    match stmt {
        Stmt::LetStar { var, .. } => Some(var),
        _ => None,
    }
}

fn drain_schedulable(stmts: Vec<Stmt>, vars: &SymSet, out: &mut Vec<Stmt>) -> Vec<Stmt> {
    stmts
        .into_iter()
        .filter_map(|stmt| {
            let schedulable = match &stmt {
                Stmt::Asgn { addr, value, .. } => {
                    addr.free_vars().is_subset(vars) && value.free_vars().is_subset(vars)
                }
                Stmt::Assert(lc) => lc.free_vars_bts().keys().all(|read| vars.contains(read)),
                Stmt::WriteChoice { value, .. } => value.free_vars().is_subset(vars),
                Stmt::LetStar { .. } => false,
            };
            if schedulable {
                out.push(stmt);
                None
            } else {
                Some(stmt)
            }
        })
        .collect()
}

fn statement_ordering(iargs: &SymSet, stmts: Vec<Stmt>) -> Result<Vec<Stmt>, TransformError> {
    let syms = variable_ordering(iargs, &stmts)?;
    let mut vars = iargs.clone();
    let mut out = Vec::with_capacity(stmts.len());
    let mut remaining = stmts;
    for sym in syms {
        remaining = drain_schedulable(remaining, &vars, &mut out);
        remaining = remaining
            .into_iter()
            .filter_map(|stmt| {
                let binds_sym = matches!(&stmt, Stmt::LetStar { var, .. } if *var == sym);
                if binds_sym {
                    out.push(stmt);
                    None
                } else {
                    Some(stmt)
                }
            })
            .collect();
        vars.insert(sym);
    }
    remaining = drain_schedulable(remaining, &vars, &mut out);
    if !remaining.is_empty() {
        let mut unbound = SymSet::new();
        for stmt in &remaining {
            unbound.extend(stmt_reads(stmt).difference(&vars).cloned());
        }
        return Err(TransformError::UnschedulableStatements {
            syms: unbound.into_iter().collect(),
        });
    }
    Ok(out)
}

pub fn reorder(iargs: &SymSet, term: GenTerm) -> Result<GenTerm, TransformError> {
    let (stmts, last) = stmts_of(term);
    let stmts = statement_ordering(iargs, stmts)?;
    walk_reordered(iargs, rebuild(stmts, last))
}

fn walk_reordered(iargs: &SymSet, gt: GenTerm) -> Result<GenTerm, TransformError> {
    let (term, bt) = gt.into_parts();
    Ok(match term {
        term @ (GenTermKind::Arbitrary | GenTermKind::Call { .. } | GenTermKind::Return(_)) => {
            GenTerm::from_parts(term, bt)
        }
        GenTermKind::WriteChoice { label, value, rest } => {
            GenTerm::write_choice(label, value, walk_reordered(iargs, *rest)?)
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => GenTerm::asgn(addr, ct, value, walk_reordered(iargs, *rest)?),
        GenTermKind::Assert { constraint, rest } => {
            GenTerm::assert(constraint, walk_reordered(iargs, *rest)?)
        }
        GenTermKind::LetStar { var, value, rest } => {
            let mut inner = iargs.clone();
            inner.insert(var.clone());
            GenTerm::let_star(
                (var, reorder(&inner, *value)?),
                walk_reordered(&inner, *rest)?,
            )
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            let then_ = reorder(iargs, *then_)?;
            let else_ = reorder(iargs, *else_)?;
            GenTerm::rebuild_ite(cond, then_, else_)
        }
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => {
            let mut inner = iargs.clone();
            inner.insert(var.clone());
            GenTerm::map((var, var_bt, perm), reorder(&inner, *body)?)
        }
        GenTermKind::Scope { name, body } => GenTerm::scope(name, reorder(iargs, *body)?),
    })
}
