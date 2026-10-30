use super::*;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

pub const CHECKED_TRANSLATOR_REVISION: &str = "austen-gen-checked-v1";

pub const MAX_CHECKED_EXPANSION: usize = 65_536;

#[derive(Default)]
struct CheckedWalk {
    calls: Vec<Sym>,
    visited: usize,
    bodies: HashMap<CheckedCall, CheckedExpr>,
}

#[derive(PartialEq, Eq)]
struct CheckedCall {
    name: Sym,
    actuals: Vec<(BaseType, Dynamic)>,
}

impl Hash for CheckedCall {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.actuals.len().hash(state);
        for (_, actual) in &self.actuals {
            actual.hash(state);
        }
    }
}

#[derive(Debug, Clone)]
pub struct CheckedExpr {
    pub value: SmtExpr,
    pub defined: Bool,
}

impl CheckedExpr {
    fn total(value: SmtExpr) -> Self {
        Self {
            value,
            defined: Bool::from_bool(true),
        }
    }
}

impl Translator<'_> {
    pub fn checked(&mut self, env: &Env<'_>, it: &IndexTerm) -> Result<CheckedExpr, SmtError> {
        self.checked_term(env, it, &mut CheckedWalk::default())
    }

    fn checked_term(
        &mut self,
        env: &Env<'_>,
        it: &IndexTerm,
        walk: &mut CheckedWalk,
    ) -> Result<CheckedExpr, SmtError> {
        if walk.visited >= MAX_CHECKED_EXPANSION {
            return Err(SmtError::ExpansionLimit {
                what: "checked expression expansion",
                count: walk.visited as u128 + 1,
                limit: MAX_CHECKED_EXPANSION as u128,
            });
        }
        walk.visited += 1;
        self.enter()?;
        let out = self.checked_at_depth(env, it, walk);
        self.depth -= 1;
        out
    }

    fn checked_at_depth(
        &mut self,
        env: &Env<'_>,
        it: &IndexTerm,
        walk: &mut CheckedWalk,
    ) -> Result<CheckedExpr, SmtError> {
        checked_type(&it.bt, self.pointers())?;
        let out = match &it.term {
            Term::Const(c) => {
                match c {
                    Const::Z(_) | Const::Bits(..) | Const::Bool(_) | Const::Null => {}
                    Const::Pointer { addr, .. } if u64::try_from(addr).is_ok() => {}
                    _ => return Err(refused("constant", &it.bt)),
                }
                CheckedExpr::total(self.constant(c)?)
            }
            Term::Sym(sym) => CheckedExpr::total(
                env.get(sym)
                    .cloned()
                    .ok_or_else(|| SmtError::UnboundSymbol { sym: sym.clone() })?,
            ),
            Term::Unop(op, a) => {
                let a = self.checked_term(env, a, walk)?;
                CheckedExpr {
                    value: self.apply_unop(*op, a.value)?,
                    defined: a.defined,
                }
            }
            Term::Binop(op, a, b) => self.checked_binop(env, *op, a, b, walk)?,
            Term::Ite(c, t, e) => self.checked_ite(env, c, t, e, walk)?,
            Term::Let((sym, bound), body) => {
                let bound = self.checked_term(env, bound, walk)?;
                let scoped = env.push(sym, bound.value);
                let body = self.checked_term(&scoped, body, walk)?;
                CheckedExpr {
                    value: body.value,
                    defined: conj([bound.defined, body.defined]),
                }
            }
            Term::Record(fields) => self.checked_record(env, fields, walk)?,
            Term::RecordMember(record, member) => {
                let record = self.checked_term(env, record, walk)?;
                let (index, bt) = record_member_index(record.value.bt(), member)?;
                CheckedExpr {
                    value: self.project(&record.value, variant::FIRST, index, bt)?,
                    defined: record.defined,
                }
            }
            Term::Cast(to, a) => {
                let a = self.checked_term(env, a, walk)?;
                if !matches!(
                    (a.value.bt(), to),
                    (
                        BaseType::Integer | BaseType::Bits(..),
                        BaseType::Integer | BaseType::Bits(..)
                    ) | (BaseType::Bits(..), BaseType::Loc(_))
                        | (BaseType::Loc(_), BaseType::Bits(..) | BaseType::Loc(_))
                ) {
                    return Err(SmtError::UnsupportedCast {
                        from: a.value.bt.clone(),
                        to: to.clone(),
                    });
                }
                CheckedExpr {
                    value: self.apply_cast(to, a.value)?,
                    defined: a.defined,
                }
            }
            Term::Apply(name, args) => self.checked_apply(env, name, args, walk)?,
            other => return Err(refused(term_name(other), &it.bt)),
        };
        checked_agreement(&it.bt, out.value.bt())?;
        Ok(out)
    }

    fn checked_binop(
        &mut self,
        env: &Env<'_>,
        op: BinOp,
        a: &IndexTerm,
        b: &IndexTerm,
        walk: &mut CheckedWalk,
    ) -> Result<CheckedExpr, SmtError> {
        let l = self.checked_term(env, a, walk)?;
        let r = self.checked_term(env, b, walk)?;
        checked_agreement(l.value.bt(), r.value.bt())?;
        let defined = match (op, l.value.bt()) {
            (BinOp::And | BinOp::Implies, BaseType::Bool) => {
                conj([l.defined, guarded(l.value.expect_bool()?, r.defined)])
            }
            (BinOp::Or, BaseType::Bool) => {
                conj([l.defined, guarded(l.value.expect_bool()?.not(), r.defined)])
            }
            (BinOp::Exp, _) => return Err(refused(binop_name(op), l.value.bt())),
            (
                BinOp::Div | BinOp::Rem | BinOp::Mod,
                bt @ (BaseType::Bits(..) | BaseType::Integer),
            ) => {
                let zero = self.numeric_literal(BigInt::ZERO, bt)?;
                let nonzero = r
                    .value
                    .ast()
                    .safe_eq(zero.ast())
                    .map_err(|_| SmtError::TypeMismatch {
                        expected: bt.clone(),
                        actual: r.value.bt.clone(),
                    })?
                    .not();
                conj([l.defined, r.defined, nonzero])
            }
            (BinOp::ShiftLeft | BinOp::ShiftRight, BaseType::Bits(sign, width)) => {
                let amount = r.value.expect_bv()?.to_int(*sign == Sign::Signed);
                conj([
                    l.defined,
                    r.defined,
                    amount.ge(Int::from_i64(0)),
                    amount.lt(Int::from_u64(u64::from(*width))),
                ])
            }
            _ => conj([l.defined, r.defined]),
        };
        Ok(CheckedExpr {
            value: self.apply_binop(op, a, b, l.value, r.value)?,
            defined,
        })
    }

    fn checked_ite(
        &mut self,
        env: &Env<'_>,
        c: &IndexTerm,
        t: &IndexTerm,
        e: &IndexTerm,
        walk: &mut CheckedWalk,
    ) -> Result<CheckedExpr, SmtError> {
        let c = self.checked_term(env, c, walk)?;
        let t = self.checked_term(env, t, walk)?;
        let e = self.checked_term(env, e, walk)?;
        checked_agreement(t.value.bt(), e.value.bt())?;
        let condition = c.value.expect_bool()?;
        Ok(CheckedExpr {
            value: SmtExpr::new(condition.ite(t.value.ast(), e.value.ast()), t.value.bt),
            defined: conj([
                c.defined,
                guarded(condition.clone(), t.defined),
                guarded(condition.not(), e.defined),
            ]),
        })
    }

    fn checked_record(
        &mut self,
        env: &Env<'_>,
        fields: &[(Id, IndexTerm)],
        walk: &mut CheckedWalk,
    ) -> Result<CheckedExpr, SmtError> {
        let mut parts = Vec::with_capacity(fields.len());
        let mut members = Vec::with_capacity(fields.len());
        let mut defined = Vec::with_capacity(fields.len());
        for (id, term) in fields {
            let part = self.checked_term(env, term, walk)?;
            members.push((id.clone(), part.value.bt.clone()));
            parts.push(part.value);
            defined.push(part.defined);
        }
        Ok(CheckedExpr {
            value: self.build(BaseType::Record(members), variant::FIRST, &parts)?,
            defined: conj(defined),
        })
    }

    fn checked_apply(
        &mut self,
        env: &Env<'_>,
        name: &Sym,
        args: &[IndexTerm],
        walk: &mut CheckedWalk,
    ) -> Result<CheckedExpr, SmtError> {
        let def = self.sorts.program().function(name)?;
        let Some(body) = def.body.as_ref() else {
            return Err(SmtError::UninterpretedFunction { name: name.clone() });
        };
        if matches!(body, FunctionBody::RecDef(_)) || walk.calls.contains(name) {
            return Err(SmtError::RecursiveCheckedFunction { name: name.clone() });
        }
        if def.args.len() != args.len() {
            return Err(SmtError::FunctionArity {
                name: name.clone(),
                expected: def.args.len(),
                actual: args.len(),
            });
        }
        let mut actuals = SymMap::new();
        let mut key = CheckedCall {
            name: name.clone(),
            actuals: Vec::with_capacity(args.len()),
        };
        let mut defined = Vec::with_capacity(args.len() + 1);
        for ((param, bt), arg) in def.args.iter().zip(args) {
            let arg = self.checked_term(env, arg, walk)?;
            checked_agreement(bt, arg.value.bt())?;
            key.actuals
                .push((arg.value.bt.clone(), arg.value.ast.clone()));
            actuals.insert(param.clone(), arg.value);
            defined.push(arg.defined);
        }
        let result = if let Some(result) = walk.bodies.get(&key) {
            result.clone()
        } else {
            walk.calls.push(name.clone());
            let result = self.checked_term(&Env::Root(&actuals), body.term(), walk);
            walk.calls.pop();
            let result = result?;
            checked_agreement(&def.return_bt, result.value.bt())?;
            walk.bodies.insert(key, result.clone());
            result
        };
        defined.push(result.defined);
        Ok(CheckedExpr {
            value: result.value,
            defined: conj(defined),
        })
    }
}

fn refused(construct: &'static str, bt: &BaseType) -> SmtError {
    SmtError::UnsupportedCheckedSemantics {
        construct,
        bt: bt.clone(),
    }
}

fn conj(parts: impl IntoIterator<Item = Bool>) -> Bool {
    let mut parts: Vec<Bool> = parts
        .into_iter()
        .filter(|part| part.as_bool() != Some(true))
        .collect();
    if parts.len() > 1 {
        Bool::and(&parts)
    } else {
        parts.pop().unwrap_or_else(|| Bool::from_bool(true))
    }
}

fn guarded(condition: Bool, defined: Bool) -> Bool {
    if defined.as_bool() == Some(true) {
        defined
    } else {
        condition.implies(&defined)
    }
}

fn checked_agreement(expected: &BaseType, actual: &BaseType) -> Result<(), SmtError> {
    if expected == actual || expected.erase_pointees() == actual.erase_pointees() {
        Ok(())
    } else {
        Err(SmtError::TypeMismatch {
            expected: expected.clone(),
            actual: actual.clone(),
        })
    }
}

fn checked_type(bt: &BaseType, pointers: PointerEncoding) -> Result<(), SmtError> {
    let mut pending = vec![(bt, 0usize)];
    while let Some((at, depth)) = pending.pop() {
        if depth >= MAX_TERM_DEPTH {
            return Err(SmtError::ExpansionLimit {
                what: "checked type nesting",
                count: depth as u128 + 1,
                limit: MAX_TERM_DEPTH as u128,
            });
        }
        match at {
            BaseType::Bool | BaseType::Integer => {}
            BaseType::Bits(_, 0) => return Err(SmtError::ZeroWidthBitvector),
            BaseType::Bits(..) => {}
            BaseType::Loc(_) if pointers == PointerEncoding::Address => {}
            BaseType::Record(members) => {
                let mut names = std::collections::BTreeSet::new();
                for (name, member) in members {
                    if !names.insert(name) {
                        return Err(refused("duplicate record field", at));
                    }
                    pending.push((member, depth + 1));
                }
            }
            _ => return Err(refused("base type", at)),
        }
    }
    Ok(())
}
