use num_bigint::BigInt;

use crate::gen_dsl::lang::base_type::{BaseType, Sign, bits_range};
use crate::gen_dsl::lang::eval::declared_constructor;
use crate::gen_dsl::lang::gen_module::GenModule;
use crate::gen_dsl::lang::gen_term::GenTerm;
use crate::gen_dsl::lang::id::Id;
use crate::gen_dsl::lang::index_term::{BinOp, Const, IndexTerm, Pattern, PatternKind, Term, UnOp};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::logical_function::{FunctionBody, binders, unfoldable_defs};
use crate::gen_dsl::lang::subst::Substitution;
use crate::gen_dsl::lang::sym::SymSet;
use crate::gen_dsl::passes::partial_evaluation::{fold_ground, rewrite_embedded};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoodHandling {
    Keep,
    Drop,
}

const MAX_INLINE_DEPTH: usize = 128;

pub fn simplify_index_term(term: GenTerm, ctx: &GenModule, good: GoodHandling) -> GenTerm {
    let simplifier = Simplifier {
        ctx,
        unfoldable: unfoldable_defs(&ctx.functions),
        good,
    };
    rewrite_embedded(term, &mut |it| simplifier.simp_it(it), &mut |lc| {
        simplifier.simp_lc(lc)
    })
}

struct Simplifier<'a> {
    ctx: &'a GenModule,
    unfoldable: SymSet,
    good: GoodHandling,
}

impl Simplifier<'_> {
    fn simp_it(&self, it: IndexTerm) -> IndexTerm {
        self.fixes(self.builtin(it, 0))
    }

    fn builtin(&self, it: IndexTerm, depth: usize) -> IndexTerm {
        it.map_post(&mut |node| self.builtin_node(node, depth))
    }

    fn builtin_node(&self, node: IndexTerm, depth: usize) -> IndexTerm {
        if self.good == GoodHandling::Drop && matches!(node.term, Term::Good(_, _)) {
            return IndexTerm::boolean(true);
        }
        if let Some(folded) = fold_ground(&node, self.ctx) {
            return folded;
        }
        if let Some(inlined) = self.inline(&node, depth) {
            return inlined;
        }
        if let Some(opened) = self.open_match(&node, depth) {
            return opened;
        }
        if let Some(projected) = self.open_product(&node, depth) {
            return projected;
        }
        if let Some(compared) = self.open_constructor_inequality(&node) {
            return compared;
        }
        identities(&node).unwrap_or(node)
    }

    fn inline(&self, node: &IndexTerm, depth: usize) -> Option<IndexTerm> {
        let Term::Apply(fsym, args) = &node.term else {
            return None;
        };
        if depth >= MAX_INLINE_DEPTH || !self.unfoldable.contains(fsym) {
            return None;
        }
        let def = self.ctx.find_function(fsym)?;
        let Some(FunctionBody::Def(body)) = &def.body else {
            return None;
        };
        if def.args.len() != args.len() {
            return None;
        }
        if would_capture(&binders(body), args) {
            return None;
        }
        let sub: Substitution = def
            .args
            .iter()
            .zip(args)
            .map(|((param, _), arg)| (param.clone(), arg.clone()))
            .collect();
        Some(self.builtin(body.subst(&sub), depth + 1))
    }

    fn open_match(&self, node: &IndexTerm, depth: usize) -> Option<IndexTerm> {
        let Term::Match(scrutinee, arms) = &node.term else {
            return None;
        };
        if !valid_constructor(self.ctx, scrutinee) {
            return None;
        }
        for (pattern, body) in arms {
            let mut sub = Substitution::new();
            match bind_open_pattern(self.ctx, pattern, scrutinee, &mut sub) {
                OpenPattern::Mismatch => continue,
                OpenPattern::Unknown => return None,
                OpenPattern::Matched => {
                    if would_capture(&binders(body), sub.values()) {
                        return None;
                    }
                    return Some(self.builtin(body.subst(&sub), depth));
                }
            }
        }
        None
    }

    fn open_constructor_inequality(&self, node: &IndexTerm) -> Option<IndexTerm> {
        let Term::Binop(BinOp::Eq, left, right) = &node.term else {
            return None;
        };
        if left.bt != right.bt || !matches!(left.bt, BaseType::Datatype(_)) {
            return None;
        }
        let (Term::Constructor(left_name, _), Term::Constructor(right_name, _)) =
            (&left.term, &right.term)
        else {
            return None;
        };
        (left_name != right_name
            && valid_constructor(self.ctx, left)
            && valid_constructor(self.ctx, right))
        .then(|| IndexTerm::boolean(false))
    }

    fn open_product(&self, node: &IndexTerm, depth: usize) -> Option<IndexTerm> {
        let bt = &node.bt;
        let projected = match &node.term {
            Term::NthTuple(index, tuple) => match &tuple.term {
                Term::Ite(condition, then_, else_) => {
                    Some(project_ite(bt, condition, then_, else_, |branch| {
                        Term::NthTuple(*index, branch)
                    }))
                }
                _ => None,
            },
            Term::StructMember(base, member) => match &base.term {
                Term::Ite(condition, then_, else_) => {
                    Some(project_ite(bt, condition, then_, else_, |branch| {
                        Term::StructMember(branch, member.clone())
                    }))
                }
                _ => None,
            },
            Term::RecordMember(base, member) => match &base.term {
                Term::RecordUpdate {
                    target: _,
                    member: updated,
                    value,
                } if updated == member => Some((**value).clone()),
                Term::RecordUpdate { target, .. } => Some(IndexTerm {
                    term: Term::RecordMember(Box::new((**target).clone()), member.clone()),
                    bt: bt.clone(),
                }),
                Term::Ite(condition, then_, else_) => {
                    Some(project_ite(bt, condition, then_, else_, |branch| {
                        Term::RecordMember(branch, member.clone())
                    }))
                }
                _ => None,
            },
            Term::StructUpdate {
                target,
                member,
                value,
            } => match &target.term {
                Term::Struct(tag, fields) if fields.iter().any(|(id, _)| id == member) => {
                    Some(IndexTerm {
                        term: Term::Struct(tag.clone(), replace_field(fields, member, value)),
                        bt: bt.clone(),
                    })
                }
                _ => None,
            },
            Term::RecordUpdate {
                target,
                member,
                value,
            } => match &target.term {
                Term::Record(fields) if fields.iter().any(|(id, _)| id == member) => {
                    Some(IndexTerm {
                        term: Term::Record(replace_field(fields, member, value)),
                        bt: bt.clone(),
                    })
                }
                _ => None,
            },
            _ => None,
        }?;
        Some(self.builtin(projected, depth))
    }

    fn fixes(&self, it: IndexTerm) -> IndexTerm {
        it.map_post(&mut |node| fixes_rules(&node).unwrap_or(node))
    }

    fn simp_lc(&self, lc: LogicalConstraint) -> LogicalConstraint {
        self.fixes_lc(self.builtin_lc(lc))
    }

    fn builtin_lc(&self, lc: LogicalConstraint) -> LogicalConstraint {
        match lc {
            LogicalConstraint::T(it) => LogicalConstraint::T(self.builtin(it, 0)),
            LogicalConstraint::Forall {
                var,
                permission,
                body,
            } => {
                let permission = self.builtin(permission, 0);
                let body = self.builtin(body, 0);
                if bool_const(&body) == Some(true) {
                    LogicalConstraint::T(IndexTerm::boolean(true))
                } else {
                    LogicalConstraint::Forall {
                        var,
                        permission,
                        body,
                    }
                }
            }
        }
    }

    fn fixes_lc(&self, lc: LogicalConstraint) -> LogicalConstraint {
        match lc {
            LogicalConstraint::T(it) => LogicalConstraint::T(self.fixes(it)),
            LogicalConstraint::Forall {
                var,
                permission,
                body,
            } => {
                let permission = self.fixes(permission);
                let body = self.fixes(body);
                if bool_const(&permission) == Some(false) || bool_const(&body) == Some(true) {
                    LogicalConstraint::T(IndexTerm::boolean(true))
                } else {
                    LogicalConstraint::Forall {
                        var,
                        permission,
                        body,
                    }
                }
            }
        }
    }
}

fn would_capture<'a>(
    binders: &SymSet,
    replacements: impl IntoIterator<Item = &'a IndexTerm>,
) -> bool {
    !binders.is_empty()
        && replacements
            .into_iter()
            .any(|term| term.free_vars().iter().any(|v| binders.contains(v)))
}

fn project_ite(
    bt: &BaseType,
    condition: &IndexTerm,
    then_: &IndexTerm,
    else_: &IndexTerm,
    wrap: impl Fn(Box<IndexTerm>) -> Term,
) -> IndexTerm {
    let branch = |b: &IndexTerm| IndexTerm {
        term: wrap(Box::new(b.clone())),
        bt: bt.clone(),
    };
    IndexTerm {
        term: Term::Ite(
            Box::new(condition.clone()),
            Box::new(branch(then_)),
            Box::new(branch(else_)),
        ),
        bt: bt.clone(),
    }
}

fn replace_field(
    fields: &[(Id, IndexTerm)],
    member: &Id,
    value: &IndexTerm,
) -> Vec<(Id, IndexTerm)> {
    let mut fields = fields.to_vec();
    if let Some(slot) = fields.iter_mut().find(|(id, _)| id == member) {
        slot.1 = value.clone();
    }
    fields
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenPattern {
    Matched,
    Mismatch,
    Unknown,
}

fn valid_constructor(ctx: &GenModule, value: &IndexTerm) -> bool {
    if !matches!(&value.term, Term::Constructor(..)) {
        return false;
    }

    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        let Term::Constructor(name, members) = &value.term else {
            continue;
        };
        if declared_constructor(ctx.declarations(), name, members).is_none() {
            return false;
        }
        pending.extend(members.iter().filter_map(|(_, member)| {
            matches!(&member.term, Term::Constructor(..)).then_some(member)
        }));
    }
    true
}

fn bind_open_pattern(
    ctx: &GenModule,
    pattern: &Pattern,
    value: &IndexTerm,
    sub: &mut Substitution,
) -> OpenPattern {
    match &pattern.pattern {
        PatternKind::Sym(sym) => {
            sub.insert(sym.clone(), value.clone());
            OpenPattern::Matched
        }
        PatternKind::Wild => OpenPattern::Matched,
        PatternKind::Constructor(expected, fields) => {
            let Term::Constructor(actual, members) = &value.term else {
                return OpenPattern::Unknown;
            };
            if !valid_constructor(ctx, value) {
                return OpenPattern::Unknown;
            }
            if actual != expected {
                return OpenPattern::Mismatch;
            }
            for (id, field_pattern) in fields {
                let Some((_, member)) = members.iter().find(|(member_id, _)| member_id == id)
                else {
                    return OpenPattern::Unknown;
                };
                match bind_open_pattern(ctx, field_pattern, member, sub) {
                    OpenPattern::Matched => {}
                    other => return other,
                }
            }
            OpenPattern::Matched
        }
    }
}

fn const_int(it: &IndexTerm) -> Option<BigInt> {
    match &it.term {
        Term::Const(Const::Z(n)) => Some(n.clone()),
        Term::Const(Const::Bits((sign, width), n)) => Some(
            crate::gen_dsl::lang::base_type::normalise_to_range_big(*sign, *width, n),
        ),
        _ => None,
    }
}

fn is_zero(it: &IndexTerm) -> bool {
    const_int(it).is_some_and(|n| n == BigInt::ZERO)
}

fn is_one(it: &IndexTerm) -> bool {
    const_int(it).is_some_and(|n| n == BigInt::from(1))
}

fn bool_const(it: &IndexTerm) -> Option<bool> {
    match &it.term {
        Term::Const(Const::Bool(b)) => Some(*b),
        _ => None,
    }
}

fn bits_const(it: &IndexTerm) -> Option<((Sign, u16), &BigInt)> {
    match &it.term {
        Term::Const(Const::Bits(tag, n)) => Some((*tag, n)),
        _ => None,
    }
}

fn fits(sign: Sign, width: u16, n: &BigInt) -> bool {
    let (min, max) = bits_range(sign, width);
    *n >= min && *n <= max
}

fn range_fits_in(of_type: (Sign, u16), into: (Sign, u16)) -> bool {
    let (min, max) = bits_range(of_type.0, of_type.1);
    fits(into.0, into.1, &min) && fits(into.0, into.1, &max)
}

fn identities(n: &IndexTerm) -> Option<IndexTerm> {
    match &n.term {
        Term::Binop(op, a, b) => binop_identities(n, *op, a, b),
        Term::Unop(UnOp::Not, a) => match &a.term {
            Term::Unop(UnOp::Not, x) => Some((**x).clone()),
            _ => None,
        },
        Term::Unop(UnOp::Negate, a) => match &a.term {
            Term::Unop(UnOp::Negate, x) => Some((**x).clone()),
            _ => None,
        },
        Term::Ite(c, t, e) => {
            if let Some(taken) = bool_const(c) {
                return Some(if taken { (**t).clone() } else { (**e).clone() });
            }
            (t == e).then(|| (**t).clone())
        }
        Term::Cast(target, x) => (x.bt == *target).then(|| (**x).clone()),
        Term::NthTuple(index, tuple) => match &tuple.term {
            Term::Tuple(items) => items.get(*index).cloned(),
            _ => None,
        },
        Term::StructMember(base, member) => match &base.term {
            Term::Struct(_, members) => members
                .iter()
                .find(|(id, _)| id == member)
                .map(|(_, v)| v.clone()),
            _ => None,
        },
        Term::RecordMember(base, member) => match &base.term {
            Term::Record(members) => members
                .iter()
                .find(|(id, _)| id == member)
                .map(|(_, v)| v.clone()),
            _ => None,
        },
        Term::Const(_)
        | Term::Sym(_)
        | Term::Unop(_, _)
        | Term::EachI { .. }
        | Term::Tuple(_)
        | Term::Struct(_, _)
        | Term::StructUpdate { .. }
        | Term::Record(_)
        | Term::RecordUpdate { .. }
        | Term::Constructor(_, _)
        | Term::MemberShift(_, _, _)
        | Term::ArrayShift { .. }
        | Term::CopyAllocId { .. }
        | Term::HasAllocId(_)
        | Term::SizeOf(_)
        | Term::OffsetOf(_, _)
        | Term::Nil(_)
        | Term::Cons(_, _)
        | Term::Head(_)
        | Term::Tail(_)
        | Term::Representable(_, _)
        | Term::Good(_, _)
        | Term::Aligned { .. }
        | Term::WrapI(_, _)
        | Term::MapConst(_, _)
        | Term::MapSet(_, _, _)
        | Term::MapGet(_, _)
        | Term::MapDef(_, _)
        | Term::Apply(_, _)
        | Term::Let(_, _)
        | Term::Match(_, _)
        | Term::CnNone(_)
        | Term::CnSome(_)
        | Term::IsSome(_)
        | Term::GetOpt(_) => None,
    }
}

fn binop_identities(n: &IndexTerm, op: BinOp, a: &IndexTerm, b: &IndexTerm) -> Option<IndexTerm> {
    match op {
        BinOp::Add => {
            if is_zero(b) {
                return Some(a.clone());
            }
            is_zero(a).then(|| b.clone())
        }
        BinOp::Sub => {
            if is_zero(b) {
                return Some(a.clone());
            }
            if a == b && n.bt == BaseType::Integer {
                return IndexTerm::num_lit(BigInt::ZERO, &BaseType::Integer);
            }
            if let Term::Binop(BinOp::Add, c, d) = &a.term
                && **c == *b
            {
                return Some((**d).clone());
            }
            None
        }
        BinOp::Mul => {
            if is_zero(a) || is_zero(b) {
                return IndexTerm::num_lit(BigInt::ZERO, &n.bt);
            }
            if is_one(b) {
                return Some(a.clone());
            }
            is_one(a).then(|| b.clone())
        }
        BinOp::Div => is_one(b).then(|| a.clone()),
        BinOp::Rem | BinOp::Mod => {
            if is_one(b) {
                return IndexTerm::num_lit(BigInt::ZERO, &n.bt);
            }
            None
        }
        BinOp::And => match (bool_const(a), bool_const(b)) {
            (Some(true), _) => Some(b.clone()),
            (_, Some(true)) => Some(a.clone()),
            (Some(false), _) | (_, Some(false)) => Some(IndexTerm::boolean(false)),
            _ => (a == b).then(|| a.clone()),
        },
        BinOp::Or => match (bool_const(a), bool_const(b)) {
            (Some(true), _) | (_, Some(true)) => Some(IndexTerm::boolean(true)),
            (Some(false), _) => Some(b.clone()),
            (_, Some(false)) => Some(a.clone()),
            _ => (a == b).then(|| a.clone()),
        },
        BinOp::Implies => {
            if a == b {
                return Some(IndexTerm::boolean(true));
            }
            match (bool_const(a), bool_const(b)) {
                (Some(false), _) | (_, Some(true)) => Some(IndexTerm::boolean(true)),
                (Some(true), _) => Some(b.clone()),
                (_, Some(false)) => Some(IndexTerm {
                    term: Term::Unop(UnOp::Not, Box::new(a.clone())),
                    bt: BaseType::Bool,
                }),
                _ => None,
            }
        }
        BinOp::Eq | BinOp::Le | BinOp::LePointer => (a == b).then(|| IndexTerm::boolean(true)),
        BinOp::LtPointer => (a == b).then(|| IndexTerm::boolean(false)),
        BinOp::Min | BinOp::Max => (a == b).then(|| a.clone()),
        BinOp::Lt
        | BinOp::Exp
        | BinOp::BwXor
        | BinOp::BwAnd
        | BinOp::BwOr
        | BinOp::ShiftLeft
        | BinOp::ShiftRight
        | BinOp::SetUnion
        | BinOp::SetIntersection
        | BinOp::SetDifference
        | BinOp::SetMember
        | BinOp::Subset => None,
    }
}

fn fixes_rules(n: &IndexTerm) -> Option<IndexTerm> {
    if let Term::Binop(BinOp::Add, a, b) = &n.term {
        let arms = [(a, b), (b, a)];
        for (inner, outer) in arms {
            if let Some(((sign, width), n2)) = bits_const(outer)
                && let Some((it1, n1, inner_subtracts)) = split_inner_const_add(inner)
            {
                let merged = if inner_subtracts { n2 - &n1 } else { &n1 + n2 };
                if let Some(lit) = IndexTerm::num_lit(merged, &BaseType::Bits(sign, width)) {
                    return Some(IndexTerm {
                        term: Term::Binop(BinOp::Add, Box::new(it1.clone()), Box::new(lit)),
                        bt: n.bt.clone(),
                    });
                }
            }
        }
    }

    if let Term::Cast(bt1, x1) = &n.term {
        if let Term::Cast(bt2, x2) = &x1.term
            && let Term::Cast(bt3, x3) = &x2.term
            && let Term::Cast(bt4, x4) = &x3.term
            && bt1 == bt3
            && bt2 == bt4
        {
            return Some(IndexTerm::cast(
                bt1.clone(),
                IndexTerm::cast(bt2.clone(), (**x4).clone()),
            ));
        }
        if let Term::Cast(mid, inner) = &x1.term
            && inner.bt == *bt1
        {
            let lossless = match (bt1, mid) {
                (BaseType::Bits(s1, w1), BaseType::Bits(s2, w2)) => {
                    range_fits_in((*s1, *w1), (*s2, *w2))
                }
                (BaseType::Loc(_), BaseType::Bits(s2, w2)) => {
                    range_fits_in((Sign::Unsigned, 64), (*s2, *w2))
                }
                (BaseType::Bits(s1, w1), BaseType::Loc(_)) => {
                    range_fits_in((*s1, *w1), (Sign::Unsigned, 64))
                }
                _ => false,
            };
            if lossless {
                return Some((**inner).clone());
            }
        }
    }

    if let Term::Binop(BinOp::Eq, l, r) = &n.term {
        let matched = match (divided_and_multiplied(l), &r.term) {
            (Some((x, x_bt, k1, k2)), Term::Sym(x2)) if x == x2 && *k1 == *k2 => {
                Some((x, x_bt, k1))
            }
            _ => match (&l.term, divided_and_multiplied(r)) {
                (Term::Sym(x2), Some((x, x_bt, k1, k2))) if x == x2 && *k1 == *k2 => {
                    Some((x, x_bt, k1))
                }
                _ => None,
            },
        };
        if let Some((x, x_bt, k)) = matched
            && let Some(zero) = IndexTerm::num_lit(BigInt::ZERO, x_bt)
        {
            let modulus = IndexTerm {
                term: Term::Binop(
                    BinOp::Mod,
                    Box::new(IndexTerm::sym(x.clone(), x_bt.clone())),
                    Box::new(k.clone()),
                ),
                bt: x_bt.clone(),
            };
            return Some(IndexTerm::eq(modulus, zero));
        }
    }

    if let Term::Binop(min_or_max @ (BinOp::Min | BinOp::Max), a, b) = &n.term {
        if a == b {
            return Some((**a).clone());
        }
        let (ca, cb) = (bits_const(a), bits_const(b));
        if let (Some((_, n1)), Some((_, n2))) = (&ca, &cb) {
            let folded = match min_or_max {
                BinOp::Min => (*n1).min(*n2),
                _ => (*n1).max(*n2),
            };
            return IndexTerm::num_lit(folded.clone(), &n.bt);
        }
        let at_min = |c: &Option<((Sign, u16), &BigInt)>| {
            c.as_ref()
                .is_some_and(|((s, w), v)| **v == bits_range(*s, *w).0)
        };
        let at_max = |c: &Option<((Sign, u16), &BigInt)>| {
            c.as_ref()
                .is_some_and(|((s, w), v)| **v == bits_range(*s, *w).1)
        };
        let (l, r) = (&**a, &**b);
        let rules: [(bool, &IndexTerm); 4] = match min_or_max {
            BinOp::Min => [
                (at_min(&ca), l),
                (at_min(&cb), r),
                (at_max(&ca), r),
                (at_max(&cb), l),
            ],
            _ => [
                (at_max(&ca), l),
                (at_max(&cb), r),
                (at_min(&ca), r),
                (at_min(&cb), l),
            ],
        };
        return rules
            .iter()
            .find(|(hit, _)| *hit)
            .map(|(_, winner)| (*winner).clone());
    }

    None
}

fn split_inner_const_add(it: &IndexTerm) -> Option<(&IndexTerm, BigInt, bool)> {
    match &it.term {
        Term::Binop(BinOp::Add, x, y) => {
            if let Some((_, c)) = bits_const(y) {
                return Some((&**x, c.clone(), false));
            }
            bits_const(x).map(|(_, c)| (&**y, c.clone(), false))
        }
        Term::Binop(BinOp::Sub, x, y) => bits_const(y).map(|(_, c)| (&**x, c.clone(), true)),
        _ => None,
    }
}

fn divided_and_multiplied(
    it: &IndexTerm,
) -> Option<(
    &crate::gen_dsl::lang::sym::Sym,
    &BaseType,
    &IndexTerm,
    &IndexTerm,
)> {
    let Term::Binop(BinOp::Mul, l, k2) = &it.term else {
        return None;
    };
    let Term::Binop(BinOp::Div, subject, k1) = &l.term else {
        return None;
    };
    let Term::Sym(x) = &subject.term else {
        return None;
    };
    Some((x, &subject.bt, &**k1, &**k2))
}
