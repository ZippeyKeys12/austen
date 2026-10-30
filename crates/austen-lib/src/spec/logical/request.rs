use crate::gen_dsl::{BaseType, IndexTerm, Sctype, Sym, SymMap};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Init {
    Init,
    Uninit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestName {
    Owned(Sctype, Init),
    PName(Sym),
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    P {
        name: RequestName,
        pointer: IndexTerm,
        iargs: Vec<IndexTerm>,
    },
    Q {
        name: RequestName,
        pointer: IndexTerm,
        q: (Sym, BaseType),
        step: Sctype,
        permission: IndexTerm,
        iargs: Vec<IndexTerm>,
    },
}

impl Request {
    pub fn name(&self) -> &RequestName {
        match self {
            Request::P { name, .. } | Request::Q { name, .. } => name,
        }
    }

    pub fn predicate_name(&self) -> Option<&Sym> {
        match self.name() {
            RequestName::PName(sym) => Some(sym),
            RequestName::Owned(_, _) => None,
        }
    }

    pub fn free_vars_bts(&self) -> SymMap<BaseType> {
        match self {
            Request::P { pointer, iargs, .. } => {
                crate::gen_dsl::free_vars_bts_list(std::iter::once(pointer).chain(iargs))
            }
            Request::Q {
                pointer,
                q,
                permission,
                iargs,
                ..
            } => {
                let mut inner =
                    crate::gen_dsl::free_vars_bts_list(std::iter::once(permission).chain(iargs));
                inner.remove(&q.0);
                let mut vars = pointer.free_vars_bts();
                for (sym, bt) in inner {
                    vars.entry(sym).or_insert(bt);
                }
                vars
            }
        }
    }
}
