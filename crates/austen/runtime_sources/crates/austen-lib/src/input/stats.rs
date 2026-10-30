use std::collections::BTreeMap;

use crate::gen_dsl::{SiteTally, Timings, merge_timings};

use super::Rejections;

pub const GAVE_UP_ATTEMPTS: u64 = 100;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemberStats {
    pub engine: &'static str,
    pub inputs: u64,
    pub rejected: u64,
    pub rejections: Rejections,
    pub sites: SiteTally,
    pub retired: bool,
}

impl MemberStats {
    pub fn new(engine: &'static str) -> Self {
        Self {
            engine,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SizeStats {
    pub owned_bytes: BTreeMap<u64, u64>,
    pub test_case_size: BTreeMap<u64, u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FunctionStats {
    pub id: u8,
    pub name: String,
    pub inputs: u64,
    pub rejected: u64,
    pub rejections: Rejections,
    pub dispatched: u64,
    pub crashes: u64,
    pub post_failed: u64,
    pub pre_failed: u64,
    pub parse_rejected: u64,
    pub gave_up: bool,
    pub timing: Timings,
    pub sites: SiteTally,
    pub sizes: SizeStats,
    pub members: Vec<MemberStats>,
}

impl FunctionStats {
    pub fn new(id: u8, name: String) -> Self {
        Self {
            id,
            name,
            ..Self::default()
        }
    }

    pub fn absorb_execution(&mut self, executed: &Self) {
        self.inputs = executed.inputs;
        self.dispatched = executed.dispatched;
        self.crashes = executed.crashes;
        self.post_failed = executed.post_failed;
        self.pre_failed = executed.pre_failed;
        self.parse_rejected = executed.parse_rejected;
        merge_timings(&mut self.timing, &executed.timing);
    }
}

#[derive(Debug, Clone, Default)]
pub struct AttemptStats {
    pub function_id: u8,
    pub engine: &'static str,
    pub backtracks: u64,
    pub owned_bytes: u64,
    pub timing: Timings,
}
