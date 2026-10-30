use austen_lib::gen_dsl::choice::translate::scalar_value;
use austen_lib::gen_dsl::{BaseType, Env, IndexTerm, Scalar, Sym, Term, Value};

use super::blame::{Failure, PointerReq};

fn native_width(width: u16) -> bool {
    matches!(width, 8 | 16 | 32 | 64)
}

pub(super) fn supported(target: &BaseType) -> bool {
    match target {
        BaseType::Loc(_) => true,
        BaseType::Bits(_, width) => native_width(*width),
        _ => false,
    }
}

pub(super) fn of(value: &Value) -> Option<u64> {
    match value {
        Value::Scalar(Scalar::Pointer { addr, .. }) => Some(*addr),
        Value::Scalar(Scalar::Bits(_, width, _)) if native_width(*width) => {
            value.as_int().map(|address| address as u64)
        }
        _ => None,
    }
}

pub(super) fn carrier(address: u64, target: &BaseType) -> Value {
    debug_assert!(supported(target));
    scalar_value(i128::from(address), target).expect("a non-negative address fits every carrier")
}

pub(super) fn for_each_base(term: &IndexTerm, env: &Env, mut visit: impl FnMut(Sym, u64)) -> bool {
    let mut base = term;
    loop {
        match &base.term {
            Term::Sym(sym) => {
                if supported(&base.bt)
                    && let Some(address) = env.get(sym).and_then(of)
                {
                    visit(sym.clone(), address);
                    return true;
                }
                return false;
            }
            Term::Cast(_, inner)
            | Term::MemberShift(inner, _, _)
            | Term::ArrayShift { base: inner, .. }
            | Term::CopyAllocId { loc: inner, .. } => base = inner,
            _ => {
                for sym in term.free_vars() {
                    if let Some(Value::Scalar(Scalar::Pointer { addr, .. })) = env.get(&sym) {
                        visit(sym, *addr);
                    }
                }
                return false;
            }
        }
    }
}

pub(super) fn blame_bases(
    failure: &mut Failure,
    term: &IndexTerm,
    env: &Env,
    requirement: impl Fn(u64) -> PointerReq,
) {
    for_each_base(term, env, |sym, base| {
        failure.blame(sym, Some(requirement(base)))
    });
}
