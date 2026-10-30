use crate::gen_dsl::lang::gen_module::{call_graph_of, transitive_closure};
use crate::gen_dsl::{GenDef, GenTerm, GenTermKind, Sym, SymMap};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EffectBits {
    may_scope: bool,
    may_write_choice: bool,
}

impl EffectBits {
    pub const ALL: Self = Self {
        may_scope: true,
        may_write_choice: true,
    };

    pub const fn may_scope(self) -> bool {
        self.may_scope
    }

    pub const fn may_write_choice(self) -> bool {
        self.may_write_choice
    }

    pub const fn may_touch_tree(self) -> bool {
        self.may_scope || self.may_write_choice
    }

    fn scope() -> Self {
        Self {
            may_scope: true,
            may_write_choice: false,
        }
    }

    fn write() -> Self {
        Self {
            may_scope: false,
            may_write_choice: true,
        }
    }

    fn join(&mut self, other: Self) {
        self.may_scope |= other.may_scope;
        self.may_write_choice |= other.may_write_choice;
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChoiceEffects {
    defs: SymMap<EffectBits>,
}

impl ChoiceEffects {
    pub fn analyze(defs: &[(Sym, GenDef)]) -> Self {
        let reach = transitive_closure(&call_graph_of(defs));
        let mut direct = SymMap::new();
        for (name, def) in defs {
            let mut bits = EffectBits::default();
            scan(&def.body, &mut bits, &mut |_| EffectBits::default());
            direct.insert(name.clone(), bits);
        }

        let summaries = direct
            .iter()
            .map(|(name, own)| {
                let mut bits = *own;
                for callee in &reach[name] {
                    bits.join(direct.get(callee).copied().unwrap_or(EffectBits::ALL));
                }
                (name.clone(), bits)
            })
            .collect();
        Self { defs: summaries }
    }

    pub fn definition(&self, fsym: &Sym) -> EffectBits {
        self.defs.get(fsym).copied().unwrap_or(EffectBits::ALL)
    }

    pub fn callee(&self, fsym: &Sym) -> EffectBits {
        self.definition(fsym)
    }

    pub fn term(&self, term: &GenTerm) -> EffectBits {
        let mut bits = EffectBits::default();
        scan(term, &mut bits, &mut |callee| self.callee(callee));
        bits
    }

    pub fn may_touch_tree(&self, term: &GenTerm) -> bool {
        self.term(term).may_touch_tree()
    }

    pub fn may_write_choice(&self, term: &GenTerm) -> bool {
        self.term(term).may_write_choice()
    }
}

fn scan(term: &GenTerm, bits: &mut EffectBits, call: &mut impl FnMut(&Sym) -> EffectBits) {
    match &term.term {
        GenTermKind::Arbitrary | GenTermKind::Return(_) => {}
        GenTermKind::Call { fsym, .. } => bits.join(call(fsym)),
        GenTermKind::WriteChoice { rest, .. } => {
            bits.join(EffectBits::write());
            scan(rest, bits, call);
        }
        GenTermKind::Scope { body, .. } => {
            bits.join(EffectBits::scope());
            scan(body, bits, call);
        }
        GenTermKind::Asgn { rest, .. } | GenTermKind::Assert { rest, .. } => {
            scan(rest, bits, call);
        }
        GenTermKind::LetStar { value, rest, .. } => {
            scan(value, bits, call);
            scan(rest, bits, call);
        }
        GenTermKind::Ite { then_, else_, .. } => {
            scan(then_, bits, call);
            scan(else_, bits, call);
        }
        GenTermKind::Map { body, .. } => scan(body, bits, call),
    }
}
