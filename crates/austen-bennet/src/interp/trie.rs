use std::collections::BTreeMap;

use austen_lib::gen_dsl::{Label, Scope, ScopeKey, ScopeMergeError};

#[derive(Debug, Default)]
pub(crate) struct Trie {
    scope: Scope,
    fragments: BTreeMap<ScopeKey, Vec<Scope>>,
    journal: Vec<Entry>,
}

#[derive(Debug)]
enum Entry {
    Choice(Label),
    Sub(ScopeKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TrieMark(usize);

impl Trie {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn mark(&self) -> TrieMark {
        TrieMark(self.journal.len())
    }

    pub fn rewind(&mut self, mark: TrieMark) {
        while self.journal.len() > mark.0 {
            match self.journal.pop().expect("journal longer than the mark") {
                Entry::Choice(label) => self.scope.remove_choice(label),
                Entry::Sub(key) => {
                    let remove = {
                        let fragments = self
                            .fragments
                            .get_mut(&key)
                            .expect("a journaled scope fragment exists");
                        fragments.pop().expect("a journaled fragment exists");
                        fragments.is_empty()
                    };
                    if remove {
                        self.fragments.remove(&key);
                    }
                }
            }
        }
    }

    pub fn insert(&mut self, label: Label, choice: austen_lib::gen_dsl::Choice) {
        self.scope.insert(label, choice);
        self.journal.push(Entry::Choice(label));
    }

    pub fn push_scope(&mut self, key: ScopeKey, child: Scope) {
        if child.is_empty() {
            return;
        }
        self.fragments.entry(key.clone()).or_default().push(child);
        self.journal.push(Entry::Sub(key));
    }

    pub fn finish(mut self) -> Result<Scope, ScopeMergeError> {
        for (key, fragments) in self.fragments {
            for fragment in fragments {
                self.scope.try_merge_scope(&key, fragment)?;
            }
        }
        Ok(self.scope)
    }
}
