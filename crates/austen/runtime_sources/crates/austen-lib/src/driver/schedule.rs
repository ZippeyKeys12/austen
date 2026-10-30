use std::collections::BTreeMap;
use std::time::Duration;

use crate::gen_dsl::merge_timings;
use crate::gen_dsl::rand_like::{Rng, SplitMix64};
use crate::{
    Budget, CampaignReport, Corpus, Crashes, FunctionStats, LoopStats, Rejections,
    RetainedArtifacts, Schedule, StageBudget, intern_name,
};

pub(super) struct StageSeeds {
    seed: u64,
    handed_first: bool,
    stream: SplitMix64,
}

impl StageSeeds {
    pub(super) fn new(seed: u64) -> Self {
        Self {
            seed,
            handed_first: false,
            stream: SplitMix64::new(seed),
        }
    }

    pub(super) fn next(&mut self) -> u64 {
        if !self.handed_first {
            self.handed_first = true;
            return self.seed;
        }
        self.stream.next_u64()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ScheduleClock {
    pub(super) journal_offsets: Vec<u64>,
    pub(super) stage_offsets_ms: Vec<u64>,
}

impl ScheduleClock {
    pub(super) fn single(journal_offset: u64) -> Self {
        Self {
            journal_offsets: vec![journal_offset],
            stage_offsets_ms: vec![0],
        }
    }

    pub(super) fn shift_for(&self, section_offset: u64) -> u64 {
        self.journal_offsets
            .iter()
            .zip(&self.stage_offsets_ms)
            .rev()
            .find(|(journal, _)| **journal <= section_offset)
            .map_or(0, |(_, ms)| *ms)
    }
}

pub(super) fn stage_budget(
    stated: StageBudget,
    is_last: bool,
    single: bool,
    build_only: bool,
    replay: bool,
    duration_secs: Option<u64>,
) -> Budget {
    if build_only {
        return Budget::Build;
    }
    if replay {
        return Budget::Tape;
    }
    match stated {
        StageBudget::Seconds(seconds) => Budget::Seconds(seconds.get()),
        StageBudget::Unstated if single => duration_secs.map_or(Budget::Unbounded, Budget::Seconds),
        StageBudget::Unstated => {
            debug_assert!(is_last, "the parser gives every non-last stage a budget");
            Budget::Unbounded
        }
    }
}

pub(super) fn total_budget(
    schedule: &Schedule,
    build_only: bool,
    replay: bool,
    duration_secs: Option<u64>,
) -> Budget {
    if build_only {
        return Budget::Build;
    }
    if replay {
        return Budget::Tape;
    }
    if schedule.is_single() {
        return stage_budget(
            schedule.stages()[0].budget,
            true,
            true,
            false,
            false,
            duration_secs,
        );
    }
    let mut total = 0u64;
    for stage in schedule.stages() {
        match stage.budget {
            StageBudget::Seconds(seconds) => total = total.saturating_add(seconds.get()),
            StageBudget::Unstated => return Budget::Unbounded,
        }
    }
    Budget::Seconds(total)
}

pub(super) fn merge_campaigns(
    schedule: &Schedule,
    corpus_snapshot_interval: Duration,
    reports: Vec<&CampaignReport>,
) -> Option<CampaignReport> {
    if reports.is_empty() {
        return None;
    }
    if schedule.is_single() {
        return reports.into_iter().next().cloned();
    }
    let mut findings = Crashes::default();
    let mut corpus: Option<Corpus> = None;
    let mut retained = RetainedArtifacts::default();
    let mut tally: Option<crate::wire::Tally> = None;
    let mut oracle: Option<crate::wire::OracleStatus> = None;
    let mut internal: Option<LoopStats> = None;
    let mut external = None;
    for report in reports {
        findings.artifacts += report.findings.artifacts;
        findings.unique += report.findings.unique;
        for (verdict, count) in &report.findings.verdicts {
            *findings.verdicts.entry(*verdict).or_default() += count;
        }
        findings.filed.extend(report.findings.filed.iter().cloned());
        findings.post_failed += report.findings.post_failed;
        if let Some(kept) = &report.corpus {
            match &mut corpus {
                Some(merged) => {
                    merged.artifacts += kept.artifacts;
                    merged.unique += kept.unique;
                }
                None => corpus = Some(kept.clone()),
            }
        }
        retained.extend(report.retained.iter().cloned());
        if let Some(t) = report.tally {
            let merged = tally.get_or_insert(crate::wire::Tally {
                seen: 0,
                selected: 0,
                dispatched: 0,
                post_failed: 0,
            });
            merged.seen += t.seen;
            merged.selected += t.selected;
            merged.dispatched += t.dispatched;
            merged.post_failed += t.post_failed;
        }
        if let Some(o) = report.oracle {
            let merged = oracle.get_or_insert_with(Default::default);
            merged.parse_rejected += o.parse_rejected;
            merged.pre_passed += o.pre_passed;
            merged.pre_failed += o.pre_failed;
            merged.pre_unsupported += o.pre_unsupported;
            merged.pre_internal_error += o.pre_internal_error;
            merged.post_unsupported += o.post_unsupported;
        }
        if let Some(stats) = &report.internal {
            match &mut internal {
                Some(merged) => merge_loop_stats(merged, stats),
                None => internal = Some(stats.clone()),
            }
        }
        if let Some(outcome) = &report.external {
            external = Some(outcome.clone());
        }
    }
    Some(CampaignReport {
        engine: Some(intern_name(&schedule.canonical_engine())),
        corpus_snapshot_interval,
        findings,
        corpus,
        retained,
        tally,
        oracle,
        internal,
        external,
    })
}

fn merge_loop_stats(merged: &mut LoopStats, other: &LoopStats) {
    merged.inputs += other.inputs;
    merged.rejected += other.rejected;
    merge_timings(&mut merged.timing, &other.timing);
    merged.retention_complete &= other.retention_complete;
    match (&mut merged.rejections, &other.rejections) {
        (Some(into), Some(from)) => into.add(from.nonzero().iter().copied()),
        (None, Some(from)) => merged.rejections = Some(from.clone()),
        (_, None) => {}
    }
    match (&mut merged.functions, &other.functions) {
        (Some(into), Some(from)) => {
            let mut by_id: BTreeMap<u8, usize> = into
                .iter()
                .enumerate()
                .map(|(index, stats)| (stats.id, index))
                .collect();
            for stats in from {
                match by_id.get(&stats.id) {
                    Some(index) => merge_function_stats(&mut into[*index], stats),
                    None => {
                        by_id.insert(stats.id, into.len());
                        into.push(stats.clone());
                    }
                }
            }
        }
        (None, Some(from)) => merged.functions = Some(from.clone()),
        (_, None) => {}
    }
}

fn merge_function_stats(into: &mut FunctionStats, from: &FunctionStats) {
    into.inputs += from.inputs;
    into.rejected += from.rejected;
    into.rejections
        .add(from.rejections.nonzero().iter().copied());
    into.dispatched += from.dispatched;
    into.crashes += from.crashes;
    into.post_failed += from.post_failed;
    into.pre_failed += from.pre_failed;
    into.parse_rejected += from.parse_rejected;
    into.gave_up |= from.gave_up;
    merge_timings(&mut into.timing, &from.timing);
    for (site, count) in &from.sites {
        let entry = into.sites.entry(site.clone()).or_default();
        entry.failed += count.failed;
        entry.passed += count.passed;
    }
    for (bytes, count) in &from.sizes.owned_bytes {
        *into.sizes.owned_bytes.entry(*bytes).or_default() += count;
    }
    for (size, count) in &from.sizes.test_case_size {
        *into.sizes.test_case_size.entry(*size).or_default() += count;
    }
    if into.members.len() == from.members.len()
        && into
            .members
            .iter()
            .zip(&from.members)
            .all(|(a, b)| a.engine == b.engine)
    {
        for (a, b) in into.members.iter_mut().zip(&from.members) {
            a.inputs += b.inputs;
            a.rejected += b.rejected;
            a.rejections.add(b.rejections.nonzero().iter().copied());
            a.retired |= b.retired;
            for (site, count) in &b.sites {
                let entry = a.sites.entry(site.clone()).or_default();
                entry.failed += count.failed;
                entry.passed += count.passed;
            }
        }
    }
}

#[allow(dead_code)]
fn _rejections_shape(_: &Rejections) {}
