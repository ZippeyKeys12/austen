use std::collections::BTreeSet;

use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::gen_def::GenDef;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind, ScopeName};
use crate::gen_dsl::lang::id::Id;
use crate::gen_dsl::lang::index_term::{IndexTerm, Term};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::subst::Substitution;
use crate::gen_dsl::lang::sym::{Sym, SymGen, SymMap, SymSet};
use crate::gen_dsl::passes::TransformError;
use crate::gen_dsl::passes::prune_returns::render;
use crate::gen_dsl::passes::reorder::{Stmt, rebuild, stmt_reads, stmts_of};
use crate::gen_dsl::passes::tail::map_tail;
use crate::gen_dsl::passes::{peel_scopes, wrap_scopes};

const RET: &str = "ret";

#[derive(Debug, Clone, PartialEq, Eq)]
struct SpecKey {
    callee: Sym,
    sunk: Vec<Vec<usize>>,
    pushed: Vec<LogicalConstraint>,
}

#[derive(Debug, Clone)]
struct Planned {
    key: SpecKey,
    name: Sym,
    oarg: BaseType,
}

struct Planner {
    originals: Vec<(Sym, GenDef)>,
    planned: Vec<Planned>,
}

struct Site {
    at: usize,
    scopes: Vec<ScopeName>,
    fsym: Sym,
    args: Vec<IndexTerm>,
    groups: Vec<(Sym, usize, Vec<usize>)>,
    pushed: Vec<usize>,
    moved: Vec<usize>,
    first_pos: SymMap<usize>,
}

fn call_mentions(gt: &GenTerm, out: &mut SymMap<usize>) {
    match &gt.term {
        GenTermKind::Arbitrary | GenTermKind::Return(_) => {}
        GenTermKind::Call { args, .. } => {
            let mut seen = SymSet::new();
            for arg in args {
                seen.extend(arg.free_vars());
            }
            for sym in seen {
                *out.entry(sym).or_insert(0) += 1;
            }
        }
        GenTermKind::Asgn { rest, .. }
        | GenTermKind::WriteChoice { rest, .. }
        | GenTermKind::Assert { rest, .. } => call_mentions(rest, out),
        GenTermKind::LetStar { value, rest, .. } => {
            call_mentions(value, out);
            call_mentions(rest, out);
        }
        GenTermKind::Ite { then_, else_, .. } => {
            call_mentions(then_, out);
            call_mentions(else_, out);
        }
        GenTermKind::Map { body, .. } | GenTermKind::Scope { body, .. } => call_mentions(body, out),
    }
}

fn mentions_of(gt: &GenTerm) -> SymMap<usize> {
    let mut out = SymMap::new();
    call_mentions(gt, &mut out);
    out
}

fn returns_value(callee: &GenDef) -> bool {
    callee.oarg != BaseType::Unit
}

fn record_members(callee: &GenDef, sunk: &[Vec<usize>]) -> Vec<(Id, BaseType)> {
    let mut members: Vec<(Id, BaseType)> = sunk
        .iter()
        .map(|group| {
            let (formal, bt) = &callee.iargs[group[0]];
            (Id(render(formal)), bt.clone())
        })
        .collect();
    if returns_value(callee) {
        members.push((Id(RET.into()), callee.oarg.clone()));
    }
    members
}

fn member(base: &Sym, base_bt: &BaseType, id: &Id, bt: BaseType) -> IndexTerm {
    IndexTerm {
        term: Term::RecordMember(
            Box::new(IndexTerm::sym(base.clone(), base_bt.clone())),
            id.clone(),
        ),
        bt,
    }
}

fn placeholder() -> GenTerm {
    GenTerm::return_(IndexTerm::unit())
}

impl Planner {
    fn original(&self, name: &Sym) -> Option<&GenDef> {
        self.originals
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, def)| def)
    }

    fn plan(&mut self, key: SpecKey, callee: &GenDef, syms: &mut SymGen) -> (Sym, BaseType) {
        if let Some(planned) = self.planned.iter().find(|planned| planned.key == key) {
            return (planned.name.clone(), planned.oarg.clone());
        }
        let params: Vec<String> = key
            .sunk
            .iter()
            .map(|group| render(&callee.iargs[group[0]].0))
            .collect();
        let name = syms.fresh(&format!("{}_{}", render(&callee.name), params.join("_")));
        let oarg = BaseType::Record(record_members(callee, &key.sunk));
        self.planned.push(Planned {
            key,
            name: name.clone(),
            oarg: oarg.clone(),
        });
        (name, oarg)
    }

    fn analyze(&self, stmts: &[Stmt], at: usize, mentions: &SymMap<usize>) -> Option<Site> {
        let Stmt::LetStar { value, .. } = &stmts[at] else {
            return None;
        };
        let (scopes, inner) = peel_scopes(value.clone());
        let GenTermKind::Call { fsym, args } = inner.term else {
            return None;
        };
        let callee = self.original(&fsym)?;
        if callee.spec || callee.c_types.is_some() || callee.iargs.len() != args.len() {
            return None;
        }

        let mut first_pos: SymMap<usize> = SymMap::new();
        for (pos, arg) in args.iter().enumerate() {
            if let Some(sym) = arg.as_sym() {
                first_pos.entry(sym.clone()).or_insert(pos);
            }
        }

        let mut groups: Vec<(Sym, usize, Vec<usize>)> = Vec::new();
        for (pos, arg) in args.iter().enumerate() {
            let Some(sym) = arg.as_sym() else { continue };
            if let Some(group) = groups.iter_mut().find(|(s, _, _)| s == sym) {
                group.2.push(pos);
                continue;
            }
            let binder = (0..at).rev().find(|&j| {
                matches!(&stmts[j], Stmt::LetStar { var, value }
                    if var == sym && matches!(value.term, GenTermKind::Arbitrary))
            });
            let Some(binder) = binder else { continue };
            let whole = args
                .iter()
                .all(|a| !a.free_vars().contains(sym) || a.as_sym() == Some(sym));
            if whole && mentions.get(sym) == Some(&1) {
                groups.push((sym.clone(), binder, vec![pos]));
            }
        }

        loop {
            let candidates: SymSet = groups.iter().map(|(s, _, _)| s.clone()).collect();
            let allowed: SymSet = candidates.iter().chain(first_pos.keys()).cloned().collect();
            let before = groups.len();
            groups.retain(|(sym, binder, _)| {
                stmts[binder + 1..at].iter().all(|stmt| {
                    let reads = stmt_reads(stmt);
                    if !reads.contains(sym) {
                        return true;
                    }
                    match stmt {
                        Stmt::Assert(_) => reads.is_subset(&allowed),
                        Stmt::WriteChoice { .. } => true,
                        Stmt::LetStar { .. } | Stmt::Asgn { .. } => false,
                    }
                })
            });
            if groups.len() == before {
                break;
            }
        }
        if groups.is_empty() {
            return None;
        }

        let candidates: SymSet = groups.iter().map(|(s, _, _)| s.clone()).collect();
        let earliest = groups.iter().map(|(_, binder, _)| *binder).min()?;
        let mut pushed = Vec::new();
        let mut moved = Vec::new();
        for (k, stmt) in stmts.iter().enumerate().take(at).skip(earliest + 1) {
            if stmt_reads(stmt).is_disjoint(&candidates) {
                continue;
            }
            match stmt {
                Stmt::Assert(_) => pushed.push(k),
                Stmt::WriteChoice { .. } => moved.push(k),
                Stmt::LetStar { .. } | Stmt::Asgn { .. } => {}
            }
        }

        let sunk: Vec<Vec<usize>> = groups.iter().map(|(_, _, ps)| ps.clone()).collect();
        let ids: BTreeSet<Id> = record_members(callee, &sunk)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        if ids.len() != sunk.len() + usize::from(returns_value(callee)) {
            return None;
        }

        Some(Site {
            at,
            scopes,
            fsym,
            args,
            groups,
            pushed,
            moved,
            first_pos,
        })
    }

    fn rewrite_site(&mut self, stmts: Vec<Stmt>, site: Site, syms: &mut SymGen) -> Vec<Stmt> {
        let callee = self
            .original(&site.fsym)
            .expect("analyzed against the original table")
            .clone();
        let Stmt::LetStar { var, .. } = &stmts[site.at] else {
            unreachable!("analyzed as a let*-of-call")
        };
        let var = var.clone();

        let mut sub = Substitution::new();
        for (sym, _, positions) in &site.groups {
            let (formal, bt) = &callee.iargs[positions[0]];
            sub.insert(sym.clone(), IndexTerm::sym(formal.clone(), bt.clone()));
        }
        for (sym, pos) in &site.first_pos {
            let (formal, bt) = &callee.iargs[*pos];
            sub.entry(sym.clone())
                .or_insert_with(|| IndexTerm::sym(formal.clone(), bt.clone()));
        }
        let pushed: Vec<LogicalConstraint> = site
            .pushed
            .iter()
            .map(|&k| match &stmts[k] {
                Stmt::Assert(lc) => lc.subst(&sub),
                _ => unreachable!("pushed indices name asserts"),
            })
            .collect();
        let sunk: Vec<Vec<usize>> = site.groups.iter().map(|(_, _, ps)| ps.clone()).collect();
        let key = SpecKey {
            callee: site.fsym.clone(),
            sunk: sunk.clone(),
            pushed,
        };
        let (name, oarg) = self.plan(key, &callee, syms);

        let sunk_positions: BTreeSet<usize> = sunk.iter().flatten().copied().collect();
        let remaining: Vec<IndexTerm> = site
            .args
            .into_iter()
            .enumerate()
            .filter(|(pos, _)| !sunk_positions.contains(pos))
            .map(|(_, arg)| arg)
            .collect();
        let params: Vec<String> = sunk
            .iter()
            .map(|group| render(&callee.iargs[group[0]].0))
            .collect();
        let with = syms.fresh(&format!("{}_with_{}", render(&var), params.join("_")));
        let members = record_members(&callee, &sunk);

        let binders: BTreeSet<usize> = site.groups.iter().map(|(_, b, _)| *b).collect();
        let pushed_at: BTreeSet<usize> = site.pushed.iter().copied().collect();
        let moved_at: BTreeSet<usize> = site.moved.iter().copied().collect();
        let mut out = Vec::with_capacity(stmts.len() + sunk.len() + 2);
        let mut moved = Vec::with_capacity(moved_at.len());
        for (k, stmt) in stmts.into_iter().enumerate() {
            if k < site.at {
                if binders.contains(&k) || pushed_at.contains(&k) {
                    continue;
                }
                if moved_at.contains(&k) {
                    moved.push(stmt);
                    continue;
                }
                out.push(stmt);
            } else if k == site.at {
                let call = GenTerm::call(name.clone(), remaining.clone(), oarg.clone());
                out.push(Stmt::LetStar {
                    var: with.clone(),
                    value: wrap_scopes(site.scopes.clone(), call),
                });
                for ((sym, _, _), (id, bt)) in site.groups.iter().zip(&members) {
                    out.push(Stmt::LetStar {
                        var: sym.clone(),
                        value: GenTerm::return_(member(&with, &oarg, id, bt.clone())),
                    });
                }
                out.append(&mut moved);
                let value = if returns_value(&callee) {
                    member(&with, &oarg, &Id(RET.into()), callee.oarg.clone())
                } else {
                    IndexTerm::unit()
                };
                out.push(Stmt::LetStar {
                    var: var.clone(),
                    value: GenTerm::return_(value),
                });
            } else {
                out.push(stmt);
            }
        }
        out
    }

    fn rewrite_term(
        &mut self,
        gt: GenTerm,
        mentions: &SymMap<usize>,
        syms: &mut SymGen,
    ) -> GenTerm {
        let (mut stmts, last) = stmts_of(gt);
        loop {
            let site = (0..stmts.len()).find_map(|at| self.analyze(&stmts, at, mentions));
            let Some(site) = site else { break };
            stmts = self.rewrite_site(stmts, site, syms);
        }
        let stmts = stmts
            .into_iter()
            .map(|stmt| match stmt {
                Stmt::LetStar { var, value } => Stmt::LetStar {
                    var,
                    value: self.rewrite_term(value, mentions, syms),
                },
                other => other,
            })
            .collect();
        let (term, bt) = last.into_parts();
        let last = match term {
            GenTermKind::Ite { cond, then_, else_ } => GenTerm::rebuild_ite(
                cond,
                self.rewrite_term(*then_, mentions, syms),
                self.rewrite_term(*else_, mentions, syms),
            ),
            GenTermKind::Map {
                var,
                var_bt,
                perm,
                body,
            } => GenTerm::map(
                (var, var_bt, perm),
                self.rewrite_term(*body, mentions, syms),
            ),
            GenTermKind::Scope { name, body } => {
                GenTerm::scope(name, self.rewrite_term(*body, mentions, syms))
            }
            term @ (GenTermKind::Arbitrary | GenTermKind::Call { .. } | GenTermKind::Return(_)) => {
                GenTerm::from_parts(term, bt)
            }
            GenTermKind::WriteChoice { .. }
            | GenTermKind::Asgn { .. }
            | GenTermKind::LetStar { .. }
            | GenTermKind::Assert { .. } => unreachable!("stmts_of opens every spine statement"),
        };
        rebuild(stmts, last)
    }

    fn build(&self, planned: &Planned, syms: &mut SymGen) -> Result<GenDef, TransformError> {
        let callee = self
            .original(&planned.key.callee)
            .expect("requests name original definitions")
            .clone();
        let key = &planned.key;
        let sunk_positions: BTreeSet<usize> = key.sunk.iter().flatten().copied().collect();
        let iargs: Vec<(Sym, BaseType)> = callee
            .iargs
            .iter()
            .enumerate()
            .filter(|(pos, _)| !sunk_positions.contains(pos))
            .map(|(_, arg)| arg.clone())
            .collect();

        let mut prefix = Vec::new();
        for group in &key.sunk {
            let (first, bt) = callee.iargs[group[0]].clone();
            prefix.push(Stmt::LetStar {
                var: first.clone(),
                value: GenTerm::arbitrary(bt.clone()),
            });
            for &pos in &group[1..] {
                prefix.push(Stmt::LetStar {
                    var: callee.iargs[pos].0.clone(),
                    value: GenTerm::return_(IndexTerm::sym(first.clone(), bt.clone())),
                });
            }
        }
        prefix.extend(key.pushed.iter().cloned().map(Stmt::Assert));

        let members = record_members(&callee, &key.sunk);
        let record_of = |ret: IndexTerm| -> IndexTerm {
            let mut fields: Vec<(Id, IndexTerm)> = key
                .sunk
                .iter()
                .zip(&members)
                .map(|(group, (id, bt))| {
                    let (first, _) = &callee.iargs[group[0]];
                    (id.clone(), IndexTerm::sym(first.clone(), bt.clone()))
                })
                .collect();
            if returns_value(&callee) {
                fields.push((Id(RET.into()), ret));
            }
            IndexTerm::record(fields).expect("member ids were checked when the site was planned")
        };
        let body = map_tail(callee.body.clone(), &mut |tail| {
            let (term, bt) = tail.into_parts();
            Ok(match term {
                GenTermKind::Return(it) => GenTerm::return_(record_of(it)),
                term => {
                    let bound = syms.fresh(RET);
                    GenTerm::let_star(
                        (bound.clone(), GenTerm::from_parts(term, bt.clone())),
                        GenTerm::return_(record_of(IndexTerm::sym(bound, bt))),
                    )
                }
            })
        })?;

        Ok(GenDef {
            name: planned.name.clone(),
            iargs,
            oarg: planned.oarg.clone(),
            c_types: None,
            spec: false,
            body: rebuild(prefix, body),
            ..callee
        })
    }
}

pub fn sink_args(defs: &mut Vec<(Sym, GenDef)>, syms: &mut SymGen) -> Result<(), TransformError> {
    let mut planner = Planner {
        originals: defs.clone(),
        planned: Vec::new(),
    };
    for (_, def) in defs.iter_mut() {
        let mentions = mentions_of(&def.body);
        let body = std::mem::replace(&mut def.body, placeholder());
        def.body = planner.rewrite_term(body, &mentions, syms);
    }
    let mut next = 0;
    while next < planner.planned.len() {
        let planned = planner.planned[next].clone();
        let mut def = planner.build(&planned, syms)?;
        let mentions = mentions_of(&def.body);
        let body = std::mem::replace(&mut def.body, placeholder());
        def.body = planner.rewrite_term(body, &mentions, syms);
        defs.push((def.name.clone(), def));
        next += 1;
    }
    Ok(())
}
