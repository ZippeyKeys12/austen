use serde::{Deserialize, Serialize};

use crate::gen_dsl::{GenModule, Sym};
use crate::spec::{Post, SpecModule};

mod counters;
pub mod cover;
pub mod harness;
pub mod heap;
pub mod incoming;
pub mod oracle_status;
pub mod radix;
pub mod source;
pub mod tally;
pub mod trust;
pub mod validation;
pub use crate::gen_dsl::{Region, TestCaseSize};
pub use cover::{COVER_ENV, COVER_MAGIC, CoverPage, EdgeSet};
pub use harness::{
    AFL_NO_FORKSRV_ENV, ALLOCATION_REDIRECTS, AllocationRedirect, EXTERNAL_INPUT_BYTES,
    HF_AUTO_ENV, HF_PERSISTENT_ENV, HF_STDIN_FALLBACK_ENV, INPUT_TIMEOUT, Mode, ModeError,
    ORACLE_CAPABILITY_SYMBOL, PERSISTENT_ENV, PERSISTENT_YES, VALIDATE_ENV, inherited_refusal,
    mode_vars, remove_mode_vars, standalone_command, wait_within,
};
pub use heap::{
    ALLOW_UNSTABLE_ADDRESSES_ENV, DEFAULT_HEAP_BASE, DEFAULT_HEAP_BYTES, HEAP_BASE_ENV,
    HEAP_BYTES_ENV, HeapBase, is_default_base, unstable_addresses_token,
};
pub use incoming::{
    INCOMING_ENV, INCOMING_VERSION, IncomingGlobal, IncomingInfo, IncomingParseError,
    IncomingPollError, PUBLISH_TIMEOUT,
};
pub use oracle_status::{
    ORACLE_STATUS_BYTES, ORACLE_STATUS_ENV, ORACLE_STATUS_MAGIC, ORACLE_STATUS_SLOTS, OracleStatus,
    OracleStatusAccess, OracleStatusMap, OracleStatusReader, PreStatus,
};
pub use radix::parse_u64;
pub use source::{ByteUse, POINTER_READING, with_source, with_source_accounted};
pub use tally::{
    TALLY_BYTES, TALLY_ENV, TALLY_MAGIC, TALLY_SLOTS, Tally, TallyAccess, TallyMap, TallyReader,
};
pub use trust::{TRUST_ENV, Trust};
pub use validation::{
    CONSTRAINT_FALSE_KIND, ValidationFailure, ValidationFrameError, ValidationObservation,
    ValidationParse, ValidationPrecondition, ValidationRunError, decode_validation_frames,
    encode_validation_frames, validate_candidates,
};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessFunction {
    pub post_defaults: crate::gen_dsl::DefaultTypes,
    pub target_id: u8,
    pub name: String,
    pub generator: Sym,
    pub post: Post,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "UncheckedHarnessModule")]
pub struct HarnessModule {
    pub functions: Vec<HarnessFunction>,
    pub generator_module: GenModule,
    pub spec: SpecModule,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedHarnessModule {
    functions: Vec<HarnessFunction>,
    generator_module: GenModule,
    spec: SpecModule,
}

impl TryFrom<UncheckedHarnessModule> for HarnessModule {
    type Error = String;

    fn try_from(module: UncheckedHarnessModule) -> Result<Self, Self::Error> {
        module
            .generator_module
            .validate()
            .map_err(|error| format!("invalid generator module: {error}"))?;
        for function in &module.functions {
            module
                .generator_module
                .function_generator(&function.generator)
                .map_err(|error| {
                    format!(
                        "function {:?} has invalid function generator {:?}: {error}",
                        function.name, function.generator
                    )
                })?;
        }
        Ok(Self {
            functions: module.functions,
            generator_module: module.generator_module,
            spec: module.spec,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct HarnessFunctionRef<'a> {
    pub post_defaults: &'a crate::gen_dsl::DefaultTypes,
    pub target_id: u8,
    pub name: &'a str,
    pub generator: &'a Sym,
    pub post: &'a Post,
}

#[derive(Debug, Serialize)]
pub struct HarnessModuleRef<'a> {
    pub functions: Vec<HarnessFunctionRef<'a>>,
    pub generator_module: &'a GenModule,
    pub spec: &'a SpecModule,
}

pub const GENERATOR_ENV: &str = "AUSTEN_GENERATOR";

pub const DEBUG_CHILD_ENV: &str = "AUSTEN_DEBUG_CHILD";

pub const ECHO_ENV: &str = "AUSTEN_ECHO";

pub const LIBFUZZER_ENV: &str = "AUSTEN_LIBFUZZER";

pub const LIBFUZZER_DRIVER_SYMBOL: &str = "LLVMFuzzerRunDriver";

pub const IN_PROCESS_DRIVER_SYMBOLS: &[&str] = &[LIBFUZZER_DRIVER_SYMBOL];

pub use libafl::executors::forkserver::SHM_ENV_VAR;

pub use libafl::executors::forkserver::SHM_FUZZ_ENV_VAR;

pub const VERSION: u8 = 1;

pub const FUNCTION_SELECTOR_BYTES: usize = std::mem::size_of::<u8>();

pub const SIZE_SELECTOR_BYTES: usize = std::mem::size_of::<u32>();

pub const INPUT_HEADER_BYTES: usize = FUNCTION_SELECTOR_BYTES + SIZE_SELECTOR_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InputHeaderError {
    #[error("input has no function selector byte")]
    MissingFunction,
    #[error("input has {actual} of the {SIZE_SELECTOR_BYTES} required test-case-size bytes")]
    MissingSize { actual: usize },
}

pub fn parse_function_selector(bytes: &[u8]) -> Result<(u8, &[u8]), InputHeaderError> {
    let (&function, remaining) = bytes
        .split_first()
        .ok_or(InputHeaderError::MissingFunction)?;
    Ok((function, remaining))
}

pub fn parse_size_selector(bytes: &[u8]) -> Result<(TestCaseSize, &[u8]), InputHeaderError> {
    let encoded = bytes
        .get(..SIZE_SELECTOR_BYTES)
        .ok_or(InputHeaderError::MissingSize {
            actual: bytes.len(),
        })?;
    let encoded = u32::from_le_bytes(
        encoded
            .try_into()
            .expect("SIZE_SELECTOR_BYTES is the width of u32"),
    );
    let size = TestCaseSize::new(u64::from(encoded) + 1)
        .expect("every size-selector bit pattern maps into TestCaseSize");
    Ok((size, &bytes[SIZE_SELECTOR_BYTES..]))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputParts<'a> {
    pub function_id: u8,
    pub test_case_size: TestCaseSize,
    pub choice_tape: &'a [u8],
}

impl<'a> InputParts<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, InputHeaderError> {
        let (function_id, bytes) = parse_function_selector(bytes)?;
        let (test_case_size, choice_tape) = parse_size_selector(bytes)?;
        Ok(Self {
            function_id,
            test_case_size,
            choice_tape,
        })
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct AustenInput {
    bytes: Vec<u8>,
}

impl AustenInput {
    pub fn from_parts(
        function_id: u8,
        test_case_size: TestCaseSize,
        choice_tape: impl Into<Vec<u8>>,
    ) -> Self {
        let choice_tape = choice_tape.into();
        let mut bytes = Vec::with_capacity(INPUT_HEADER_BYTES + choice_tape.len());
        bytes.push(function_id);
        let encoded = u32::try_from(test_case_size.get() - 1)
            .expect("TestCaseSize minus one always fits u32");
        bytes.extend_from_slice(&encoded.to_le_bytes());
        bytes.extend_from_slice(&choice_tape);
        Self { bytes }
    }

    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: bytes.into(),
        }
    }

    pub fn parts(&self) -> Result<InputParts<'_>, InputHeaderError> {
        InputParts::parse(&self.bytes)
    }

    pub fn to_hex(&self) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut text = String::with_capacity(2 + self.as_bytes().len() * 2);
        text.push_str("0x");
        for byte in self.as_bytes() {
            text.push(DIGITS[usize::from(byte >> 4)] as char);
            text.push(DIGITS[usize::from(byte & 15)] as char);
        }
        text
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

mod libafl_impls;
