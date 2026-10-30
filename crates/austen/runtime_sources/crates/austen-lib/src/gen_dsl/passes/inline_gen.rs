use crate::gen_dsl::lang::gen_def::GenDef;
use crate::gen_dsl::lang::gen_subst::fresh_same;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::index_term::IndexTerm;
use crate::gen_dsl::lang::subst::Substitution;
use crate::gen_dsl::lang::sym::{Sym, SymGen, SymMap, SymSet};
use crate::gen_dsl::passes::TransformError;
use crate::gen_dsl::passes::prune_call_graph::prune_call_graph;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InlineMode {
    #[default]
    Nothing,
    NonRecursive,
    SemiRecursive,
    Everything,
}

impl InlineMode {
    pub const SPELLINGS: &'static [(&'static str, InlineMode)] = &[
        ("nothing", InlineMode::Nothing),
        ("nonrec", InlineMode::NonRecursive),
        ("semirec", InlineMode::SemiRecursive),
        ("everything", InlineMode::Everything),
    ];

    pub fn from_token(token: &str) -> Option<InlineMode> {
        Self::SPELLINGS
            .iter()
            .find(|(name, _)| *name == token)
            .map(|(_, mode)| *mode)
    }

    pub fn as_str(self) -> &'static str {
        Self::SPELLINGS
            .iter()
            .find(|(_, mode)| *mode == self)
            .map(|(name, _)| *name)
            .expect("SPELLINGS carries every InlineMode")
    }
}

pub fn inline_gen(
    defs: &mut Vec<(Sym, GenDef)>,
    mode: InlineMode,
    syms: &mut SymGen,
) -> Result<(), TransformError> {
    match mode {
        InlineMode::Nothing => return Ok(()),
        InlineMode::NonRecursive => {
            inline_non_recursive(defs, syms)?;
        }
        InlineMode::SemiRecursive => {
            inline_non_recursive(defs, syms)?;
            inline_semi_recursive(defs, syms)?;
        }
        InlineMode::Everything => {
            inline_non_recursive(defs, syms)?;
            inline_recursive(defs, syms)?;
        }
    }
    prune_call_graph(defs);
    Ok(())
}

pub(in crate::gen_dsl) fn splice(gd: &GenDef, args: &[IndexTerm], syms: &mut SymGen) -> GenTerm {
    assert_eq!(
        gd.iargs.len(),
        args.len(),
        "call to {:?} has {} arguments for {} parameters",
        gd.name,
        args.len(),
        gd.iargs.len(),
    );
    let body = freshen_binders(gd.body.clone(), syms);
    let sub: Substitution = gd
        .iargs
        .iter()
        .map(|(formal, _)| formal.clone())
        .zip(args.iter().cloned())
        .collect();
    body.subst(&sub, syms)
}

fn freshen_binders(term: GenTerm, syms: &mut SymGen) -> GenTerm {
    let (term, bt) = term.into_parts();
    match term {
        GenTermKind::Arbitrary | GenTermKind::Call { .. } | GenTermKind::Return(_) => {
            GenTerm::from_parts(term, bt)
        }
        GenTermKind::WriteChoice { label, value, rest } => {
            GenTerm::write_choice(label, value, freshen_binders(*rest, syms))
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => GenTerm::asgn(addr, ct, value, freshen_binders(*rest, syms)),
        GenTermKind::Assert { constraint, rest } => {
            GenTerm::assert(constraint, freshen_binders(*rest, syms))
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            let then_ = freshen_binders(*then_, syms);
            let else_ = freshen_binders(*else_, syms);
            GenTerm::rebuild_ite(cond, then_, else_)
        }
        GenTermKind::Scope { name, body } => GenTerm::scope(name, freshen_binders(*body, syms)),
        GenTermKind::LetStar { var, value, rest } => {
            let value = freshen_binders(*value, syms);
            let fresh = fresh_same(syms, &var);
            let rename =
                Substitution::from([(var, IndexTerm::sym(fresh.clone(), value.bt.clone()))]);
            let rest = (*rest).subst(&rename, syms);
            GenTerm::let_star((fresh, value), freshen_binders(rest, syms))
        }
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => {
            let fresh = fresh_same(syms, &var);
            let rename = Substitution::from([(var, IndexTerm::sym(fresh.clone(), var_bt.clone()))]);
            let perm = perm.subst(&rename);
            let body = (*body).subst(&rename, syms);
            GenTerm::map((fresh, var_bt, perm), freshen_binders(body, syms))
        }
    }
}

enum Policy<'p> {
    NonRecursive,
    SemiRecursive(&'p SymSet),
    Everything(&'p SymSet),
}

impl Policy<'_> {
    fn inlines(&self, fsym: &Sym, lookup: &SymMap<GenDef>) -> Result<bool, TransformError> {
        match self {
            Policy::NonRecursive => match lookup.get(fsym) {
                None => Err(TransformError::UnknownCallee { fsym: fsym.clone() }),
                Some(gd) => Ok(!gd.recursive),
            },
            Policy::SemiRecursive(semi_rec) => Ok(semi_rec.contains(fsym)),
            Policy::Everything(dont_unfold) => {
                if dont_unfold.contains(fsym) {
                    Ok(false)
                } else if lookup.contains_key(fsym) {
                    Ok(true)
                } else {
                    Err(TransformError::UnknownCallee { fsym: fsym.clone() })
                }
            }
        }
    }
}

struct Inline<'a> {
    lookup: &'a SymMap<GenDef>,
    policy: Policy<'a>,
}

fn walk(
    cx: &Inline,
    stack: &mut Vec<Sym>,
    syms: &mut SymGen,
    term: GenTerm,
) -> Result<GenTerm, TransformError> {
    let (term, bt) = term.into_parts();
    Ok(match term {
        GenTermKind::Arbitrary | GenTermKind::Return(_) => GenTerm::from_parts(term, bt),
        GenTermKind::WriteChoice { label, value, rest } => {
            GenTerm::write_choice(label, value, walk(cx, stack, syms, *rest)?)
        }
        GenTermKind::Call { fsym, args } => {
            if !cx.policy.inlines(&fsym, cx.lookup)? {
                GenTerm::from_parts(GenTermKind::Call { fsym, args }, bt)
            } else {
                if stack.contains(&fsym) {
                    return Err(TransformError::InlineCycle { fsym });
                }
                let gd = cx
                    .lookup
                    .get(&fsym)
                    .expect("the policy inlines only defined callees");
                let body = splice(gd, &args, syms);
                stack.push(fsym);
                let out = walk(cx, stack, syms, body)?;
                stack.pop();
                out
            }
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => GenTerm::asgn(addr, ct, value, walk(cx, stack, syms, *rest)?),
        GenTermKind::LetStar { var, value, rest } => GenTerm::let_star(
            (var, walk(cx, stack, syms, *value)?),
            walk(cx, stack, syms, *rest)?,
        ),
        GenTermKind::Assert { constraint, rest } => {
            GenTerm::assert(constraint, walk(cx, stack, syms, *rest)?)
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            let then_ = walk(cx, stack, syms, *then_)?;
            let else_ = walk(cx, stack, syms, *else_)?;
            GenTerm::rebuild_ite(cond, then_, else_)
        }
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => GenTerm::map((var, var_bt, perm), walk(cx, stack, syms, *body)?),
        GenTermKind::Scope { name, body } => GenTerm::scope(name, walk(cx, stack, syms, *body)?),
    })
}

fn snapshot(defs: &[(Sym, GenDef)]) -> SymMap<GenDef> {
    defs.iter()
        .map(|(name, def)| (name.clone(), def.clone()))
        .collect()
}

fn inline_non_recursive(
    defs: &mut [(Sym, GenDef)],
    syms: &mut SymGen,
) -> Result<(), TransformError> {
    let lookup = snapshot(defs);
    let cx = Inline {
        lookup: &lookup,
        policy: Policy::NonRecursive,
    };
    for (_, def) in defs.iter_mut() {
        if !def.spec {
            continue;
        }
        def.body = walk(&cx, &mut Vec::new(), syms, def.body.clone())?;
    }
    Ok(())
}

fn semi_recursive_info(defs: &[(Sym, GenDef)]) -> (SymSet, SymSet) {
    let mut incoming: SymMap<Vec<(Sym, usize)>> = SymMap::new();
    for (name, def) in defs {
        for (callee, count) in call_counts(&def.body) {
            incoming
                .entry(callee)
                .or_default()
                .push((name.clone(), count));
        }
    }
    let mut semi_rec = SymSet::new();
    let mut preds = SymSet::new();
    for (name, def) in defs {
        if !def.recursive {
            continue;
        }
        if let Some([(pred, count)]) = incoming.get(name).map(Vec::as_slice)
            && pred != name
            && *count == 1
        {
            semi_rec.insert(name.clone());
            preds.insert(pred.clone());
        }
    }
    let targets = &preds - &semi_rec;
    (semi_rec, targets)
}

fn inline_semi_recursive(
    defs: &mut [(Sym, GenDef)],
    syms: &mut SymGen,
) -> Result<(), TransformError> {
    let (semi_rec, targets) = semi_recursive_info(defs);
    if targets.is_empty() {
        return Ok(());
    }
    let lookup = snapshot(defs);
    let cx = Inline {
        lookup: &lookup,
        policy: Policy::SemiRecursive(&semi_rec),
    };
    for (name, def) in defs.iter_mut() {
        if !targets.contains(name) {
            continue;
        }
        def.body = walk(&cx, &mut Vec::new(), syms, def.body.clone())?;
    }
    Ok(())
}

fn inline_recursive(defs: &mut [(Sym, GenDef)], syms: &mut SymGen) -> Result<(), TransformError> {
    let recursive_fsyms: SymSet = defs
        .iter()
        .filter(|(_, def)| def.recursive)
        .map(|(name, _)| name.clone())
        .collect();
    let spec_callees: SymSet = defs
        .iter()
        .filter(|(_, def)| def.spec)
        .flat_map(|(_, def)| call_counts(&def.body).into_keys())
        .collect();
    let lookup = snapshot(defs);
    for (name, def) in defs.iter_mut() {
        if !(def.spec || (def.recursive && spec_callees.contains(name))) {
            continue;
        }
        let dont_unfold: SymSet = if def.spec {
            recursive_fsyms
                .iter()
                .cloned()
                .chain([name.clone()])
                .collect()
        } else {
            SymSet::from([name.clone()])
        };
        let cx = Inline {
            lookup: &lookup,
            policy: Policy::Everything(&dont_unfold),
        };
        def.body = walk(&cx, &mut Vec::new(), syms, def.body.clone())?;
    }
    Ok(())
}

fn call_counts(term: &GenTerm) -> SymMap<usize> {
    fn go(term: &GenTerm, acc: &mut SymMap<usize>) {
        match &term.term {
            GenTermKind::Arbitrary | GenTermKind::Return(_) => {}
            GenTermKind::Call { fsym, .. } => *acc.entry(fsym.clone()).or_insert(0) += 1,
            GenTermKind::Asgn { rest, .. }
            | GenTermKind::WriteChoice { rest, .. }
            | GenTermKind::Assert { rest, .. } => go(rest, acc),
            GenTermKind::LetStar { value, rest, .. } => {
                go(value, acc);
                go(rest, acc);
            }
            GenTermKind::Ite { then_, else_, .. } => {
                go(then_, acc);
                go(else_, acc);
            }
            GenTermKind::Map { body, .. } | GenTermKind::Scope { body, .. } => go(body, acc),
        }
    }
    let mut acc = SymMap::new();
    go(term, &mut acc);
    acc
}
