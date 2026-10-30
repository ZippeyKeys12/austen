use crate::gen_dsl::choice::tree::ChoiceTree;
use crate::gen_dsl::lang::base_type::BaseType;
use crate::gen_dsl::lang::env::Env;
use crate::gen_dsl::lang::eval::eval_in;
use crate::gen_dsl::lang::gen_module::GenModule;
use crate::gen_dsl::lang::gen_term::GenTerm;
use crate::gen_dsl::lang::gen_term::GenTermKind;
use crate::gen_dsl::lang::sctype::Sctype;
use crate::gen_dsl::lang::sym::Sym;
use crate::gen_dsl::{
    DefaultTypes, Entropy, Evaluator, GenCallError, Incoming, RejectTally, Rng, TestCaseSize,
};

#[derive(Clone, Copy)]
pub struct ProduceCx<'request, 'input> {
    pub evaluator: Option<&'request Evaluator<'input>>,
    pub stop: &'request dyn Fn() -> bool,
}

impl<'request, 'input> ProduceCx<'request, 'input> {
    pub fn uninterrupted(evaluator: Option<&'request Evaluator<'input>>) -> Self {
        fn never() -> bool {
            false
        }
        Self {
            evaluator,
            stop: &never,
        }
    }
}

fn completed(result: Result<Option<ChoiceTree>, GenFailure>) -> Result<ChoiceTree, GenFailure> {
    result.map(|tree| tree.expect("uninterrupted producer"))
}

pub trait TreeProducer {
    fn produce_in(
        &mut self,
        term: &GenTerm,
        cx: ProduceCx<'_, '_>,
    ) -> Result<Option<ChoiceTree>, GenFailure>;

    fn produce(&mut self, term: &GenTerm) -> Result<ChoiceTree, GenFailure> {
        completed(self.produce_in(term, ProduceCx::uninterrupted(None)))
    }

    fn produce_with_evaluator_until(
        &mut self,
        term: &GenTerm,
        evaluator: &Evaluator<'_>,
        stop: &dyn Fn() -> bool,
    ) -> Result<Option<ChoiceTree>, GenFailure> {
        self.produce_in(
            term,
            ProduceCx {
                evaluator: Some(evaluator),
                stop,
            },
        )
    }

    fn produce_with_evaluator(
        &mut self,
        term: &GenTerm,
        evaluator: &Evaluator<'_>,
    ) -> Result<ChoiceTree, GenFailure> {
        completed(self.produce_in(term, ProduceCx::uninterrupted(Some(evaluator))))
    }

    fn produce_until(
        &mut self,
        term: &GenTerm,
        stop: &dyn Fn() -> bool,
    ) -> Result<Option<ChoiceTree>, GenFailure> {
        self.produce_in(
            term,
            ProduceCx {
                evaluator: None,
                stop,
            },
        )
    }
}

pub fn root_test_case_size(
    term: &GenTerm,
    module: &GenModule,
    incoming: &Incoming,
) -> Result<Option<TestCaseSize>, GenFailure> {
    let GenTermKind::Call { fsym, args } = &term.term else {
        return Ok(None);
    };
    let def = module.resolve_call(fsym, args.len())?;
    let Some(size_arg) = &def.size_arg else {
        return Ok(None);
    };
    let Some(at) = def.iargs.iter().position(|(param, _)| param == size_arg) else {
        return Err(GenFailure::InvalidTestCaseSize { fsym: fsym.clone() });
    };
    let mut env = Env::new();
    incoming.bind_globals(&mut env);
    let size = eval_in(&args[at], &env, module)
        .and_then(|value| value.as_int())
        .and_then(|value| u64::try_from(value).ok())
        .and_then(TestCaseSize::new)
        .ok_or_else(|| GenFailure::InvalidTestCaseSize { fsym: fsym.clone() })?;
    Ok(Some(size))
}

pub fn request_evaluator<'a, R: Rng + ?Sized>(
    term: &GenTerm,
    module: &'a GenModule,
    incoming: &Incoming,
    supplied: Option<&'a Evaluator<'_>>,
    rng: &mut R,
    max_size: u64,
) -> Result<(Option<TestCaseSize>, Evaluator<'a>), GenFailure> {
    let root_size = root_test_case_size(term, module, incoming)?;
    let evaluator = match supplied {
        Some(source) => source.for_module(module),
        None => {
            let size = root_size.unwrap_or_else(|| TestCaseSize::sample_uniform(rng, max_size));
            Evaluator::sampled(module, size, rng.next_u64(), &DefaultTypes::new())?
        }
    };
    Ok((root_size, evaluator))
}

pub trait RejectingProducer: TreeProducer {
    fn rejections(&self) -> RejectTally;

    fn sites(&self) -> &crate::gen_dsl::SiteTally {
        static EMPTY: crate::gen_dsl::SiteTally = crate::gen_dsl::SiteTally::new();
        &EMPTY
    }

    fn timings(&self) -> &crate::gen_dsl::Timings {
        static EMPTY: crate::gen_dsl::Timings = crate::gen_dsl::Timings::new();
        &EMPTY
    }

    fn last_owned_bytes(&self) -> u64 {
        0
    }
}

pub trait EntropyBacked: TreeProducer {
    fn entropy_mut(&mut self) -> &mut Entropy;

    fn produce_with_entropy_and_evaluator_until(
        &mut self,
        term: &GenTerm,
        entropy: &mut Entropy,
        evaluator: &Evaluator<'_>,
        stop: &dyn Fn() -> bool,
    ) -> Result<Option<ChoiceTree>, GenFailure> {
        produce_with_lent_entropy(
            self,
            term,
            entropy,
            ProduceCx {
                evaluator: Some(evaluator),
                stop,
            },
        )
    }

    fn produce_with_entropy(
        &mut self,
        term: &GenTerm,
        entropy: &mut Entropy,
    ) -> Result<ChoiceTree, GenFailure> {
        completed(produce_with_lent_entropy(
            self,
            term,
            entropy,
            ProduceCx::uninterrupted(None),
        ))
    }

    fn produce_with_entropy_until(
        &mut self,
        term: &GenTerm,
        entropy: &mut Entropy,
        stop: &dyn Fn() -> bool,
    ) -> Result<Option<ChoiceTree>, GenFailure> {
        produce_with_lent_entropy(
            self,
            term,
            entropy,
            ProduceCx {
                evaluator: None,
                stop,
            },
        )
    }
}

fn produce_with_lent_entropy<P: EntropyBacked + ?Sized>(
    producer: &mut P,
    term: &GenTerm,
    entropy: &mut Entropy,
    cx: ProduceCx<'_, '_>,
) -> Result<Option<ChoiceTree>, GenFailure> {
    std::mem::swap(producer.entropy_mut(), entropy);
    let result = producer.produce_in(term, cx);
    std::mem::swap(producer.entropy_mut(), entropy);
    result
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GenFailure {
    #[error("choice tree is incompatible with the wire generator: {reason}")]
    InvalidChoiceTree { reason: String },
    #[error(transparent)]
    Default(#[from] crate::gen_dsl::DefaultError),
    #[error("no valid tree after {attempts} attempts")]
    RetriesExhausted { attempts: u32 },
    #[error("no byte size for C type {ct:?}")]
    UnsizedCtype { ct: Sctype },
    #[error("no scalar width for base type {bt:?}")]
    UnsupportedBaseType { bt: BaseType },
    #[error("choice width {bits} is not representable")]
    UnrepresentableWidth { bits: u16 },
    #[error("call to unknown generator {fsym:?}")]
    UnknownGenerator { fsym: Sym },
    #[error("call to {fsym:?} passes {actual} args, definition takes {expected}")]
    CallArityMismatch {
        fsym: Sym,
        expected: usize,
        actual: usize,
    },
    #[error("root call to {fsym:?} supplies an invalid test-case size")]
    InvalidTestCaseSize { fsym: Sym },
    #[error("map permission is not a recognized finite range")]
    UnsupportedMapDomain,
    #[error(transparent)]
    ScopeMerge(#[from] crate::gen_dsl::choice::tree::ScopeMergeError),
}

impl From<GenCallError> for GenFailure {
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
