use serde::{Deserialize, Serialize};

use crate::gen_dsl::TestCaseSize;
use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::gen_term::GenTerm;
use crate::gen_dsl::lang::index_term::IndexTerm;
use crate::gen_dsl::lang::sctype::Sctype;
use crate::gen_dsl::lang::sym::{Sym, SymSet};
use crate::gen_dsl::region::Region;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GenDef {
    pub filename: String,
    pub recursive: bool,
    pub spec: bool,
    pub name: Sym,
    pub iargs: Vec<(Sym, BaseType)>,
    pub oarg: BaseType,
    pub c_types: Option<Vec<(Sym, Sctype)>>,
    pub body: GenTerm,
    #[serde(default)]
    pub size_arg: Option<Sym>,
    #[serde(default)]
    pub rec_size_arg: Option<Sym>,
    #[serde(default)]
    pub heap_base_arg: Option<Sym>,
    #[serde(default)]
    pub heap_size_arg: Option<Sym>,
}

impl GenDef {
    pub fn same_signature(&self, other: &GenDef) -> bool {
        let Self {
            filename,
            recursive,
            spec,
            name,
            iargs: _,
            oarg,
            c_types,
            size_arg,
            rec_size_arg,
            heap_base_arg: _,
            heap_size_arg: _,
            body: _,
        } = self;
        *filename == other.filename
            && *recursive == other.recursive
            && *spec == other.spec
            && name.num == other.name.num
            && name.name == other.name.name
            && self.signature_iargs().eq(other.signature_iargs())
            && *oarg == other.oarg
            && *c_types == other.c_types
            && *size_arg == other.size_arg
            && *rec_size_arg == other.rec_size_arg
    }

    fn signature_iargs(&self) -> impl Iterator<Item = &(Sym, BaseType)> {
        self.iargs
            .iter()
            .filter(|(sym, _)| !self.heap_args().any(|heap| heap == sym))
    }

    pub fn size_args(&self) -> impl Iterator<Item = &Sym> {
        self.size_arg.iter().chain(&self.rec_size_arg)
    }

    pub fn heap_args(&self) -> impl Iterator<Item = &Sym> {
        self.heap_base_arg.iter().chain(&self.heap_size_arg)
    }

    pub fn iarg_syms(&self) -> SymSet {
        self.iargs.iter().map(|(sym, _)| sym.clone()).collect()
    }

    pub fn root_call_with_size(&self, size: TestCaseSize, heap: Region) -> GenTerm {
        let args = self
            .iargs
            .iter()
            .map(|(param, bt)| {
                let literal = if self.size_args().any(|s| s == param) {
                    Some(u128::from(size.get()))
                } else if self.heap_base_arg.as_ref() == Some(param) {
                    Some(u128::from(heap.base))
                } else if self.heap_size_arg.as_ref() == Some(param) {
                    Some(u128::from(heap.size))
                } else {
                    None
                };
                match literal {
                    Some(value) => IndexTerm::num_lit(value.into(), bt)
                        .expect("a u64 inhabits the declared uintptr argument type"),
                    None => IndexTerm::sym(param.clone(), bt.clone()),
                }
            })
            .collect();
        GenTerm::call(self.name.clone(), args, self.oarg.clone())
    }
}
