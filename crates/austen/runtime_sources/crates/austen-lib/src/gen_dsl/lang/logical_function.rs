use serde::{Deserialize, Serialize};

use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::free_vars::bound_by_pattern;
use crate::gen_dsl::lang::index_term::{IndexTerm, Term};
use crate::gen_dsl::lang::sym::{Sym, SymMap, SymSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FunctionBody {
    Def(IndexTerm),
    RecDef(IndexTerm),
}

impl FunctionBody {
    pub fn term(&self) -> &IndexTerm {
        match self {
            FunctionBody::Def(it) | FunctionBody::RecDef(it) => it,
        }
    }

    pub fn is_recursive(&self) -> bool {
        matches!(self, FunctionBody::RecDef(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogicalFunctionDef {
    pub name: Sym,
    #[serde(default)]
    pub args: Vec<(Sym, BaseType)>,
    pub return_bt: BaseType,
    #[serde(default)]
    pub body: Option<FunctionBody>,
}

impl LogicalFunctionDef {
    pub fn is_recursive(&self) -> bool {
        self.body.as_ref().is_some_and(FunctionBody::is_recursive)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FunctionTableError {
    #[error(
        "logical function {name:?} is declared `Def` but its unfolding reaches itself \
         (mutually recursive group: {group:?})"
    )]
    CyclicDef { name: Sym, group: Vec<Sym> },
}

pub fn well_formed(functions: &[(Sym, LogicalFunctionDef)]) -> Result<(), FunctionTableError> {
    let graph = DefGraph::of(functions);
    let cyclic = graph.cyclic_members();
    for (name, _) in functions {
        if let Some(group) = cyclic.get(name) {
            return Err(FunctionTableError::CyclicDef {
                name: name.clone(),
                group: group.clone(),
            });
        }
    }
    Ok(())
}

pub fn unfoldable_defs(functions: &[(Sym, LogicalFunctionDef)]) -> SymSet {
    let graph = DefGraph::of(functions);
    let cyclic = graph.cyclic_members();
    graph
        .names
        .iter()
        .filter(|name| !cyclic.contains_key(*name))
        .cloned()
        .collect()
}

pub(in crate::gen_dsl) fn binders(it: &IndexTerm) -> SymSet {
    let mut applied = SymSet::new();
    let mut bound = SymSet::new();
    scan_body(it, &mut applied, &mut bound);
    bound
}

fn scan_body(it: &IndexTerm, applied: &mut SymSet, bound: &mut SymSet) {
    match &it.term {
        Term::Const(_)
        | Term::Sym(_)
        | Term::SizeOf(_)
        | Term::OffsetOf(_, _)
        | Term::Nil(_)
        | Term::CnNone(_) => {}
        Term::Unop(_, a)
        | Term::NthTuple(_, a)
        | Term::StructMember(a, _)
        | Term::RecordMember(a, _)
        | Term::MemberShift(a, _, _)
        | Term::HasAllocId(a)
        | Term::Head(a)
        | Term::Tail(a)
        | Term::Representable(_, a)
        | Term::Good(_, a)
        | Term::WrapI(_, a)
        | Term::MapConst(_, a)
        | Term::Cast(_, a)
        | Term::CnSome(a)
        | Term::IsSome(a)
        | Term::GetOpt(a) => scan_body(a, applied, bound),
        Term::Binop(_, a, b)
        | Term::ArrayShift {
            base: a, index: b, ..
        }
        | Term::CopyAllocId { addr: a, loc: b }
        | Term::Cons(a, b)
        | Term::Aligned { t: a, align: b }
        | Term::MapGet(a, b)
        | Term::StructUpdate {
            target: a,
            value: b,
            ..
        }
        | Term::RecordUpdate {
            target: a,
            value: b,
            ..
        } => {
            scan_body(a, applied, bound);
            scan_body(b, applied, bound);
        }
        Term::Ite(a, b, c) | Term::MapSet(a, b, c) => {
            scan_body(a, applied, bound);
            scan_body(b, applied, bound);
            scan_body(c, applied, bound);
        }
        Term::Tuple(ts) => {
            for t in ts {
                scan_body(t, applied, bound);
            }
        }
        Term::Struct(_, ms) | Term::Record(ms) | Term::Constructor(_, ms) => {
            for (_, t) in ms {
                scan_body(t, applied, bound);
            }
        }
        Term::EachI {
            var: (s, _), body, ..
        }
        | Term::MapDef((s, _), body) => {
            bound.insert(s.clone());
            scan_body(body, applied, bound);
        }
        Term::Let((s, value), body) => {
            bound.insert(s.clone());
            scan_body(value, applied, bound);
            scan_body(body, applied, bound);
        }
        Term::Match(scrutinee, arms) => {
            scan_body(scrutinee, applied, bound);
            for (pattern, arm) in arms {
                bound_by_pattern(pattern, bound);
                scan_body(arm, applied, bound);
            }
        }
        Term::Apply(name, args) => {
            applied.insert(name.clone());
            for arg in args {
                scan_body(arg, applied, bound);
            }
        }
    }
}

struct DefGraph {
    names: Vec<Sym>,
    edges: Vec<Vec<usize>>,
}

impl DefGraph {
    fn of(functions: &[(Sym, LogicalFunctionDef)]) -> DefGraph {
        let mut index: SymMap<usize> = SymMap::new();
        let mut names = Vec::new();
        let mut bodies = Vec::new();
        for (name, def) in functions {
            if index.contains_key(name) {
                continue;
            }
            if let Some(FunctionBody::Def(body)) = &def.body {
                index.insert(name.clone(), names.len());
                names.push(name.clone());
                bodies.push(body);
            }
        }
        let edges = bodies
            .iter()
            .map(|body| {
                let mut applied = SymSet::new();
                let mut bound = SymSet::new();
                scan_body(body, &mut applied, &mut bound);
                applied
                    .iter()
                    .filter_map(|g| index.get(g).copied())
                    .collect()
            })
            .collect();
        DefGraph { names, edges }
    }

    fn cyclic_members(&self) -> SymMap<Vec<Sym>> {
        const UNVISITED: usize = usize::MAX;
        let n = self.names.len();
        let mut index = vec![UNVISITED; n];
        let mut low = vec![0usize; n];
        let mut on_stack = vec![false; n];
        let mut stack: Vec<usize> = Vec::new();
        let mut next = 0usize;
        let mut out: SymMap<Vec<Sym>> = SymMap::new();
        let mut call: Vec<(usize, usize)> = Vec::new();

        for root in 0..n {
            if index[root] != UNVISITED {
                continue;
            }
            index[root] = next;
            low[root] = next;
            next += 1;
            stack.push(root);
            on_stack[root] = true;
            call.push((root, 0));
            while let Some(frame) = call.last_mut() {
                let v = frame.0;
                if frame.1 < self.edges[v].len() {
                    let w = self.edges[v][frame.1];
                    frame.1 += 1;
                    if index[w] == UNVISITED {
                        index[w] = next;
                        low[w] = next;
                        next += 1;
                        stack.push(w);
                        on_stack[w] = true;
                        call.push((w, 0));
                    } else if on_stack[w] {
                        low[v] = low[v].min(index[w]);
                    }
                } else {
                    call.pop();
                    if let Some(parent) = call.last() {
                        low[parent.0] = low[parent.0].min(low[v]);
                    }
                    if low[v] == index[v] {
                        let mut component = Vec::new();
                        loop {
                            let w = stack.pop().expect("Tarjan stack holds the component");
                            on_stack[w] = false;
                            component.push(w);
                            if w == v {
                                break;
                            }
                        }
                        if component.len() > 1 || self.edges[v].contains(&v) {
                            let mut group: Vec<Sym> =
                                component.iter().map(|w| self.names[*w].clone()).collect();
                            group.sort();
                            for w in &component {
                                out.insert(self.names[*w].clone(), group.clone());
                            }
                        }
                    }
                }
            }
        }
        out
    }
}
