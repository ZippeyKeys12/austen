use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::index_term::{BinOp, IndexTerm, Term};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::subst::Substitution;
use crate::gen_dsl::lang::sym::{Sym, SymGen, SymSet};
use crate::gen_dsl::passes::{TransformError, peel_scopes, term_bounds, wrap_scopes};

pub fn each_fusion(
    iargs: &SymSet,
    term: GenTerm,
    syms: &mut SymGen,
) -> Result<GenTerm, TransformError> {
    fuse(iargs, term, syms)
}

fn fuse(vars: &SymSet, gt: GenTerm, syms: &mut SymGen) -> Result<GenTerm, TransformError> {
    let (term, bt) = gt.into_parts();
    Ok(match term {
        term @ (GenTermKind::Arbitrary | GenTermKind::Call { .. } | GenTermKind::Return(_)) => {
            GenTerm::from_parts(term, bt)
        }
        GenTermKind::WriteChoice { label, value, rest } => {
            GenTerm::write_choice(label, value, fuse(vars, *rest, syms)?)
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => GenTerm::asgn(addr, ct, value, fuse(vars, *rest, syms)?),
        GenTermKind::LetStar { var, value, rest } => {
            let (scopes, value) = peel_scopes(*value);
            match value {
                GenTerm {
                    term:
                        GenTermKind::Map {
                            var: index,
                            var_bt: index_bt,
                            perm,
                            body,
                        },
                    ..
                } => {
                    let bounds = term_bounds::bounds(&index, &index_bt, &perm)?;
                    let body = {
                        let mut inner = vars.clone();
                        inner.insert(index.clone());
                        fuse(&inner, *body, syms)?
                    };
                    let mut rest_vars = vars.clone();
                    rest_vars.insert(var.clone());
                    let (rest, constraints) = Collect {
                        vars: &rest_vars,
                        x: &var,
                        map_index: &index,
                        map_permission: &perm,
                        map_bounds: &bounds,
                    }
                    .collect(true, *rest)?;
                    let rest = fuse(&rest_vars, rest, syms)?;
                    let body = if is_trivially_true(&constraints) {
                        body
                    } else {
                        let result = syms.fresh("fused_elem");
                        let elem_bt = body.bt.clone();
                        let per_element = replace_index(&var, &index, &result, constraints);
                        GenTerm::let_star(
                            (result.clone(), body),
                            GenTerm::assert(
                                LogicalConstraint::T(per_element),
                                GenTerm::return_(IndexTerm::sym(result, elem_bt)),
                            ),
                        )
                    };
                    let map = GenTerm::map((index, index_bt, perm), body);
                    GenTerm::let_star((var, wrap_scopes(scopes, map)), rest)
                }
                value => {
                    let value = wrap_scopes(scopes, fuse(vars, value, syms)?);
                    let mut inner = vars.clone();
                    inner.insert(var.clone());
                    GenTerm::let_star((var, value), fuse(&inner, *rest, syms)?)
                }
            }
        }
        GenTermKind::Assert { constraint, rest } => {
            GenTerm::assert(constraint, fuse(vars, *rest, syms)?)
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            let then_ = fuse(vars, *then_, syms)?;
            let else_ = fuse(vars, *else_, syms)?;
            GenTerm::rebuild_ite(cond, then_, else_)
        }
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => {
            let mut inner = vars.clone();
            inner.insert(var.clone());
            GenTerm::map((var, var_bt, perm), fuse(&inner, *body, syms)?)
        }
        GenTermKind::Scope { name, body } => GenTerm::scope(name, fuse(vars, *body, syms)?),
    })
}

struct Candidate {
    index: Sym,
    index_bt: BaseType,
    permission: IndexTerm,
    body: IndexTerm,
    free: SymSet,
}

#[derive(Clone, Copy)]
struct Collect<'a> {
    vars: &'a SymSet,
    x: &'a Sym,
    map_index: &'a Sym,
    map_permission: &'a IndexTerm,
    map_bounds: &'a (IndexTerm, IndexTerm),
}

impl Collect<'_> {
    fn collect(self, delete: bool, gt: GenTerm) -> Result<(GenTerm, IndexTerm), TransformError> {
        let (term, bt) = gt.into_parts();
        Ok(match term {
            term @ (GenTermKind::Arbitrary
            | GenTermKind::Call { .. }
            | GenTermKind::Return(_)
            | GenTermKind::Map { .. }
            | GenTermKind::Scope { .. }) => (GenTerm::from_parts(term, bt), tt()),
            GenTermKind::WriteChoice { label, value, rest } => {
                let (rest, constraint) = self.collect(delete, *rest)?;
                (GenTerm::write_choice(label, value, rest), constraint)
            }
            GenTermKind::Asgn {
                addr,
                ct,
                value,
                rest,
            } => {
                let (rest, constraint) = self.collect(delete, *rest)?;
                (GenTerm::asgn(addr, ct, value, rest), constraint)
            }
            GenTermKind::LetStar { var, value, rest } if var == *self.x => (
                GenTerm::from_parts(GenTermKind::LetStar { var, value, rest }, bt),
                tt(),
            ),
            GenTermKind::LetStar { var, value, rest } => {
                let (value, c_value) = self.collect(delete, *value)?;
                let (rest, c_rest) = self.collect(delete, *rest)?;
                (
                    GenTerm::let_star((var, value), rest),
                    IndexTerm::and2(c_value, c_rest),
                )
            }
            GenTermKind::Assert { constraint, rest } => {
                let candidate = match &constraint {
                    LogicalConstraint::Forall {
                        var: (index, index_bt),
                        permission,
                        body,
                    } => {
                        let mut free = permission.free_vars();
                        free.extend(body.free_vars());
                        (free.contains(self.x)
                            && check_index_ok(self.x, index, permission)
                            && check_index_ok(self.x, index, body))
                        .then(|| Candidate {
                            index: index.clone(),
                            index_bt: index_bt.clone(),
                            permission: permission.clone(),
                            body: body.clone(),
                            free,
                        })
                    }
                    LogicalConstraint::T(_) => None,
                };
                let (rest, inherited) = self.collect(delete, *rest)?;
                let Some(c) = candidate else {
                    return Ok((GenTerm::assert(constraint, rest), inherited));
                };
                let forall_bounds = term_bounds::bounds(&c.index, &c.index_bt, &c.permission)?;
                let mut free_beyond_index = c.free;
                free_beyond_index.remove(&c.index);
                if forall_bounds == *self.map_bounds && free_beyond_index.is_subset(self.vars) {
                    let subst = Substitution::from([(
                        c.index,
                        IndexTerm::sym(self.map_index.clone(), c.index_bt),
                    )]);
                    let permission = c.permission.subst(&subst);
                    let body = c.body.subst(&subst);
                    let renamed = if permission == *self.map_permission {
                        body
                    } else {
                        implies(permission, body)
                    };
                    let hoisted = IndexTerm::and2(renamed, inherited);
                    if delete {
                        (rest, hoisted)
                    } else {
                        (GenTerm::assert(constraint, rest), hoisted)
                    }
                } else {
                    (GenTerm::assert(constraint, rest), inherited)
                }
            }
            GenTermKind::Ite { cond, then_, else_ } => {
                let delete_inner = cond.free_vars().is_subset(self.vars);
                let (then_, then_c) = self.collect(delete_inner, *then_)?;
                let (else_, else_c) = self.collect(delete_inner, *else_)?;
                let node = GenTerm::rebuild_ite(cond.clone(), then_, else_);
                let hoisted = match (is_trivially_true(&then_c), is_trivially_true(&else_c)) {
                    (false, true) if delete_inner => implies(cond, then_c),
                    (true, false) if delete_inner => implies(IndexTerm::not_(cond), else_c),
                    (false, false) if delete_inner => ite(cond, then_c, else_c),
                    _ => or2(then_c, else_c),
                };
                (node, hoisted)
            }
        })
    }
}

fn check_index_ok(m: &Sym, i: &Sym, it: &IndexTerm) -> bool {
    if let Term::MapGet(map, key) = &it.term
        && let Term::Sym(y) = &map.term
        && y == m
    {
        return matches!(&key.term, Term::Sym(j) if j == i);
    }
    match &it.term {
        Term::Const(_)
        | Term::SizeOf(_)
        | Term::OffsetOf(_, _)
        | Term::Nil(_)
        | Term::CnNone(_) => true,
        Term::Sym(y) => y != m,
        Term::Unop(_, t)
        | Term::NthTuple(_, t)
        | Term::StructMember(t, _)
        | Term::RecordMember(t, _)
        | Term::Cast(_, t)
        | Term::MemberShift(t, _, _)
        | Term::HasAllocId(t)
        | Term::Head(t)
        | Term::Tail(t)
        | Term::Representable(_, t)
        | Term::Good(_, t)
        | Term::WrapI(_, t)
        | Term::MapConst(_, t)
        | Term::CnSome(t)
        | Term::IsSome(t)
        | Term::GetOpt(t) => check_index_ok(m, i, t),
        Term::EachI { body, .. } | Term::MapDef(_, body) => check_index_ok(m, i, body),
        Term::Binop(_, a, b) | Term::Cons(a, b) | Term::MapGet(a, b) => {
            check_index_ok(m, i, a) && check_index_ok(m, i, b)
        }
        Term::StructUpdate { target, value, .. } | Term::RecordUpdate { target, value, .. } => {
            check_index_ok(m, i, target) && check_index_ok(m, i, value)
        }
        Term::ArrayShift { base, index, .. } => {
            check_index_ok(m, i, base) && check_index_ok(m, i, index)
        }
        Term::CopyAllocId { addr, loc } => check_index_ok(m, i, addr) && check_index_ok(m, i, loc),
        Term::Aligned { t, align } => check_index_ok(m, i, t) && check_index_ok(m, i, align),
        Term::Let((_, bound), body) => check_index_ok(m, i, bound) && check_index_ok(m, i, body),
        Term::Ite(a, b, c) | Term::MapSet(a, b, c) => {
            check_index_ok(m, i, a) && check_index_ok(m, i, b) && check_index_ok(m, i, c)
        }
        Term::Tuple(ts) | Term::Apply(_, ts) => ts.iter().all(|t| check_index_ok(m, i, t)),
        Term::Struct(_, members) | Term::Record(members) | Term::Constructor(_, members) => {
            members.iter().all(|(_, t)| check_index_ok(m, i, t))
        }
        Term::Match(scrutinee, arms) => {
            check_index_ok(m, i, scrutinee) && arms.iter().all(|(_, t)| check_index_ok(m, i, t))
        }
    }
}

fn replace_index(m: &Sym, i: &Sym, result: &Sym, it: IndexTerm) -> IndexTerm {
    it.map_post(&mut |node| {
        let (term, bt) = node.into_parts();
        match term {
            Term::MapGet(map, key) if matches!(&map.term, Term::Sym(y) if y == m) => {
                debug_assert!(
                    matches!(&key.term, Term::Sym(j) if j == i),
                    "a map read at a non-index key survived collection: {key:?}"
                );
                IndexTerm::sym(result.clone(), bt)
            }
            term => IndexTerm { term, bt },
        }
    })
}

fn is_trivially_true(it: &IndexTerm) -> bool {
    if it.is_true() {
        return true;
    }
    match &it.term {
        Term::Binop(BinOp::And, a, b) => is_trivially_true(a) && is_trivially_true(b),
        Term::Binop(BinOp::Or, a, b) => is_trivially_true(a) || is_trivially_true(b),
        Term::Binop(BinOp::Implies, _, b) => is_trivially_true(b),
        Term::Ite(_, t, e) => is_trivially_true(t) && is_trivially_true(e),
        _ => false,
    }
}

fn tt() -> IndexTerm {
    IndexTerm::boolean(true)
}

fn or2(l: IndexTerm, r: IndexTerm) -> IndexTerm {
    IndexTerm {
        term: Term::Binop(BinOp::Or, Box::new(l), Box::new(r)),
        bt: BaseType::Bool,
    }
}

fn implies(l: IndexTerm, r: IndexTerm) -> IndexTerm {
    IndexTerm {
        term: Term::Binop(BinOp::Implies, Box::new(l), Box::new(r)),
        bt: BaseType::Bool,
    }
}

fn ite(cond: IndexTerm, then_: IndexTerm, else_: IndexTerm) -> IndexTerm {
    let bt = then_.bt.clone();
    IndexTerm {
        term: Term::Ite(Box::new(cond), Box::new(then_), Box::new(else_)),
        bt,
    }
}
