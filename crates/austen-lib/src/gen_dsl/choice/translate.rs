use crate::gen_dsl::choice::draws::{
    Blind, ChoiceTape, Draw, DrawSource, DrawTape, Site, endless, of_values,
};
use crate::gen_dsl::choice::interp::GenFailure;
use crate::gen_dsl::choice::seq::Bv;
use crate::gen_dsl::choice::tree::{
    AttachError, Choice, ChoiceTree, Label, Scope, ScopeKey, ScopeMergeError,
};
use crate::gen_dsl::incoming::{Grant, Incoming};
use crate::gen_dsl::lang::base_type::{BaseType, Sign, normalise_to_range, normalise_to_range_big};
use crate::gen_dsl::lang::bytes::value_bytes;
use crate::gen_dsl::lang::env::Env;
use crate::gen_dsl::lang::eval::Evaluator;

use crate::gen_dsl::lang::gen_def::GenDef;
use crate::gen_dsl::lang::gen_module::{GenCallError, GenModule};
use crate::gen_dsl::lang::gen_term::{GenTerm, GenTermKind};
use crate::gen_dsl::lang::id::Id;
use crate::gen_dsl::lang::index_term::{BinOp, IndexTerm, Term};
use crate::gen_dsl::lang::logical_constraint::LogicalConstraint;
use crate::gen_dsl::lang::sctype::Sctype;
use crate::gen_dsl::lang::struct_def::StructDef;
use crate::gen_dsl::lang::sym::{Sym, SymMap};
use crate::gen_dsl::lang::value::{PointerOrigin, Scalar, Value};
use crate::gen_dsl::marked_heap::MarkedHeap;

use crate::gen_dsl::rand_like::TestCaseSize;
use crate::gen_dsl::region::Region;
use crate::gen_dsl::{DefaultChoices, DefaultError, DefaultTypes};

pub const MAX_CALL_DEPTH: usize = 64;

pub const MAX_MAP_ITERATIONS: i128 = 4096;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TranslateError {
    #[error(transparent)]
    Default(#[from] DefaultError),
    #[error("no scalar width for base type {bt:?}")]
    UnsupportedBaseType { bt: BaseType },
    #[error("no byte size for C type {ct:?}")]
    UnsizedCtype { ct: Sctype },
    #[error("asgn write [{addr:#x}, +{size}) is outside writable memory")]
    AsgnOutOfBounds { addr: u64, size: u64 },
    #[error("asgn write [{addr:#x}, +{size}) overlaps an earlier write")]
    AsgnOverlap { addr: u64, size: u64 },
    #[error("cannot evaluate the {context}")]
    Unevaluatable { context: &'static str },
    #[error("assert constraint failed")]
    FailedConstraint,
    #[error("call to unknown generator {fsym:?}")]
    UnknownGenerator { fsym: Sym },
    #[error("call to {fsym:?} passes {actual} args, definition takes {expected}")]
    CallArityMismatch {
        fsym: Sym,
        expected: usize,
        actual: usize,
    },
    #[error("calls nest deeper than {} at {fsym:?}", MAX_CALL_DEPTH)]
    CallDepthExceeded { fsym: Sym },
    #[error("map permission is not a recognized finite range")]
    UnsupportedMapDomain,
    #[error("map domain {lo}..{hi} exceeds {} iterations", MAX_MAP_ITERATIONS)]
    MapDomainTooLarge { lo: i128, hi: i128 },
    #[error("no tree choice for the write labeled {}", .label.0)]
    MissingChoice { label: Label },
    #[error("no tree scope {key} for the scope term")]
    MissingScope { key: ScopeKey },
    #[error("choice {}: term wants {expected} bits, tree has {actual}", .label.0)]
    WidthMismatch {
        label: Label,
        expected: u16,
        actual: u16,
    },
    #[error("choice {}: {value} does not fit {bits} unsigned bits", .label.0)]
    ValueOutOfRange {
        label: Label,
        value: i128,
        bits: u16,
    },
    #[error(
        "choice {}: computed value {expected} does not match tree value {actual}",
        .label.0
    )]
    ChoiceMismatch {
        label: Label,
        expected: i128,
        actual: i128,
    },
    #[error("the recorded aggregate at choice {} does not match the site", .label.0)]
    StructChoice { label: Label },
    #[error("tree choice {} is never read by the term", .label.0)]
    UnusedChoice { label: Label },
    #[error("tree scope {key} is never entered by the term")]
    UnusedScope { key: ScopeKey },
}

impl From<GenCallError> for TranslateError {
    fn from(error: GenCallError) -> Self {
        match error {
            GenCallError::UnknownGenerator { name } => Self::UnknownGenerator { fsym: name },
            GenCallError::Arity {
                name,
                expected,
                actual,
            } => Self::CallArityMismatch {
                fsym: name,
                expected,
                actual,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReconstructError {
    #[error(transparent)]
    Default(#[from] DefaultError),
    #[error("no scalar width for base type {bt:?}")]
    UnsupportedBaseType { bt: BaseType },
    #[error("no byte size for C type {ct:?}")]
    UnsizedCtype { ct: Sctype },
    #[error("asgn write [{addr:#x}, +{size}) is outside writable memory")]
    AsgnOutOfBounds { addr: u64, size: u64 },
    #[error("asgn write [{addr:#x}, +{size}) overlaps an earlier write")]
    AsgnOverlap { addr: u64, size: u64 },
    #[error("choice {}: width {bits} does not fit a choice", .label.0)]
    UnrepresentableWidth { label: Label, bits: u16 },
    #[error("cannot evaluate the {context}")]
    Unevaluatable { context: &'static str },
    #[error("assert constraint failed")]
    FailedConstraint,
    #[error("call to unknown generator {fsym:?}")]
    UnknownGenerator { fsym: Sym },
    #[error("call to {fsym:?} passes {actual} args, definition takes {expected}")]
    CallArityMismatch {
        fsym: Sym,
        expected: usize,
        actual: usize,
    },
    #[error("calls nest deeper than {} at {fsym:?}", MAX_CALL_DEPTH)]
    CallDepthExceeded { fsym: Sym },
    #[error("map permission is not a recognized finite range")]
    UnsupportedMapDomain,
    #[error("map domain {lo}..{hi} exceeds {} iterations", MAX_MAP_ITERATIONS)]
    MapDomainTooLarge { lo: i128, hi: i128 },
    #[error(transparent)]
    ScopeMerge(#[from] ScopeMergeError),
    #[error("the recorded aggregate at choice {} does not match the site", .label.0)]
    StructChoice { label: Label },
    #[error("input ends before the write labeled {}", .label.0)]
    Truncated { label: Label },
    #[error("choice {}: {value} does not fit {bits} unsigned bits", .label.0)]
    ValueOutOfRange {
        label: Label,
        value: i128,
        bits: u16,
    },
    #[error(
        "choice {}: computed value {expected} does not match input value {actual}",
        .label.0
    )]
    ChoiceMismatch {
        label: Label,
        expected: i128,
        actual: i128,
    },
    #[error("{count} tape bytes are never drawn by the term")]
    TrailingBytes { count: usize },
    #[error("{count} supplied values are never drawn by the term")]
    TrailingValues { count: usize },
}

impl From<GenCallError> for ReconstructError {
    fn from(error: GenCallError) -> Self {
        match error {
            GenCallError::UnknownGenerator { name } => Self::UnknownGenerator { fsym: name },
            GenCallError::Arity {
                name,
                expected,
                actual,
            } => Self::CallArityMismatch {
                fsym: name,
                expected,
                actual,
            },
        }
    }
}

#[derive(Clone, Copy)]
struct WireContext<'a> {
    ctx: &'a GenModule,
    incoming: &'a Incoming,
}

impl<'a> WireContext<'a> {
    fn new(ctx: &'a GenModule, incoming: &'a Incoming) -> Self {
        Self { ctx, incoming }
    }
}

#[derive(Clone, Copy)]
pub struct TreeLinearizer<'a>(WireContext<'a>);

impl<'a> TreeLinearizer<'a> {
    pub fn new(ctx: &'a GenModule, incoming: &'a Incoming) -> Self {
        Self(WireContext::new(ctx, incoming))
    }

    pub fn linearize(
        &self,
        term: &GenTerm,
        initial_env: Env,
        tree: &ChoiceTree,
    ) -> Result<ChoiceTape, TranslateError> {
        term.walk_root(
            self.0.ctx,
            Memory::Incoming(self.0.incoming),
            tree,
            Checks::Skipped,
            initial_env,
        )
    }

    pub fn linearize_with_evaluator(
        &self,
        term: &GenTerm,
        initial_env: Env,
        tree: &ChoiceTree,
        evaluator: &Evaluator<'_>,
    ) -> Result<ChoiceTape, TranslateError> {
        term.walk_root_with_evaluator(
            self.0.ctx,
            Memory::Incoming(self.0.incoming),
            tree,
            Checks::Skipped,
            initial_env,
            evaluator,
        )
    }
}

#[derive(Clone, Copy)]
pub struct TapeParser<'a>(WireContext<'a>);

impl<'a> TapeParser<'a> {
    pub fn new(ctx: &'a GenModule, incoming: &'a Incoming) -> Self {
        Self(WireContext::new(ctx, incoming))
    }

    pub fn parse(
        &self,
        term: &GenTerm,
        initial_env: Env,
        tape: &[u8],
    ) -> Result<ParseOutput, ReconstructError> {
        self.parse_from_checks(
            term,
            initial_env,
            &mut endless(DrawTape::new(tape)),
            Checks::Checked,
        )
    }

    pub fn parse_with_defaults(
        &self,
        term: &GenTerm,
        initial_env: Env,
        tape: &[u8],
        post_defaults: &DefaultTypes,
    ) -> Result<ParseOutput, ReconstructError> {
        self.parse_from_with_defaults(
            term,
            initial_env,
            &mut endless(DrawTape::new(tape)),
            post_defaults,
        )
    }

    pub fn parse_from_with_defaults(
        &self,
        term: &GenTerm,
        initial_env: Env,
        draws: &mut dyn DrawSource,
        post_defaults: &DefaultTypes,
    ) -> Result<ParseOutput, ReconstructError> {
        self.parse_from_checks_with_defaults(
            term,
            initial_env,
            draws,
            Checks::Checked,
            post_defaults,
        )
    }

    pub fn parse_trusted_from_with_defaults(
        &self,
        term: &GenTerm,
        initial_env: Env,
        draws: &mut dyn DrawSource,
        post_defaults: &DefaultTypes,
    ) -> Result<ParseOutput, ReconstructError> {
        self.parse_from_checks_with_defaults(
            term,
            initial_env,
            draws,
            Checks::Trusted,
            post_defaults,
        )
    }

    pub fn parse_candidate_from_with_defaults(
        &self,
        term: &GenTerm,
        initial_env: Env,
        draws: &mut dyn DrawSource,
        post_defaults: &DefaultTypes,
    ) -> Result<ParseOutput, ReconstructError> {
        self.parse_from_checks_with_defaults(
            term,
            initial_env,
            draws,
            Checks::Candidate,
            post_defaults,
        )
    }

    fn parse_from_checks_with_defaults(
        &self,
        term: &GenTerm,
        initial_env: Env,
        draws: &mut dyn DrawSource,
        checks: Checks,
        post_defaults: &DefaultTypes,
    ) -> Result<ParseOutput, ReconstructError> {
        let evaluator = Evaluator::decoding(self.0.ctx, draws, post_defaults);
        term.rebuild_with_evaluator(
            self.0.ctx,
            Memory::Incoming(self.0.incoming),
            checks,
            &mut Recorder::Untracked,
            initial_env,
            evaluator,
        )
    }

    pub fn parse_from(
        &self,
        term: &GenTerm,
        initial_env: Env,
        draws: &mut dyn DrawSource,
    ) -> Result<ParseOutput, ReconstructError> {
        self.parse_from_checks(term, initial_env, draws, Checks::Checked)
    }

    pub fn parse_trusted_from(
        &self,
        term: &GenTerm,
        initial_env: Env,
        draws: &mut dyn DrawSource,
    ) -> Result<ParseOutput, ReconstructError> {
        self.parse_from_checks(term, initial_env, draws, Checks::Trusted)
    }

    pub fn parse_candidate_from(
        &self,
        term: &GenTerm,
        initial_env: Env,
        draws: &mut dyn DrawSource,
    ) -> Result<ParseOutput, ReconstructError> {
        self.parse_from_checks(term, initial_env, draws, Checks::Candidate)
    }

    fn parse_from_checks(
        &self,
        term: &GenTerm,
        initial_env: Env,
        draws: &mut dyn DrawSource,
        checks: Checks,
    ) -> Result<ParseOutput, ReconstructError> {
        term.rebuild_with(
            self.0.ctx,
            Memory::Incoming(self.0.incoming),
            draws,
            checks,
            &mut Recorder::Untracked,
            initial_env,
        )
    }
}

impl GenTerm {
    pub fn translate(&self, tree: &ChoiceTree) -> Result<ChoiceTape, TranslateError> {
        self.translate_in(&GenModule::default(), tree)
    }

    pub fn translate_in(
        &self,
        ctx: &GenModule,
        tree: &ChoiceTree,
    ) -> Result<ChoiceTape, TranslateError> {
        self.walk_root(ctx, Memory::Heap(None), tree, Checks::Skipped, Env::new())
    }

    pub fn translate_with_incoming(
        &self,
        ctx: &GenModule,
        incoming: &Incoming,
        tree: &ChoiceTree,
    ) -> Result<ChoiceTape, TranslateError> {
        self.walk_root(
            ctx,
            Memory::Incoming(incoming),
            tree,
            Checks::Skipped,
            Env::new(),
        )
    }

    pub fn validate(&self, tree: &ChoiceTree) -> Result<(), TranslateError> {
        self.validate_in(&GenModule::default(), None, tree)
    }

    pub fn validate_in(
        &self,
        ctx: &GenModule,
        heap: Option<Region>,
        tree: &ChoiceTree,
    ) -> Result<(), TranslateError> {
        self.walk_root(ctx, Memory::Heap(heap), tree, Checks::Checked, Env::new())
            .map(drop)
    }

    pub fn validate_with_incoming(
        &self,
        ctx: &GenModule,
        incoming: &Incoming,
        tree: &ChoiceTree,
    ) -> Result<(), TranslateError> {
        self.walk_root(
            ctx,
            Memory::Incoming(incoming),
            tree,
            Checks::Checked,
            Env::new(),
        )
        .map(drop)
    }

    pub fn validate_with_evaluator(
        &self,
        ctx: &GenModule,
        incoming: &Incoming,
        tree: &ChoiceTree,
        evaluator: &Evaluator<'_>,
    ) -> Result<(), TranslateError> {
        self.walk_root_with_evaluator(
            ctx,
            Memory::Incoming(incoming),
            tree,
            Checks::Checked,
            Env::new(),
            evaluator,
        )
        .map(drop)
    }

    pub fn reconstruct(&self, tape: &[u8]) -> Result<ChoiceTree, ReconstructError> {
        self.reconstruct_in(&GenModule::default(), tape)
    }

    pub fn reconstruct_in(
        &self,
        ctx: &GenModule,
        tape: &[u8],
    ) -> Result<ChoiceTree, ReconstructError> {
        self.reconstruct_with(ctx, Memory::Heap(None), tape, Env::new())
    }

    fn reconstruct_with(
        &self,
        ctx: &GenModule,
        memory: Memory<'_>,
        tape: &[u8],
        env: Env,
    ) -> Result<ChoiceTree, ReconstructError> {
        let mut root = Scope::new();
        let parsed = self.walk_tape_output(
            ctx,
            memory,
            tape,
            Checks::Skipped,
            &mut Recorder::Tree { scope: &mut root },
            env,
        )?;
        Ok(ChoiceTree::new(root).with_defaults(parsed.defaults))
    }

    pub fn validate_tape(&self, tape: &[u8]) -> Result<(), ReconstructError> {
        self.validate_tape_in(&GenModule::default(), None, tape)
    }

    pub fn validate_tape_in(
        &self,
        ctx: &GenModule,
        heap: Option<Region>,
        tape: &[u8],
    ) -> Result<(), ReconstructError> {
        self.walk_tape(
            ctx,
            Memory::Heap(heap),
            tape,
            Checks::Checked,
            &mut Recorder::Untracked,
            Env::new(),
        )
    }

    pub fn validate_tape_with_incoming(
        &self,
        ctx: &GenModule,
        incoming: &Incoming,
        tape: &[u8],
    ) -> Result<(), ReconstructError> {
        self.walk_tape(
            ctx,
            Memory::Incoming(incoming),
            tape,
            Checks::Checked,
            &mut Recorder::Untracked,
            Env::new(),
        )
    }

    fn walk_tape(
        &self,
        ctx: &GenModule,
        memory: Memory<'_>,
        tape: &[u8],
        checks: Checks,
        rec: &mut Recorder<'_>,
        env: Env,
    ) -> Result<(), ReconstructError> {
        self.walk_tape_output(ctx, memory, tape, checks, rec, env)
            .map(drop)
    }

    fn walk_tape_output(
        &self,
        ctx: &GenModule,
        memory: Memory<'_>,
        tape: &[u8],
        checks: Checks,
        rec: &mut Recorder<'_>,
        env: Env,
    ) -> Result<ParseOutput, ReconstructError> {
        let mut source = DrawTape::new(tape);
        let output = self.rebuild_with(ctx, memory, &mut source, checks, rec, env)?;
        match tape.len() - source.consumed() {
            0 => Ok(output),
            count => Err(ReconstructError::TrailingBytes { count }),
        }
    }

    pub fn parse_in(
        &self,
        ctx: &GenModule,
        heap: Option<Region>,
        tape: &[u8],
    ) -> Result<ParseOutput, ReconstructError> {
        self.parse_from(ctx, heap, &mut endless(DrawTape::new(tape)))
    }

    pub fn parse_with_incoming(
        &self,
        ctx: &GenModule,
        incoming: &Incoming,
        tape: &[u8],
    ) -> Result<ParseOutput, ReconstructError> {
        self.parse_from_with_incoming(ctx, incoming, &mut endless(DrawTape::new(tape)))
    }

    pub fn parse_from(
        &self,
        ctx: &GenModule,
        heap: Option<Region>,
        draws: &mut dyn DrawSource,
    ) -> Result<ParseOutput, ReconstructError> {
        self.rebuild_with(
            ctx,
            Memory::Heap(heap),
            draws,
            Checks::Checked,
            &mut Recorder::Untracked,
            Env::new(),
        )
    }

    pub fn parse_from_with_incoming(
        &self,
        ctx: &GenModule,
        incoming: &Incoming,
        draws: &mut dyn DrawSource,
    ) -> Result<ParseOutput, ReconstructError> {
        self.rebuild_with(
            ctx,
            Memory::Incoming(incoming),
            draws,
            Checks::Checked,
            &mut Recorder::Untracked,
            Env::new(),
        )
    }

    pub fn parse_candidate_from(
        &self,
        ctx: &GenModule,
        heap: Option<Region>,
        draws: &mut dyn DrawSource,
    ) -> Result<ParseOutput, ReconstructError> {
        self.rebuild_with(
            ctx,
            Memory::Heap(heap),
            draws,
            Checks::Candidate,
            &mut Recorder::Untracked,
            Env::new(),
        )
    }

    pub fn parse_trusted_from(
        &self,
        ctx: &GenModule,
        draws: &mut dyn DrawSource,
    ) -> Result<ParseOutput, ReconstructError> {
        self.rebuild_with(
            ctx,
            Memory::Heap(None),
            draws,
            Checks::Trusted,
            &mut Recorder::Untracked,
            Env::new(),
        )
    }

    pub fn parse_trusted_from_with_incoming(
        &self,
        ctx: &GenModule,
        incoming: &Incoming,
        draws: &mut dyn DrawSource,
    ) -> Result<ParseOutput, ReconstructError> {
        self.rebuild_with(
            ctx,
            Memory::Incoming(incoming),
            draws,
            Checks::Trusted,
            &mut Recorder::Untracked,
            Env::new(),
        )
    }

    fn walk_root(
        &self,
        ctx: &GenModule,
        memory: Memory<'_>,
        tree: &ChoiceTree,
        checks: Checks,
        env: Env,
    ) -> Result<ChoiceTape, TranslateError> {
        let evaluator = Evaluator::sampled(ctx, TestCaseSize::MAX, 0, &DefaultTypes::new())?;
        self.walk_root_with_evaluator(ctx, memory, tree, checks, env, &evaluator)
    }

    fn walk_root_with_evaluator(
        &self,
        ctx: &GenModule,
        memory: Memory<'_>,
        tree: &ChoiceTree,
        checks: Checks,
        env: Env,
        evaluator: &Evaluator<'_>,
    ) -> Result<ChoiceTape, TranslateError> {
        let mut root = tree.root.clone();
        let evaluator = evaluator.encoding(ctx, &tree.defaults)?;
        let mut env = env;
        memory.bind_globals(&mut env);
        let mut writes = WriteLog::default();
        let frame = Frame {
            ctx,
            evaluator: &evaluator,
            memory,
            depth: 0,
            checks,
        };
        let result = walk(self, frame, &mut root, &mut env, &mut writes);
        if let Some(error) = evaluator.default_error() {
            return Err(error.into());
        }
        result?;
        check_drained(&root)?;
        Ok(evaluator.finish_encoding()?)
    }

    pub fn tape_from_values_in(
        &self,
        ctx: &GenModule,
        heap: Option<Region>,
        values: &[i128],
    ) -> Result<ChoiceTape, ReconstructError> {
        self.tape_from_values_with(ctx, Memory::Heap(heap), values, Env::new())
    }

    pub fn tape_from_values_with_incoming(
        &self,
        ctx: &GenModule,
        incoming: &Incoming,
        values: &[i128],
    ) -> Result<ChoiceTape, ReconstructError> {
        self.tape_from_values_with(ctx, Memory::Incoming(incoming), values, Env::new())
    }

    pub fn candidate_tape_from_values_in(
        &self,
        ctx: &GenModule,
        heap: Option<Region>,
        values: &[i128],
    ) -> Result<ChoiceTape, ReconstructError> {
        self.candidate_tape_from_values_with(ctx, Memory::Heap(heap), values, Env::new())
    }

    fn tape_from_values_with(
        &self,
        ctx: &GenModule,
        memory: Memory<'_>,
        values: &[i128],
        env: Env,
    ) -> Result<ChoiceTape, ReconstructError> {
        self.tape_from_values_with_checks(ctx, memory, values, env, Checks::Checked)
    }

    fn candidate_tape_from_values_with(
        &self,
        ctx: &GenModule,
        memory: Memory<'_>,
        values: &[i128],
        env: Env,
    ) -> Result<ChoiceTape, ReconstructError> {
        self.tape_from_values_with_checks(ctx, memory, values, env, Checks::Candidate)
    }

    fn tape_from_values_with_checks(
        &self,
        ctx: &GenModule,
        memory: Memory<'_>,
        values: &[i128],
        env: Env,
        checks: Checks,
    ) -> Result<ChoiceTape, ReconstructError> {
        let mut source = Blind(of_values(values));
        let mut drawn = ChoiceTape::new();
        let mut recording = RecordingDraws {
            source: &mut source,
            tape: &mut drawn,
        };
        self.rebuild_with(
            ctx,
            memory,
            &mut recording,
            checks,
            &mut Recorder::Untracked,
            env,
        )?;
        check_source_drained(source.0)?;
        Ok(drawn)
    }

    fn rebuild_with(
        &self,
        ctx: &GenModule,
        memory: Memory<'_>,
        draws: &mut dyn DrawSource,
        checks: Checks,
        rec: &mut Recorder<'_>,
        env: Env,
    ) -> Result<ParseOutput, ReconstructError> {
        let evaluator = Evaluator::decoding(ctx, draws, &DefaultTypes::new());
        self.rebuild_with_evaluator(ctx, memory, checks, rec, env, evaluator)
    }

    fn rebuild_with_evaluator(
        &self,
        ctx: &GenModule,
        memory: Memory<'_>,
        checks: Checks,
        rec: &mut Recorder<'_>,
        env: Env,
        evaluator: Evaluator<'_>,
    ) -> Result<ParseOutput, ReconstructError> {
        let mut env = env;
        memory.bind_globals(&mut env);
        let mut writes = WriteLog::default();
        let frame = Frame {
            ctx,
            evaluator: &evaluator,
            memory,
            depth: 0,
            checks,
        };
        let result = rebuild(self, frame, rec, &mut env, &mut writes);
        if let Some(error) = evaluator.default_error() {
            return Err(error.into());
        }
        let ret = result?;
        let defaults = evaluator.finish_decoding()?;
        writes.allocation_roots.sort_unstable();
        writes.allocation_roots.dedup();
        Ok(ParseOutput {
            defaults,
            ret,
            writes: writes.rendered,
            env: env.into_bindings(),
            allocation_roots: writes.allocation_roots,
        })
    }
}

pub fn bind_size_args(env: &mut Env, def: &GenDef, size: TestCaseSize) {
    let BaseType::Bits(sign, width) = crate::gen_dsl::uintptr_bt() else {
        unreachable!("size arguments are declared at uintptr_bt()")
    };
    for sym in def.size_args() {
        let scalar = Scalar::bits(sign, width, i128::from(size.get()))
            .expect("a TestCaseSize fits the size arguments' width");
        env.insert(sym, Value::Scalar(scalar));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SizeArgError {
    NotDeclared(Sym),
    CollidesWithGlobal(Sym),
    WrongType(Sym),
}

pub fn validate_size_args(def: &GenDef, module: &GenModule) -> Result<(), SizeArgError> {
    let size_bt = crate::gen_dsl::uintptr_bt();
    for size in def.size_args() {
        if module.find_global(size).is_some() {
            return Err(SizeArgError::CollidesWithGlobal(size.clone()));
        }
        let mut declared = false;
        for (_, bt) in def.iargs.iter().filter(|(param, _)| param == size) {
            declared = true;
            if bt != &size_bt {
                return Err(SizeArgError::WrongType(size.clone()));
            }
        }
        if !declared {
            return Err(SizeArgError::NotDeclared(size.clone()));
        }
    }
    Ok(())
}

pub fn spent_rec_size(def: &GenDef, callee_env: &Env) -> bool {
    def.rec_size_arg
        .as_ref()
        .is_some_and(|sym| callee_env.get(sym).and_then(Value::as_int) == Some(0))
}

pub enum RootCallError<'a> {
    Structural(GenCallError),
    CallArg(&'a IndexTerm),
    RecursionBudget,
}

pub fn enter_root<'a>(
    term: &'a GenTerm,
    ctx: &'a GenModule,
    incoming: &Incoming,
) -> Result<(&'a GenTerm, Env), RootCallError<'a>> {
    enter_root_with_evaluator(term, ctx, incoming, &Evaluator::new(ctx))
}

pub fn enter_root_with_evaluator<'a>(
    term: &'a GenTerm,
    ctx: &'a GenModule,
    incoming: &Incoming,
    evaluator: &Evaluator<'_>,
) -> Result<(&'a GenTerm, Env), RootCallError<'a>> {
    let GenTermKind::Call { fsym, args } = &term.term else {
        let mut env = Env::new();
        incoming.bind_globals(&mut env);
        return Ok((term, env));
    };

    let def = ctx
        .resolve_call(fsym, args.len())
        .map_err(RootCallError::Structural)?;
    let mut callee_env = Env::new();
    let mut root_env = Env::new();
    incoming.bind_globals(&mut root_env);
    for ((param, _), arg) in def.iargs.iter().zip(args) {
        let Some(value) = evaluator.eval(arg, &root_env) else {
            return Err(RootCallError::CallArg(arg));
        };
        callee_env.insert(param, value);
    }
    if spent_rec_size(def, &callee_env) {
        return Err(RootCallError::RecursionBudget);
    }
    Ok((&def.body, callee_env))
}

impl GenDef {
    fn root_env(&self) -> Env {
        let mut env = Env::new();
        bind_size_args(&mut env, self, TestCaseSize::MAX);
        env
    }

    pub fn translate(&self, tree: &ChoiceTree) -> Result<ChoiceTape, TranslateError> {
        self.translate_in(&GenModule::default(), tree)
    }

    pub fn translate_in(
        &self,
        ctx: &GenModule,
        tree: &ChoiceTree,
    ) -> Result<ChoiceTape, TranslateError> {
        self.body.walk_root(
            ctx,
            Memory::Heap(None),
            tree,
            Checks::Skipped,
            self.root_env(),
        )
    }

    pub fn translate_with_incoming(
        &self,
        ctx: &GenModule,
        incoming: &Incoming,
        tree: &ChoiceTree,
    ) -> Result<ChoiceTape, TranslateError> {
        self.body.walk_root(
            ctx,
            Memory::Incoming(incoming),
            tree,
            Checks::Skipped,
            self.root_env(),
        )
    }

    pub fn reconstruct(&self, tape: &[u8]) -> Result<ChoiceTree, ReconstructError> {
        self.reconstruct_in(&GenModule::default(), tape)
    }

    pub fn reconstruct_in(
        &self,
        ctx: &GenModule,
        tape: &[u8],
    ) -> Result<ChoiceTree, ReconstructError> {
        self.body
            .reconstruct_with(ctx, Memory::Heap(None), tape, self.root_env())
    }

    pub fn validate(&self, tree: &ChoiceTree) -> Result<(), TranslateError> {
        self.validate_in(&GenModule::default(), None, tree)
    }

    pub fn validate_in(
        &self,
        ctx: &GenModule,
        heap: Option<Region>,
        tree: &ChoiceTree,
    ) -> Result<(), TranslateError> {
        self.body
            .walk_root(
                ctx,
                Memory::Heap(heap),
                tree,
                Checks::Checked,
                self.root_env(),
            )
            .map(drop)
    }

    pub fn validate_with_incoming(
        &self,
        ctx: &GenModule,
        incoming: &Incoming,
        tree: &ChoiceTree,
    ) -> Result<(), TranslateError> {
        self.body
            .walk_root(
                ctx,
                Memory::Incoming(incoming),
                tree,
                Checks::Checked,
                self.root_env(),
            )
            .map(drop)
    }

    pub fn validate_tape(&self, tape: &[u8]) -> Result<(), ReconstructError> {
        self.validate_tape_in(&GenModule::default(), None, tape)
    }

    pub fn validate_tape_in(
        &self,
        ctx: &GenModule,
        heap: Option<Region>,
        tape: &[u8],
    ) -> Result<(), ReconstructError> {
        self.body.walk_tape(
            ctx,
            Memory::Heap(heap),
            tape,
            Checks::Checked,
            &mut Recorder::Untracked,
            self.root_env(),
        )
    }

    pub fn validate_tape_with_incoming(
        &self,
        ctx: &GenModule,
        incoming: &Incoming,
        tape: &[u8],
    ) -> Result<(), ReconstructError> {
        self.body.walk_tape(
            ctx,
            Memory::Incoming(incoming),
            tape,
            Checks::Checked,
            &mut Recorder::Untracked,
            self.root_env(),
        )
    }

    pub fn tape_from_values_in(
        &self,
        ctx: &GenModule,
        heap: Option<Region>,
        values: &[i128],
    ) -> Result<ChoiceTape, ReconstructError> {
        self.body
            .tape_from_values_with(ctx, Memory::Heap(heap), values, self.root_env())
    }

    pub fn tape_from_values_with_incoming(
        &self,
        ctx: &GenModule,
        incoming: &Incoming,
        values: &[i128],
    ) -> Result<ChoiceTape, ReconstructError> {
        self.body
            .tape_from_values_with(ctx, Memory::Incoming(incoming), values, self.root_env())
    }

    pub fn candidate_tape_from_values_with_incoming(
        &self,
        ctx: &GenModule,
        incoming: &Incoming,
        values: &[i128],
    ) -> Result<ChoiceTape, ReconstructError> {
        self.body.candidate_tape_from_values_with(
            ctx,
            Memory::Incoming(incoming),
            values,
            self.root_env(),
        )
    }

    pub fn parse_in(
        &self,
        ctx: &GenModule,
        heap: Option<Region>,
        tape: &[u8],
    ) -> Result<ParseOutput, ReconstructError> {
        self.parse_from(ctx, heap, &mut endless(DrawTape::new(tape)))
    }

    pub fn parse_with_incoming(
        &self,
        ctx: &GenModule,
        incoming: &Incoming,
        tape: &[u8],
    ) -> Result<ParseOutput, ReconstructError> {
        self.parse_from_with_incoming(ctx, incoming, &mut endless(DrawTape::new(tape)))
    }

    pub fn parse_from(
        &self,
        ctx: &GenModule,
        heap: Option<Region>,
        draws: &mut dyn DrawSource,
    ) -> Result<ParseOutput, ReconstructError> {
        self.body.rebuild_with(
            ctx,
            Memory::Heap(heap),
            draws,
            Checks::Checked,
            &mut Recorder::Untracked,
            self.root_env(),
        )
    }

    pub fn parse_from_with_incoming(
        &self,
        ctx: &GenModule,
        incoming: &Incoming,
        draws: &mut dyn DrawSource,
    ) -> Result<ParseOutput, ReconstructError> {
        self.body.rebuild_with(
            ctx,
            Memory::Incoming(incoming),
            draws,
            Checks::Checked,
            &mut Recorder::Untracked,
            self.root_env(),
        )
    }

    pub fn parse_candidate_from_with_incoming(
        &self,
        ctx: &GenModule,
        incoming: &Incoming,
        draws: &mut dyn DrawSource,
    ) -> Result<ParseOutput, ReconstructError> {
        self.body.rebuild_with(
            ctx,
            Memory::Incoming(incoming),
            draws,
            Checks::Candidate,
            &mut Recorder::Untracked,
            self.root_env(),
        )
    }

    pub fn parse_trusted_from(
        &self,
        ctx: &GenModule,
        draws: &mut dyn DrawSource,
    ) -> Result<ParseOutput, ReconstructError> {
        self.body.rebuild_with(
            ctx,
            Memory::Heap(None),
            draws,
            Checks::Trusted,
            &mut Recorder::Untracked,
            self.root_env(),
        )
    }

    pub fn parse_trusted_from_with_incoming(
        &self,
        ctx: &GenModule,
        incoming: &Incoming,
        draws: &mut dyn DrawSource,
    ) -> Result<ParseOutput, ReconstructError> {
        self.body.rebuild_with(
            ctx,
            Memory::Incoming(incoming),
            draws,
            Checks::Trusted,
            &mut Recorder::Untracked,
            self.root_env(),
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Checks {
    Checked,
    Candidate,
    Trusted,
    Skipped,
}

impl Checks {
    fn asserts(self) -> bool {
        matches!(self, Checks::Checked)
    }

    fn asgn_guards(self) -> bool {
        matches!(self, Checks::Checked)
    }

    fn renders(self) -> bool {
        !matches!(self, Checks::Skipped)
    }
}

#[derive(Clone, Copy)]
enum Memory<'a> {
    Heap(Option<Region>),
    Incoming(&'a Incoming),
}

impl<'a> Memory<'a> {
    fn bind_globals(self, env: &mut Env) {
        match self {
            Memory::Heap(_) => {}
            Memory::Incoming(incoming) => incoming.bind_globals(env),
        }
    }
}

#[derive(Clone, Copy)]
struct Frame<'a, 'e> {
    ctx: &'a GenModule,
    evaluator: &'a Evaluator<'e>,
    memory: Memory<'a>,
    depth: usize,
    checks: Checks,
}

impl Frame<'_, '_> {
    fn deeper(self) -> Self {
        Self {
            depth: self.depth + 1,
            ..self
        }
    }
}

#[derive(Default)]
struct WriteLog {
    extents: MarkedHeap,
    rendered: Vec<AsgnWrite>,
    allocation_roots: Vec<u64>,
}

fn collect_allocation_roots(value: &Value, roots: &mut Vec<u64>) {
    roots.extend(value.scalars().filter_map(|scalar| scalar.heap_address()));
}

enum Recorder<'a> {
    Tree { scope: &'a mut Scope },
    Untracked,
}

struct RecordingDraws<'a> {
    source: &'a mut dyn DrawSource,
    tape: &'a mut ChoiceTape,
}

impl DrawSource for RecordingDraws<'_> {
    fn draw(&mut self, site: Site) -> Option<Draw> {
        let draw = self.source.draw(site)?;
        let bv = draw.at(site.width);
        self.tape.push(bv.value, bv.bits);
        Some(draw)
    }
}

impl Recorder<'_> {
    fn record_draw(&mut self, label: Label, bv: Bv, bits: u8) {
        self.record(
            label,
            Choice::Scalar {
                value: bv.value,
                bits,
            },
        );
    }

    fn record(&mut self, label: Label, choice: Choice) {
        match self {
            Self::Tree { scope } => scope.insert(label, choice),
            Self::Untracked => {}
        }
    }
}

fn walk(
    term: &GenTerm,
    frame: Frame<'_, '_>,
    scope: &mut Scope,
    env: &mut Env,
    writes: &mut WriteLog,
) -> Result<Option<Value>, TranslateError> {
    match &term.term {
        GenTermKind::Arbitrary => Ok(None),
        GenTermKind::LetStar { var, value, rest } => {
            if let Some((label, after_write)) = fused_write(var, value, rest) {
                if let Some(ct) = site_ctype(&value.bt) {
                    let v = take_tree_aggregate(label, &ct, scope, frame.ctx, frame.evaluator)?;
                    env.insert(var, v);
                } else {
                    let raw = take_tree_choice(label, &value.bt, scope, frame.evaluator)?;
                    if let Some(v) = scalar_value(raw, &value.bt) {
                        env.insert(var, v);
                    }
                }
                return walk(after_write, frame, scope, env, writes);
            }
            if let Some(v) = walk(value, frame, scope, env, writes)? {
                env.insert(var, v);
            }
            walk(rest, frame, scope, env, writes)
        }
        GenTermKind::WriteChoice { label, value, rest } => {
            if let Some(ct) = site_ctype(&value.bt) {
                let expected =
                    frame
                        .evaluator
                        .eval(value, env)
                        .ok_or(TranslateError::Unevaluatable {
                            context: "choice-write value",
                        })?;
                let actual = take_tree_aggregate(*label, &ct, scope, frame.ctx, frame.evaluator)?;
                if actual != expected {
                    return Err(TranslateError::StructChoice { label: *label });
                }
                return walk(rest, frame, scope, env, writes);
            }
            scalar_width(&value.bt).ok_or_else(|| TranslateError::UnsupportedBaseType {
                bt: value.bt.clone(),
            })?;
            let expected = frame
                .evaluator
                .eval(value, env)
                .and_then(|v| choice_value(&v, &value.bt))
                .ok_or(TranslateError::Unevaluatable {
                    context: "choice-write value",
                })?;
            let actual = take_tree_choice(*label, &value.bt, scope, frame.evaluator)?;
            if actual != expected {
                return Err(TranslateError::ChoiceMismatch {
                    label: *label,
                    expected,
                    actual,
                });
            }
            walk(rest, frame, scope, env, writes)
        }
        GenTermKind::Return(it) => Ok(frame.evaluator.eval(it, env)),
        GenTermKind::Assert { constraint, rest } => {
            let verdict =
                check_constraint_with_evaluator(constraint, env, frame.ctx, frame.evaluator);
            if frame.checks.asserts() {
                verdict?;
            }
            walk(rest, frame, scope, env, writes)
        }
        GenTermKind::Scope { name, body } => {
            let key = name.eval_key_with_evaluator(env, frame.evaluator);
            if let Some(mut child) = key.as_ref().and_then(|key| scope.scopes.remove(key)) {
                let v = walk(body, frame, &mut child, env, writes)?;
                check_drained(&child)?;
                return Ok(v);
            }

            let mut empty = Scope::new();
            match walk(body, frame, &mut empty, env, writes) {
                Ok(v) => {
                    check_drained(&empty)?;
                    Ok(v)
                }
                Err(TranslateError::MissingChoice { .. } | TranslateError::MissingScope { .. }) => {
                    Err(match key {
                        Some(key) => TranslateError::MissingScope { key },
                        None => TranslateError::Unevaluatable {
                            context: "scope index",
                        },
                    })
                }
                Err(error) => Err(error),
            }
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            let Some(c) = frame.evaluator.eval(cond, env).and_then(|v| v.as_bool()) else {
                return Err(TranslateError::Unevaluatable {
                    context: "ite condition",
                });
            };
            walk(if c { then_ } else { else_ }, frame, scope, env, writes)
        }
        GenTermKind::Call { fsym, args } => {
            let def = frame.ctx.resolve_call(fsym, args.len())?;
            if frame.depth >= MAX_CALL_DEPTH {
                return Err(TranslateError::CallDepthExceeded { fsym: fsym.clone() });
            }
            let mut callee_env = Env::new();
            for ((param, _), arg) in def.iargs.iter().zip(args) {
                let v = frame
                    .evaluator
                    .eval(arg, env)
                    .ok_or(TranslateError::Unevaluatable {
                        context: "call argument",
                    })?;
                callee_env.insert(param, v);
            }
            walk(&def.body, frame.deeper(), scope, &mut callee_env, writes)
        }
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => {
            let mut entries = Vec::new();
            let candidates = map_candidate_range_with_evaluator(
                perm,
                var,
                var_bt,
                env,
                frame.ctx,
                frame.evaluator,
            )?;
            for i in candidates.range.clone() {
                let v = env.scoped(var, index_scalar(var_bt, i), |env| {
                    if candidates.holds_at_with_evaluator(perm, env, frame.ctx, frame.evaluator)? {
                        walk(body, frame, scope, env, writes)
                    } else {
                        Ok(None)
                    }
                })?;
                if let Some(v) = v {
                    entries.push((i, v));
                }
            }
            Ok(Some(Value::iteration_map(entries)))
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => {
            if frame.checks.renders() {
                let w = if frame.checks.asgn_guards() {
                    check_asgn_with_memory_with_evaluator(
                        addr,
                        ct,
                        value,
                        env,
                        frame.memory,
                        &mut writes.extents,
                        frame.ctx,
                        frame.evaluator,
                    )?
                } else {
                    render_asgn_with_evaluator(addr, ct, value, env, frame.ctx, frame.evaluator)?
                };
                if !w.bytes.is_empty() {
                    writes.rendered.push(w);
                }
            } else {
                frame.evaluator.eval(addr, env);
                frame.evaluator.eval(value, env);
            }
            walk(rest, frame, scope, env, writes)
        }
    }
}

fn take_tree_choice(
    label: Label,
    bt: &BaseType,
    scope: &mut Scope,
    evaluator: &Evaluator<'_>,
) -> Result<i128, TranslateError> {
    let width =
        scalar_width(bt).ok_or_else(|| TranslateError::UnsupportedBaseType { bt: bt.clone() })?;
    let choice = scope
        .choices
        .remove(&label)
        .ok_or(TranslateError::MissingChoice { label })?;
    let Choice::Scalar {
        value: choice_value,
        bits: choice_bits,
    } = choice
    else {
        return Err(TranslateError::WidthMismatch {
            label,
            expected: width,
            actual: 0,
        });
    };
    let choice = Bv {
        value: choice_value,
        bits: u16::from(choice_bits),
    };
    if choice.bits != width {
        return Err(TranslateError::WidthMismatch {
            label,
            expected: width,
            actual: choice.bits,
        });
    }
    if !fits_unsigned(choice.value, width) {
        return Err(TranslateError::ValueOutOfRange {
            label,
            value: choice.value,
            bits: width,
        });
    }
    evaluator.push_choice(choice.value, width);
    Ok(choice.value)
}

fn check_drained(scope: &Scope) -> Result<(), TranslateError> {
    if let Some(label) = scope.choices.keys().next() {
        return Err(TranslateError::UnusedChoice { label: *label });
    }
    if let Some(key) = scope
        .scopes
        .iter()
        .find_map(|(key, child)| (!child.is_empty()).then_some(key))
    {
        return Err(TranslateError::UnusedScope { key: key.clone() });
    }
    Ok(())
}

fn check_source_drained(source: impl Iterator<Item = Draw>) -> Result<(), ReconstructError> {
    match source.count() {
        0 => Ok(()),
        count => Err(ReconstructError::TrailingValues { count }),
    }
}

fn rebuild(
    term: &GenTerm,
    frame: Frame<'_, '_>,
    rec: &mut Recorder<'_>,
    env: &mut Env,
    writes: &mut WriteLog,
) -> Result<Option<Value>, ReconstructError> {
    match &term.term {
        GenTermKind::Arbitrary => Ok(None),
        GenTermKind::LetStar { var, value, rest } => {
            if let Some((label, after_write)) = fused_write(var, value, rest) {
                if let Some(ct) = site_ctype(&value.bt) {
                    let v = draw_written_aggregate(label, &ct, frame, rec)?;
                    collect_allocation_roots(&v, &mut writes.allocation_roots);
                    env.insert(var, v);
                } else {
                    let bv = draw_written_choice(label, &value.bt, frame.evaluator, rec)?;
                    if let Some(v) = scalar_value(bv.value, &value.bt) {
                        collect_allocation_roots(&v, &mut writes.allocation_roots);
                        env.insert(var, v);
                    }
                }
                return rebuild(after_write, frame, rec, env, writes);
            }
            if let Some(v) = rebuild(value, frame, rec, env, writes)? {
                env.insert(var, v);
            }
            rebuild(rest, frame, rec, env, writes)
        }
        GenTermKind::WriteChoice { label, value, rest } => {
            if let Some(ct) = site_ctype(&value.bt) {
                let expected =
                    frame
                        .evaluator
                        .eval(value, env)
                        .ok_or(ReconstructError::Unevaluatable {
                            context: "choice-write value",
                        })?;
                let actual = draw_written_aggregate(*label, &ct, frame, rec)?;
                if actual != expected {
                    return Err(ReconstructError::StructChoice { label: *label });
                }
                return rebuild(rest, frame, rec, env, writes);
            }
            scalar_width(&value.bt).ok_or_else(|| ReconstructError::UnsupportedBaseType {
                bt: value.bt.clone(),
            })?;
            let expected = frame
                .evaluator
                .eval(value, env)
                .and_then(|v| choice_value(&v, &value.bt))
                .ok_or(ReconstructError::Unevaluatable {
                    context: "choice-write value",
                })?;
            let actual = draw_written_choice(*label, &value.bt, frame.evaluator, rec)?.value;
            if actual != expected {
                return Err(ReconstructError::ChoiceMismatch {
                    label: *label,
                    expected,
                    actual,
                });
            }
            rebuild(rest, frame, rec, env, writes)
        }
        GenTermKind::Return(it) => Ok(frame.evaluator.eval(it, env)),
        GenTermKind::Assert { constraint, rest } => {
            let verdict =
                check_constraint_with_evaluator(constraint, env, frame.ctx, frame.evaluator);
            if frame.checks.asserts() {
                verdict?;
            }
            rebuild(rest, frame, rec, env, writes)
        }
        GenTermKind::Scope { name, body } => {
            let key = name.eval_key_with_evaluator(env, frame.evaluator);
            let Recorder::Tree { scope } = rec else {
                return rebuild(body, frame, rec, env, writes);
            };
            let mut child = Scope::new();
            let result = rebuild(
                body,
                frame,
                &mut Recorder::Tree { scope: &mut child },
                env,
                writes,
            )?;
            scope.try_attach(key, child).map_err(|error| match error {
                AttachError::MissingKey => ReconstructError::Unevaluatable {
                    context: "scope index",
                },
                AttachError::Merge(error) => error.into(),
            })?;
            Ok(result)
        }
        GenTermKind::Ite { cond, then_, else_ } => {
            let Some(c) = frame.evaluator.eval(cond, env).and_then(|v| v.as_bool()) else {
                return Err(ReconstructError::Unevaluatable {
                    context: "ite condition",
                });
            };
            rebuild(if c { then_ } else { else_ }, frame, rec, env, writes)
        }
        GenTermKind::Call { fsym, args } => {
            let def = frame.ctx.resolve_call(fsym, args.len())?;
            if frame.depth >= MAX_CALL_DEPTH {
                return Err(ReconstructError::CallDepthExceeded { fsym: fsym.clone() });
            }
            let mut callee_env = Env::new();
            for ((param, _), arg) in def.iargs.iter().zip(args) {
                let v = frame
                    .evaluator
                    .eval(arg, env)
                    .ok_or(ReconstructError::Unevaluatable {
                        context: "call argument",
                    })?;
                callee_env.insert(param, v);
            }
            rebuild(&def.body, frame.deeper(), rec, &mut callee_env, writes)
        }
        GenTermKind::Map {
            var,
            var_bt,
            perm,
            body,
        } => {
            let mut entries = Vec::new();
            let candidates = map_candidate_range_with_evaluator(
                perm,
                var,
                var_bt,
                env,
                frame.ctx,
                frame.evaluator,
            )?;
            for i in candidates.range.clone() {
                let v = env.scoped(var, index_scalar(var_bt, i), |env| {
                    if candidates.holds_at_with_evaluator(perm, env, frame.ctx, frame.evaluator)? {
                        rebuild(body, frame, rec, env, writes)
                    } else {
                        Ok(None)
                    }
                })?;
                if let Some(v) = v {
                    entries.push((i, v));
                }
            }
            Ok(Some(Value::iteration_map(entries)))
        }
        GenTermKind::Asgn {
            addr,
            ct,
            value,
            rest,
        } => {
            if frame.checks.renders() {
                let w = if frame.checks.asgn_guards() {
                    check_asgn_with_memory_with_evaluator(
                        addr,
                        ct,
                        value,
                        env,
                        frame.memory,
                        &mut writes.extents,
                        frame.ctx,
                        frame.evaluator,
                    )?
                } else {
                    render_asgn_with_evaluator(addr, ct, value, env, frame.ctx, frame.evaluator)?
                };
                if !w.bytes.is_empty() {
                    writes.rendered.push(w);
                }
            } else {
                frame.evaluator.eval(addr, env);
                frame.evaluator.eval(value, env);
            }
            rebuild(rest, frame, rec, env, writes)
        }
    }
}

fn draw_written_choice(
    label: Label,
    bt: &BaseType,
    evaluator: &Evaluator<'_>,
    rec: &mut Recorder<'_>,
) -> Result<Bv, ReconstructError> {
    let bv = draw_scalar(label, bt, evaluator)?;
    let bits = u8::try_from(bv.bits).map_err(|_| ReconstructError::UnrepresentableWidth {
        label,
        bits: bv.bits,
    })?;
    rec.record_draw(label, bv, bits);
    Ok(bv)
}

fn draw_scalar(
    label: Label,
    bt: &BaseType,
    evaluator: &Evaluator<'_>,
) -> Result<Bv, ReconstructError> {
    let width =
        scalar_width(bt).ok_or_else(|| ReconstructError::UnsupportedBaseType { bt: bt.clone() })?;
    u8::try_from(width)
        .map_err(|_| ReconstructError::UnrepresentableWidth { label, bits: width })?;
    let site = Site::new(width);
    let bv = evaluator
        .draw_choice(site)
        .ok_or(ReconstructError::Truncated { label })?
        .at(width);
    debug_assert_eq!(bv.bits, width, "a source must answer at the site's width");
    if !fits_unsigned(bv.value, width) {
        return Err(ReconstructError::ValueOutOfRange {
            label,
            value: bv.value,
            bits: width,
        });
    }
    Ok(bv)
}

fn site_ctype(bt: &BaseType) -> Option<Sctype> {
    match bt {
        BaseType::Struct(tag) => Some(Sctype::Struct(tag.clone())),
        _ => None,
    }
}

pub(in crate::gen_dsl) fn value_of_choice(
    choice: &Choice,
    ct: &Sctype,
    ctx: &GenModule,
    emit: &mut dyn FnMut(i128, u16),
) -> Option<Value> {
    match ct {
        Sctype::Struct(tag) => {
            let Choice::Struct(members) = choice else {
                return None;
            };
            let decls = &ctx.structs.iter().find(|(name, _)| name == tag)?.1.members;
            if decls.len() != members.len() {
                return None;
            }
            let mut fields = Vec::with_capacity(members.len());
            for ((id, member_ct), member) in decls.iter().zip(members) {
                fields.push((id.clone(), value_of_choice(member, member_ct, ctx, emit)?));
            }
            Some(Value::Struct {
                tag: tag.clone(),
                fields,
            })
        }
        Sctype::Array(elem, Some(len)) => {
            let Choice::Array(elements) = choice else {
                return None;
            };
            if elements.len() != *len {
                return None;
            }
            let entries = elements
                .iter()
                .enumerate()
                .map(|(k, element)| Some((k as i128, value_of_choice(element, elem, ctx, emit)?)))
                .collect::<Option<Vec<_>>>()?;
            Some(Value::iteration_map(entries))
        }
        Sctype::Array(_, None) => None,
        ct => {
            let bt = ct.to_base_type()?;
            let width = scalar_width(&bt)?;
            let (value, bits) = choice.scalar()?;
            if u16::from(bits) != width || !fits_unsigned(value, width) {
                return None;
            }
            emit(value, width);
            scalar_value(value, &bt)
        }
    }
}

pub fn write_choice_of(value: &Value, bt: &BaseType, ctx: &GenModule) -> Option<Choice> {
    match bt {
        BaseType::Struct(tag) => choice_of_value(value, &Sctype::Struct(tag.clone()), ctx),
        bt => {
            let width = scalar_width(bt)?;
            let bits = u8::try_from(width).ok()?;
            Some(Choice::Scalar {
                value: choice_value(value, bt)?,
                bits,
            })
        }
    }
}

pub fn choice_of_value(value: &Value, ct: &Sctype, ctx: &GenModule) -> Option<Choice> {
    match ct {
        Sctype::Struct(tag) => {
            let Value::Struct {
                tag: value_tag,
                fields,
            } = value
            else {
                return None;
            };
            if value_tag != tag {
                return None;
            }
            let decls = &ctx.structs.iter().find(|(name, _)| name == tag)?.1.members;
            let members = decls
                .iter()
                .map(|(id, member_ct)| {
                    let (_, member) = fields.iter().find(|(fid, _)| fid == id)?;
                    choice_of_value(member, member_ct, ctx)
                })
                .collect::<Option<Vec<_>>>()?;
            Some(Choice::Struct(members))
        }
        Sctype::Array(elem, Some(len)) => {
            let elements = (0..*len as i128)
                .map(|k| choice_of_value(value.map_get(k)?, elem, ctx))
                .collect::<Option<Vec<_>>>()?;
            Some(Choice::Array(elements))
        }
        Sctype::Array(_, None) => None,
        ct => {
            let bt = ct.to_base_type()?;
            let width = scalar_width(&bt)?;
            let bits = u8::try_from(width).ok()?;
            let raw = choice_value(value, &bt)?;
            Some(Choice::Scalar { value: raw, bits })
        }
    }
}

fn draw_choice(
    label: Label,
    ct: &Sctype,
    frame: Frame<'_, '_>,
) -> Result<Choice, ReconstructError> {
    match ct {
        Sctype::Struct(tag) => {
            let decls = ctx_members(frame.ctx, tag)
                .ok_or(ReconstructError::StructChoice { label })?
                .to_vec();
            let members = decls
                .iter()
                .map(|(_, member_ct)| draw_choice(label, member_ct, frame))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Choice::Struct(members))
        }
        Sctype::Array(elem, Some(len)) => {
            let elements = (0..*len)
                .map(|_| draw_choice(label, elem, frame))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Choice::Array(elements))
        }
        Sctype::Array(_, None) => Err(ReconstructError::StructChoice { label }),
        ct => {
            let bt = ct
                .to_base_type()
                .ok_or(ReconstructError::StructChoice { label })?;
            let bv = draw_scalar(label, &bt, frame.evaluator)?;
            let bits =
                u8::try_from(bv.bits).map_err(|_| ReconstructError::UnrepresentableWidth {
                    label,
                    bits: bv.bits,
                })?;
            Ok(Choice::Scalar {
                value: bv.value,
                bits,
            })
        }
    }
}

fn ctx_members<'a>(ctx: &'a GenModule, tag: &Sym) -> Option<&'a [(Id, Sctype)]> {
    ctx.structs
        .iter()
        .find(|(name, _)| name == tag)
        .map(|(_, def)| def.members.as_slice())
}

fn take_tree_aggregate(
    label: Label,
    ct: &Sctype,
    scope: &mut Scope,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<Value, TranslateError> {
    let choice = scope
        .choices
        .remove(&label)
        .ok_or(TranslateError::MissingChoice { label })?;
    value_of_choice(&choice, ct, ctx, &mut |value, width| {
        evaluator.push_choice(value, width)
    })
    .ok_or(TranslateError::StructChoice { label })
}

fn draw_written_aggregate(
    label: Label,
    ct: &Sctype,
    frame: Frame<'_, '_>,
    rec: &mut Recorder<'_>,
) -> Result<Value, ReconstructError> {
    let choice = draw_choice(label, ct, frame)?;
    let value = value_of_choice(&choice, ct, frame.ctx, &mut |_, _| {})
        .ok_or(ReconstructError::StructChoice { label })?;
    rec.record(label, choice);
    Ok(value)
}

fn fused_write<'a>(var: &Sym, value: &GenTerm, rest: &'a GenTerm) -> Option<(Label, &'a GenTerm)> {
    if !matches!(value.term, GenTermKind::Arbitrary) {
        return None;
    }
    let GenTermKind::WriteChoice {
        label,
        value: written,
        rest: after_write,
    } = &rest.term
    else {
        return None;
    };
    if !matches!(&written.term, Term::Sym(sym) if sym == var) {
        return None;
    }
    replay_types_compatible(&written.bt, &value.bt).then_some((*label, after_write))
}

fn replay_types_compatible(written: &BaseType, arbitrary: &BaseType) -> bool {
    written == arbitrary || matches!((written, arbitrary), (BaseType::Loc(_), BaseType::Loc(_)))
}

pub fn scalar_width(bt: &BaseType) -> Option<u16> {
    match bt {
        BaseType::Bits(_, width) => Some(*width),
        BaseType::Bool => Some(1),
        BaseType::Loc(_) => Some(64),
        _ => None,
    }
}

pub fn choice_bits(bt: &BaseType) -> Result<u8, GenFailure> {
    let width =
        scalar_width(bt).ok_or_else(|| GenFailure::UnsupportedBaseType { bt: bt.clone() })?;
    u8::try_from(width).map_err(|_| GenFailure::UnrepresentableWidth { bits: width })
}

pub fn evaluated_write_choice(
    value: &IndexTerm,
    env: &SymMap<Value>,
    ctx: &GenModule,
) -> Result<Option<Choice>, GenFailure> {
    evaluated_write_choice_with_evaluator(value, env, ctx, &Evaluator::new(ctx))
}

pub fn evaluated_write_choice_with_evaluator(
    value: &IndexTerm,
    env: &SymMap<Value>,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<Option<Choice>, GenFailure> {
    if !matches!(value.bt, BaseType::Struct(_)) {
        choice_bits(&value.bt)?;
    }
    Ok(evaluator
        .eval_checked(value, env)?
        .as_ref()
        .and_then(|v| write_choice_of(v, &value.bt, ctx)))
}

fn fits_unsigned(value: i128, bits: u16) -> bool {
    if value < 0 {
        return false;
    }
    bits >= 127 || value < (1i128 << bits)
}

pub fn scalar_value(value: i128, bt: &BaseType) -> Option<Value> {
    match bt {
        BaseType::Bool => Some(Value::Scalar(Scalar::Bool(value != 0))),
        BaseType::Bits(sign, width) => Some(Value::Scalar(
            Scalar::bits(*sign, *width, value).unwrap_or_else(|| Scalar::Int(value.into())),
        )),
        BaseType::Loc(_) => u64::try_from(value)
            .ok()
            .map(|addr| Value::Scalar(Scalar::heap_pointer(addr))),
        _ => Some(Value::Scalar(Scalar::Int(value.into()))),
    }
}

pub fn choice_value(value: &Value, bt: &BaseType) -> Option<i128> {
    match (value, bt) {
        (Value::Scalar(Scalar::Bool(b)), BaseType::Bool) => Some(i128::from(*b)),
        (Value::Scalar(Scalar::Bits(value_sign, value_width, n)), BaseType::Bits(sign, width))
            if value_sign == sign && value_width == width =>
        {
            if let Ok(n) = i128::try_from(n)
                && let Some(raw) = normalise_to_range(Sign::Unsigned, *width, n)
            {
                return Some(raw);
            }
            i128::try_from(&normalise_to_range_big(Sign::Unsigned, *width, n)).ok()
        }
        (Value::Scalar(Scalar::Int(n)), BaseType::Bits(_, 0))
            if n.sign() == num_bigint::Sign::NoSign =>
        {
            Some(0)
        }
        (Value::Scalar(Scalar::Pointer { addr, .. }), BaseType::Loc(_)) => Some(i128::from(*addr)),
        _ => None,
    }
}

pub fn index_scalar(bt: &BaseType, i: i128) -> Value {
    Value::Scalar(match bt {
        BaseType::Bits(sign, width) => {
            Scalar::bits(*sign, *width, i).unwrap_or_else(|| Scalar::Int(i.into()))
        }
        _ => Scalar::Int(i.into()),
    })
}

pub fn call_key(fsym: &Sym) -> ScopeKey {
    ScopeKey::Str(match &fsym.name {
        Some(n) => n.clone(),
        None => fsym.num.to_string(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapDomainError {
    Unsupported,
    Unevaluatable,
    TooLarge { lo: i128, hi: i128 },
}

impl From<MapDomainError> for TranslateError {
    fn from(e: MapDomainError) -> Self {
        match e {
            MapDomainError::Unsupported => TranslateError::UnsupportedMapDomain,
            MapDomainError::Unevaluatable => TranslateError::Unevaluatable {
                context: "map bound",
            },
            MapDomainError::TooLarge { lo, hi } => TranslateError::MapDomainTooLarge { lo, hi },
        }
    }
}

impl From<MapDomainError> for ReconstructError {
    fn from(e: MapDomainError) -> Self {
        match e {
            MapDomainError::Unsupported => ReconstructError::UnsupportedMapDomain,
            MapDomainError::Unevaluatable => ReconstructError::Unevaluatable {
                context: "map bound",
            },
            MapDomainError::TooLarge { lo, hi } => ReconstructError::MapDomainTooLarge { lo, hi },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstraintError {
    Failed,
    Unevaluatable,
}

pub fn check_constraint(
    c: &LogicalConstraint,
    env: &SymMap<Value>,
    ctx: &GenModule,
) -> Result<(), ConstraintError> {
    check_constraint_with_evaluator(c, env, ctx, &Evaluator::new(ctx))
}

pub fn check_constraint_with_evaluator(
    c: &LogicalConstraint,
    env: &SymMap<Value>,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<(), ConstraintError> {
    let verdict = |held: Option<bool>| match held {
        Some(true) => Ok(()),
        Some(false) => Err(ConstraintError::Failed),
        None => Err(ConstraintError::Unevaluatable),
    };
    match c {
        LogicalConstraint::T(it) => verdict(evaluator.eval(it, env).and_then(|v| v.as_bool())),
        LogicalConstraint::Forall {
            var,
            permission,
            body,
        } => {
            let candidates = quantifier_candidate_range_with_evaluator(
                permission, &var.0, &var.1, env, ctx, evaluator,
            )
            .map_err(|_| ConstraintError::Unevaluatable)?;
            verdict(evaluator.eval_forall(var, permission, body, candidates.range, env))
        }
    }
}

impl From<ConstraintError> for TranslateError {
    fn from(e: ConstraintError) -> Self {
        match e {
            ConstraintError::Failed => TranslateError::FailedConstraint,
            ConstraintError::Unevaluatable => TranslateError::Unevaluatable {
                context: "assert constraint",
            },
        }
    }
}

impl From<ConstraintError> for ReconstructError {
    fn from(e: ConstraintError) -> Self {
        match e {
            ConstraintError::Failed => ReconstructError::FailedConstraint,
            ConstraintError::Unevaluatable => ReconstructError::Unevaluatable {
                context: "assert constraint",
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsgnError {
    UnsizedCtype { ct: Sctype },
    Unevaluatable { context: &'static str },
    OutOfBounds { addr: u64, size: u64 },
    Overlap { addr: u64, size: u64 },
}

pub fn check_asgn(
    addr: &IndexTerm,
    ct: &Sctype,
    value: &IndexTerm,
    env: &SymMap<Value>,
    bounds: Option<Region>,
    writes: &mut Vec<Region>,
    ctx: &GenModule,
) -> Result<AsgnWrite, AsgnError> {
    check_asgn_with_evaluator(
        addr,
        ct,
        value,
        env,
        bounds,
        writes,
        ctx,
        &Evaluator::new(ctx),
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "Preserves the checked writer seam while sharing the per-input evaluator"
)]
pub fn check_asgn_with_evaluator(
    addr: &IndexTerm,
    ct: &Sctype,
    value: &IndexTerm,
    env: &SymMap<Value>,
    bounds: Option<Region>,
    writes: &mut Vec<Region>,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<AsgnWrite, AsgnError> {
    check_asgn_at_with_evaluator(
        asgn_extent_with_evaluator(addr, ct, env, ctx, evaluator)?,
        ct,
        value,
        env,
        bounds,
        writes,
        ctx,
        evaluator,
    )
}

pub fn check_asgn_incoming(
    addr: &IndexTerm,
    ct: &Sctype,
    value: &IndexTerm,
    env: &SymMap<Value>,
    incoming: &Incoming,
    writes: &mut Vec<Region>,
    ctx: &GenModule,
) -> Result<AsgnWrite, AsgnError> {
    check_asgn_incoming_with_evaluator(
        addr,
        ct,
        value,
        env,
        incoming,
        writes,
        ctx,
        &Evaluator::new(ctx),
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "Preserves the checked writer seam while sharing the per-input evaluator"
)]
pub fn check_asgn_incoming_with_evaluator(
    addr: &IndexTerm,
    ct: &Sctype,
    value: &IndexTerm,
    env: &SymMap<Value>,
    incoming: &Incoming,
    writes: &mut Vec<Region>,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<AsgnWrite, AsgnError> {
    check_asgn_target_incoming_with_evaluator(
        asgn_target_with_evaluator(addr, ct, env, ctx, evaluator)?,
        ct,
        value,
        env,
        incoming,
        writes,
        ctx,
        evaluator,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "Preserves the checked writer seam while sharing the per-input evaluator"
)]
fn check_asgn_with_memory_with_evaluator(
    addr: &IndexTerm,
    ct: &Sctype,
    value: &IndexTerm,
    env: &SymMap<Value>,
    memory: Memory<'_>,
    writes: &mut MarkedHeap,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<AsgnWrite, AsgnError> {
    let target = asgn_target_with_evaluator(addr, ct, env, ctx, evaluator)?;
    let granted = match memory {
        Memory::Heap(bounds) => {
            target.extent.base != 0 && bounds.is_some_and(|b| b.covers(&target.extent))
        }
        Memory::Incoming(incoming) => incoming.grant_write(&target.extent).is_some(),
    };
    check_granted_write(
        target.extent,
        granted,
        ct,
        value,
        env,
        writes,
        ctx,
        evaluator,
    )
}

pub fn asgn_extent(
    addr: &IndexTerm,
    ct: &Sctype,
    env: &SymMap<Value>,
    ctx: &GenModule,
) -> Result<Region, AsgnError> {
    asgn_extent_with_evaluator(addr, ct, env, ctx, &Evaluator::new(ctx))
}

pub fn asgn_extent_with_evaluator(
    addr: &IndexTerm,
    ct: &Sctype,
    env: &SymMap<Value>,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<Region, AsgnError> {
    Ok(asgn_target_with_evaluator(addr, ct, env, ctx, evaluator)?.extent)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AsgnTarget {
    pub extent: Region,
    pub origin: PointerOrigin,
}

pub fn asgn_target(
    addr: &IndexTerm,
    ct: &Sctype,
    env: &SymMap<Value>,
    ctx: &GenModule,
) -> Result<AsgnTarget, AsgnError> {
    asgn_target_with_evaluator(addr, ct, env, ctx, &Evaluator::new(ctx))
}

pub fn asgn_target_with_evaluator(
    addr: &IndexTerm,
    ct: &Sctype,
    env: &SymMap<Value>,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<AsgnTarget, AsgnError> {
    let size = asgn_size(ct, &ctx.structs)?;
    let (base, origin) = asgn_addr(addr, env, ctx, evaluator)?;
    Ok(AsgnTarget {
        extent: Region { base, size },
        origin,
    })
}

pub fn check_asgn_at(
    extent: Region,
    ct: &Sctype,
    value: &IndexTerm,
    env: &SymMap<Value>,
    bounds: Option<Region>,
    writes: &mut Vec<Region>,
    ctx: &GenModule,
) -> Result<AsgnWrite, AsgnError> {
    check_asgn_at_with_evaluator(
        extent,
        ct,
        value,
        env,
        bounds,
        writes,
        ctx,
        &Evaluator::new(ctx),
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "Preserves the checked writer seam while sharing the per-input evaluator"
)]
pub fn check_asgn_at_with_evaluator(
    extent: Region,
    ct: &Sctype,
    value: &IndexTerm,
    env: &SymMap<Value>,
    bounds: Option<Region>,
    writes: &mut Vec<Region>,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<AsgnWrite, AsgnError> {
    let granted = extent.base != 0 && bounds.is_some_and(|b| b.covers(&extent));
    check_granted_write(extent, granted, ct, value, env, writes, ctx, evaluator)
}

pub fn check_asgn_target_incoming(
    target: AsgnTarget,
    ct: &Sctype,
    value: &IndexTerm,
    env: &SymMap<Value>,
    incoming: &Incoming,
    writes: &mut Vec<Region>,
    ctx: &GenModule,
) -> Result<AsgnWrite, AsgnError> {
    check_asgn_target_incoming_with_evaluator(
        target,
        ct,
        value,
        env,
        incoming,
        writes,
        ctx,
        &Evaluator::new(ctx),
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "Preserves the checked writer seam while sharing the per-input evaluator"
)]
pub fn check_asgn_target_incoming_with_evaluator(
    target: AsgnTarget,
    ct: &Sctype,
    value: &IndexTerm,
    env: &SymMap<Value>,
    incoming: &Incoming,
    writes: &mut Vec<Region>,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<AsgnWrite, AsgnError> {
    check_asgn_granted_with_evaluator(
        target,
        incoming.grant_write(&target.extent),
        ct,
        value,
        env,
        writes,
        ctx,
        evaluator,
    )
}

pub fn check_asgn_granted(
    target: AsgnTarget,
    grant: Option<Grant>,
    ct: &Sctype,
    value: &IndexTerm,
    env: &SymMap<Value>,
    writes: &mut Vec<Region>,
    ctx: &GenModule,
) -> Result<AsgnWrite, AsgnError> {
    check_asgn_granted_with_evaluator(
        target,
        grant,
        ct,
        value,
        env,
        writes,
        ctx,
        &Evaluator::new(ctx),
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "Preserves the checked writer seam while sharing the per-input evaluator"
)]
pub fn check_asgn_granted_with_evaluator(
    target: AsgnTarget,
    grant: Option<Grant>,
    ct: &Sctype,
    value: &IndexTerm,
    env: &SymMap<Value>,
    writes: &mut Vec<Region>,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<AsgnWrite, AsgnError> {
    check_granted_write(
        target.extent,
        grant.is_some(),
        ct,
        value,
        env,
        writes,
        ctx,
        evaluator,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "One checked writer serves existing vector ledgers and the shared wire ledger"
)]
fn check_granted_write(
    extent: Region,
    granted: bool,
    ct: &Sctype,
    value: &IndexTerm,
    env: &SymMap<Value>,
    writes: &mut impl WriteExtents,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<AsgnWrite, AsgnError> {
    let Region { base: addr, size } = extent;
    if addr == 0 || (size > 0 && !granted) {
        return Err(AsgnError::OutOfBounds { addr, size });
    }
    if size > 0 && writes.overlaps(&extent) {
        return Err(AsgnError::Overlap { addr, size });
    }
    let write = rendered_write(addr, size, ct, value, env, ctx, evaluator)?;
    if size > 0 {
        writes.record(extent);
    }
    Ok(write)
}

trait WriteExtents {
    fn overlaps(&self, extent: &Region) -> bool;
    fn record(&mut self, extent: Region);
}

impl WriteExtents for Vec<Region> {
    fn overlaps(&self, extent: &Region) -> bool {
        self.iter().any(|w| w.overlaps(extent))
    }

    fn record(&mut self, extent: Region) {
        self.push(extent);
    }
}

impl WriteExtents for MarkedHeap {
    fn overlaps(&self, extent: &Region) -> bool {
        MarkedHeap::overlaps(self, extent)
    }

    fn record(&mut self, extent: Region) {
        MarkedHeap::record(self, extent);
    }
}

pub fn render_asgn(
    addr: &IndexTerm,
    ct: &Sctype,
    value: &IndexTerm,
    env: &SymMap<Value>,
    ctx: &GenModule,
) -> Result<AsgnWrite, AsgnError> {
    render_asgn_with_evaluator(addr, ct, value, env, ctx, &Evaluator::new(ctx))
}

pub fn render_asgn_with_evaluator(
    addr: &IndexTerm,
    ct: &Sctype,
    value: &IndexTerm,
    env: &SymMap<Value>,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<AsgnWrite, AsgnError> {
    let Region { base, size } = asgn_target_with_evaluator(addr, ct, env, ctx, evaluator)?.extent;
    rendered_write(base, size, ct, value, env, ctx, evaluator)
}

fn asgn_size(ct: &Sctype, structs: &[(Sym, StructDef)]) -> Result<u64, AsgnError> {
    ct.byte_size_in(structs)
        .ok_or_else(|| AsgnError::UnsizedCtype { ct: ct.clone() })
}

fn asgn_addr(
    addr: &IndexTerm,
    env: &SymMap<Value>,
    _ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<(u64, PointerOrigin), AsgnError> {
    match evaluator.eval(addr, env) {
        Some(Value::Scalar(Scalar::Pointer { addr, origin, .. })) => Ok((addr, origin)),
        _ => Err(AsgnError::Unevaluatable {
            context: "asgn address",
        }),
    }
}

fn rendered_write(
    addr: u64,
    size: u64,
    ct: &Sctype,
    value: &IndexTerm,
    env: &SymMap<Value>,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<AsgnWrite, AsgnError> {
    let Some(v) = evaluator.eval(value, env) else {
        return Err(AsgnError::Unevaluatable {
            context: "asgn value",
        });
    };
    let bytes = if size == 0 {
        Vec::new()
    } else {
        value_bytes(&v, ct, &ctx.structs)
            .filter(|b| b.len() as u64 == size)
            .ok_or(AsgnError::Unevaluatable {
                context: "asgn value bytes",
            })?
    };
    Ok(AsgnWrite { addr, bytes })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsgnWrite {
    pub addr: u64,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseOutput {
    pub defaults: DefaultChoices,
    pub ret: Option<Value>,
    pub writes: Vec<AsgnWrite>,
    pub env: SymMap<Value>,
    pub allocation_roots: Vec<u64>,
}

impl ParseOutput {
    pub fn args(&self) -> Option<Vec<i128>> {
        let Some(Value::Record(fields)) = &self.ret else {
            return None;
        };
        fn append(value: &Value, out: &mut Vec<i128>) -> Option<()> {
            match value {
                Value::Unit => {}
                Value::Scalar(Scalar::Int(n) | Scalar::Bits(_, _, n)) => {
                    out.push(i128::try_from(n).ok()?)
                }
                Value::Scalar(Scalar::Bool(b)) => out.push(i128::from(*b)),
                Value::Scalar(Scalar::Pointer { addr, .. }) => {
                    out.push(i128::from(*addr));
                }
                Value::Struct { fields, .. } => {
                    for (_, field) in fields {
                        append(field, out)?;
                    }
                }
                Value::Map { entries, default } => {
                    if default.is_some() {
                        return None;
                    }
                    for (expected, (index, value)) in entries.iter().enumerate() {
                        if *index != expected as i128 {
                            return None;
                        }
                        append(value, out)?;
                    }
                }
                Value::Record(_) | Value::Constructor { .. } => return None,
            }
            Some(())
        }

        let mut args = Vec::with_capacity(fields.len());
        for (_, field) in fields {
            append(field, &mut args)?;
        }
        Some(args)
    }
}

impl From<AsgnError> for TranslateError {
    fn from(e: AsgnError) -> Self {
        match e {
            AsgnError::UnsizedCtype { ct } => TranslateError::UnsizedCtype { ct },
            AsgnError::Unevaluatable { context } => TranslateError::Unevaluatable { context },
            AsgnError::OutOfBounds { addr, size } => TranslateError::AsgnOutOfBounds { addr, size },
            AsgnError::Overlap { addr, size } => TranslateError::AsgnOverlap { addr, size },
        }
    }
}

impl From<AsgnError> for ReconstructError {
    fn from(e: AsgnError) -> Self {
        match e {
            AsgnError::UnsizedCtype { ct } => ReconstructError::UnsizedCtype { ct },
            AsgnError::Unevaluatable { context } => ReconstructError::Unevaluatable { context },
            AsgnError::OutOfBounds { addr, size } => {
                ReconstructError::AsgnOutOfBounds { addr, size }
            }
            AsgnError::Overlap { addr, size } => ReconstructError::AsgnOverlap { addr, size },
        }
    }
}

enum Bound<'a> {
    Lower {
        bound: &'a IndexTerm,
        inclusive: bool,
        var_side: &'a IndexTerm,
    },
    Upper {
        bound: &'a IndexTerm,
        inclusive: bool,
        var_side: &'a IndexTerm,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolicMapBounds {
    pub lower: IndexTerm,
    pub lower_inclusive: bool,
    pub upper: IndexTerm,
    pub upper_inclusive: bool,
    pub index_bt: BaseType,
}

fn classify_bound<'a>(t: &'a IndexTerm, var: &Sym) -> Result<Bound<'a>, MapDomainError> {
    let Term::Binop(op, l, r) = &t.term else {
        return Err(MapDomainError::Unsupported);
    };
    let l_is_var = matches!(&l.term, Term::Sym(s) if s == var);
    let r_is_var = matches!(&r.term, Term::Sym(s) if s == var);
    match (op, l_is_var, r_is_var) {
        (_, true, true) => Err(MapDomainError::Unsupported),
        (BinOp::Le, false, true) => Ok(Bound::Lower {
            bound: l,
            inclusive: true,
            var_side: r,
        }),
        (BinOp::Lt, false, true) => Ok(Bound::Lower {
            bound: l,
            inclusive: false,
            var_side: r,
        }),
        (BinOp::Lt, true, false) => Ok(Bound::Upper {
            bound: r,
            inclusive: false,
            var_side: l,
        }),
        (BinOp::Le, true, false) => Ok(Bound::Upper {
            bound: r,
            inclusive: true,
            var_side: l,
        }),
        _ => Err(MapDomainError::Unsupported),
    }
}

pub fn symbolic_map_bounds(
    perm: &IndexTerm,
    var: &Sym,
) -> Result<SymbolicMapBounds, MapDomainError> {
    if let Term::Binop(BinOp::And, a, b) = &perm.term {
        let mut lower = None;
        let mut upper = None;
        for conjunct in [a.as_ref(), b.as_ref()] {
            match classify_bound(conjunct, var)? {
                Bound::Lower {
                    bound,
                    inclusive,
                    var_side,
                } => {
                    if bound.free_vars().contains(var)
                        || lower
                            .replace((bound.clone(), inclusive, var_side.bt.clone()))
                            .is_some()
                    {
                        return Err(MapDomainError::Unsupported);
                    }
                }
                Bound::Upper {
                    bound,
                    inclusive,
                    var_side,
                } => {
                    if bound.free_vars().contains(var)
                        || upper
                            .replace((bound.clone(), inclusive, var_side.bt.clone()))
                            .is_some()
                    {
                        return Err(MapDomainError::Unsupported);
                    }
                }
            }
        }
        return match (lower, upper) {
            (
                Some((lower, lower_inclusive, lower_bt)),
                Some((upper, upper_inclusive, upper_bt)),
            ) if lower_bt == upper_bt => Ok(SymbolicMapBounds {
                lower,
                lower_inclusive,
                upper,
                upper_inclusive,
                index_bt: lower_bt,
            }),
            _ => Err(MapDomainError::Unsupported),
        };
    }

    let Bound::Upper {
        bound: upper,
        inclusive: upper_inclusive,
        var_side,
    } = classify_bound(perm, var)?
    else {
        return Err(MapDomainError::Unsupported);
    };
    if !matches!(var_side.bt, BaseType::Bits(Sign::Unsigned, _)) || upper.free_vars().contains(var)
    {
        return Err(MapDomainError::Unsupported);
    }
    let lower = IndexTerm::num_lit(num_bigint::BigInt::from(0), &upper.bt)
        .ok_or(MapDomainError::Unsupported)?;
    Ok(SymbolicMapBounds {
        lower,
        lower_inclusive: true,
        upper: upper.clone(),
        upper_inclusive,
        index_bt: var_side.bt.clone(),
    })
}

pub(in crate::gen_dsl) fn map_start(
    perm: &IndexTerm,
    var: &Sym,
    env: &SymMap<Value>,
    ctx: &GenModule,
) -> Result<i128, MapDomainError> {
    map_start_with_evaluator(perm, var, env, ctx, &Evaluator::new(ctx))
}

pub(in crate::gen_dsl) fn map_start_with_evaluator(
    perm: &IndexTerm,
    var: &Sym,
    env: &SymMap<Value>,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<i128, MapDomainError> {
    let bounds = symbolic_map_bounds(perm, var)?;
    endpoint(&bounds.lower, !bounds.lower_inclusive, env, ctx, evaluator)
}

fn endpoint(
    term: &IndexTerm,
    step: bool,
    env: &SymMap<Value>,
    _ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<i128, MapDomainError> {
    let n = evaluator
        .eval(term, env)
        .and_then(|value| value.as_int())
        .ok_or(MapDomainError::Unevaluatable)?;
    if step {
        n.checked_add(1).ok_or(MapDomainError::Unevaluatable)
    } else {
        Ok(n)
    }
}

fn candidate_span(
    lo: i128,
    hi: i128,
    limit: Option<i128>,
) -> Result<std::ops::Range<i128>, MapDomainError> {
    if hi <= lo {
        return Ok(lo..lo);
    }
    match limit {
        None => Ok(lo..hi),
        Some(limit) => match hi.checked_sub(lo) {
            Some(n) if n <= limit => Ok(lo..hi),
            _ => Err(MapDomainError::TooLarge { lo, hi }),
        },
    }
}

pub fn map_domain(
    perm: &IndexTerm,
    var: &Sym,
    env: &SymMap<Value>,
    ctx: &GenModule,
) -> Result<std::ops::Range<i128>, MapDomainError> {
    map_domain_with_evaluator(perm, var, env, ctx, &Evaluator::new(ctx))
}

pub fn map_domain_with_evaluator(
    perm: &IndexTerm,
    var: &Sym,
    env: &SymMap<Value>,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<std::ops::Range<i128>, MapDomainError> {
    map_domain_with_limit(perm, var, env, ctx, evaluator, Some(MAX_MAP_ITERATIONS))
}

fn map_domain_with_limit(
    perm: &IndexTerm,
    var: &Sym,
    env: &SymMap<Value>,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
    limit: Option<i128>,
) -> Result<std::ops::Range<i128>, MapDomainError> {
    let bounds = symbolic_map_bounds(perm, var)?;
    let lo = endpoint(&bounds.lower, !bounds.lower_inclusive, env, ctx, evaluator)?;
    let hi = endpoint(&bounds.upper, bounds.upper_inclusive, env, ctx, evaluator)?;
    candidate_span(lo, hi, limit)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapCandidates {
    pub range: std::ops::Range<i128>,
    pub exact: bool,
}

impl MapCandidates {
    pub fn holds_at_with_evaluator(
        &self,
        perm: &IndexTerm,
        env: &SymMap<Value>,
        ctx: &GenModule,
        evaluator: &Evaluator<'_>,
    ) -> Result<bool, MapDomainError> {
        if self.exact {
            return Ok(true);
        }
        map_permission_holds_with_evaluator(perm, env, ctx, evaluator)
    }
    pub fn holds_at(
        &self,
        perm: &IndexTerm,
        env: &SymMap<Value>,
        ctx: &GenModule,
    ) -> Result<bool, MapDomainError> {
        if self.exact {
            return Ok(true);
        }
        map_permission_holds(perm, env, ctx)
    }
}

pub fn map_candidate_range(
    perm: &IndexTerm,
    var: &Sym,
    var_bt: &BaseType,
    env: &SymMap<Value>,
    ctx: &GenModule,
) -> Result<MapCandidates, MapDomainError> {
    map_candidate_range_with_evaluator(perm, var, var_bt, env, ctx, &Evaluator::new(ctx))
}

pub fn map_candidate_range_with_evaluator(
    perm: &IndexTerm,
    var: &Sym,
    var_bt: &BaseType,
    env: &SymMap<Value>,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<MapCandidates, MapDomainError> {
    candidate_range_with_limit(
        perm,
        var,
        var_bt,
        env,
        ctx,
        evaluator,
        Some(MAX_MAP_ITERATIONS),
    )
}

fn quantifier_candidate_range_with_evaluator(
    perm: &IndexTerm,
    var: &Sym,
    var_bt: &BaseType,
    env: &SymMap<Value>,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<MapCandidates, MapDomainError> {
    candidate_range_with_limit(perm, var, var_bt, env, ctx, evaluator, None)
}

fn candidate_range_with_limit(
    perm: &IndexTerm,
    var: &Sym,
    var_bt: &BaseType,
    env: &SymMap<Value>,
    ctx: &GenModule,
    evaluator: &Evaluator<'_>,
    limit: Option<i128>,
) -> Result<MapCandidates, MapDomainError> {
    match map_domain_with_limit(perm, var, env, ctx, evaluator, limit) {
        Ok(range) => return Ok(MapCandidates { range, exact: true }),
        Err(MapDomainError::Unsupported) => {}
        Err(error) => return Err(error),
    }

    let lower = crate::gen_dsl::passes::term_bounds::lower_bound_opt(var, var_bt, perm)
        .ok_or(MapDomainError::Unsupported)?;
    let upper = crate::gen_dsl::passes::term_bounds::upper_bound_opt(var, var_bt, perm)
        .ok_or(MapDomainError::Unsupported)?;
    if lower.bt != *var_bt
        || upper.bt != *var_bt
        || lower.free_vars().contains(var)
        || upper.free_vars().contains(var)
    {
        return Err(MapDomainError::Unsupported);
    }
    let lo = endpoint(&lower, false, env, ctx, evaluator)?;
    let hi = endpoint(&upper, true, env, ctx, evaluator)?;
    Ok(MapCandidates {
        range: candidate_span(lo, hi, limit)?,
        exact: false,
    })
}

pub fn map_permission_holds(
    perm: &IndexTerm,
    env: &SymMap<Value>,
    ctx: &GenModule,
) -> Result<bool, MapDomainError> {
    map_permission_holds_with_evaluator(perm, env, ctx, &Evaluator::new(ctx))
}

pub fn map_permission_holds_with_evaluator(
    perm: &IndexTerm,
    env: &SymMap<Value>,
    _ctx: &GenModule,
    evaluator: &Evaluator<'_>,
) -> Result<bool, MapDomainError> {
    evaluator
        .eval(perm, env)
        .and_then(|value| value.as_bool())
        .ok_or(MapDomainError::Unevaluatable)
}
