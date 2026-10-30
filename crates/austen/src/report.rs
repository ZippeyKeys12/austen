use std::path::{Path, PathBuf};

use austen_lib::driver::CoverageOutcome;
use austen_lib::observe::report::{
    Budget, CorpusReport, CoverageCorpus, CoverageDenominator, CoveragePoint, CoverageProvenance,
    CoverageReport, CoverageTiming, CrashesReport, ErrorReport, ExternalMode, ExternalReport,
    Findings, FindingsUnit, FunctionReport, FunctionSelector, Instrumentation, MemberReport,
    OracleReport, ReplayReport, RunReport, SCHEMA_VERSION, ShrinkReport, SiteReport, SizeReport,
    Stage, StageReport, Status, SuiteMinimizationPassReport, SuiteMinimizationReport,
    SuiteOutcomeMinimizationReport, TimingReport,
};

use crate::{EXIT_CRASHES, EXIT_FAILURE, Error, Trace, classify};

pub struct Invocation<'a> {
    pub spec: &'a Path,
    pub target: Option<&'a Path>,
    pub input: Option<&'a str>,
    pub planned_budget: Budget,
    pub planned_duration: Option<u64>,
    pub function_selector: Option<austen_lib::FunctionSelectorKind>,
    pub check_heap_ownership: bool,
    pub effective_check_heap_ownership: Option<bool>,
    pub fail_on_crash: bool,
    pub multi_stage: bool,
    pub interrupted: bool,
}

pub(crate) fn function_selector(selected: austen_lib::FunctionSelectorKind) -> FunctionSelector {
    match selected {
        austen_lib::FunctionSelectorKind::Random => FunctionSelector::Random,
        austen_lib::FunctionSelectorKind::RoundRobin => FunctionSelector::RoundRobin,
    }
}

fn entropy_keys(
    source: Option<&austen_lib::driver::EntropySource>,
) -> (Option<u64>, Option<ReplayReport>) {
    use austen_lib::driver::EntropySource;
    match source {
        Some(EntropySource::Seeded(seed)) => (Some(*seed), None),
        Some(EntropySource::Replayed { path, bytes }) => (
            None,
            Some(ReplayReport {
                path: path.clone(),
                bytes: *bytes,
            }),
        ),
        None => (None, None),
    }
}

fn budget_kind(actual: Option<austen_lib::Budget>, planned: Budget) -> Budget {
    match actual {
        Some(austen_lib::Budget::Seconds(_)) => Budget::Seconds,
        Some(austen_lib::Budget::Unbounded) => Budget::Unbounded,
        Some(austen_lib::Budget::Tape) => Budget::Tape,
        Some(austen_lib::Budget::Build) => Budget::Build,
        None => planned,
    }
}

pub(crate) fn instrumentation(
    found: austen_lib::instrumentation::Instrumentation,
) -> Instrumentation {
    use austen_lib::instrumentation::Instrumentation as Found;
    match found {
        Found::Compiled => Instrumentation::Compiled,
        Found::Detected => Instrumentation::Detected,
        Found::Absent => Instrumentation::Absent,
        Found::Unknown => Instrumentation::Unknown,
    }
}

pub(crate) fn external_mode(mode: austen_lib::external::ExternalMode) -> ExternalMode {
    use austen_lib::external::ExternalMode as Selected;
    match mode {
        Selected::HonggfuzzAuto => ExternalMode::HonggfuzzAuto,
        Selected::HonggfuzzPersistent => ExternalMode::HonggfuzzPersistent,
        Selected::HonggfuzzStdin => ExternalMode::HonggfuzzStdin,
        Selected::LibFuzzerDirect => ExternalMode::LibFuzzerDirect,
        Selected::LibFuzzerFork => ExternalMode::LibFuzzerFork,
    }
}

fn coverage_timing(timing: austen_lib::Timing) -> CoverageTiming {
    use austen_lib::Timing as Seen;
    match timing {
        Seen::Observed => CoverageTiming::Observed,
        Seen::Mtime => CoverageTiming::Mtime,
        Seen::Unknown => CoverageTiming::Unknown,
    }
}

fn coverage_provenance(why: austen_lib::Provenance) -> CoverageProvenance {
    use austen_lib::Provenance as Kept;
    match why {
        Kept::Seed | Kept::Novel | Kept::Recorded => CoverageProvenance::Recorded,
        Kept::Calibration | Kept::Tool => CoverageProvenance::EngineRetained,
    }
}

fn findings(campaign: &austen_lib::CampaignReport, merged: bool) -> Findings {
    Findings {
        unit: match (merged, campaign.external.as_ref()) {
            (true, _) | (false, Some(_)) => FindingsUnit::DistinctConfirmedInputs,
            (false, None) => FindingsUnit::CrashingExecutions,
        },
        confirmed: if merged {
            campaign.findings.filed.len()
        } else {
            campaign.findings.confirmed()
        },
        capped: Some(campaign.findings.capped()),
        post_failed: campaign.findings.post_failed,
    }
}

fn buckets<'a>(
    pairs: impl IntoIterator<Item = (&'a str, u64)>,
) -> std::collections::BTreeMap<String, u64> {
    pairs.into_iter().map(|(k, n)| (k.to_owned(), n)).collect()
}

fn suite_pass(pass: &austen_lib::driver::SuitePassSummary) -> SuiteMinimizationPassReport {
    SuiteMinimizationPassReport {
        eligible: pass.eligible,
        retained: pass.retained,
        outcomes: pass
            .outcomes
            .iter()
            .map(|(outcome, summary)| {
                (
                    outcome.token().to_string(),
                    SuiteOutcomeMinimizationReport {
                        eligible: summary.eligible,
                        retained: summary.retained,
                        fallback: summary.fallback,
                    },
                )
            })
            .collect(),
        out: pass.out.clone(),
    }
}

fn timing(stats: &austen_lib::TimingStats) -> TimingReport {
    let seconds = austen_lib::TimingStats::seconds;
    TimingReport {
        count: stats.count,
        min: seconds(stats.min),
        max: seconds(stats.max),
        total: seconds(stats.total),
        avg: if stats.count == 0 {
            0.0
        } else {
            seconds(stats.total) / stats.count as f64
        },
        last: seconds(stats.last),
    }
}

fn timings(timings: &austen_lib::Timings) -> std::collections::BTreeMap<String, TimingReport> {
    timings
        .iter()
        .map(|(event, stats)| ((*event).to_owned(), timing(stats)))
        .collect()
}

fn function(stats: &austen_lib::FunctionStats) -> FunctionReport {
    FunctionReport {
        id: stats.id,
        name: stats.name.clone(),
        inputs: stats.inputs,
        rejected: stats.rejected,
        rejections: buckets(stats.rejections.nonzero().iter().map(|(k, n)| (*k, *n))),
        dispatched: stats.dispatched,
        crashes: stats.crashes,
        post_failed: stats.post_failed,
        pre_failed: stats.pre_failed,
        parse_rejected: stats.parse_rejected,
        gave_up: stats.gave_up,
        timing: timings(&stats.timing),
        sites: stats
            .sites
            .iter()
            .map(|(site, counts)| SiteReport {
                def: austen_lib::sym_text(&site.def),
                ordinal: site.ordinal,
                failed: counts.failed,
                passed: counts.passed,
            })
            .collect(),
        sizes: SizeReport {
            owned_bytes: stats.sizes.owned_bytes.clone(),
            test_case_size: stats.sizes.test_case_size.clone(),
        },
        members: (!stats.members.is_empty()).then(|| {
            stats
                .members
                .iter()
                .map(|member| MemberReport {
                    engine: member.engine.to_string(),
                    inputs: member.inputs,
                    rejected: member.rejected,
                    rejections: buckets(member.rejections.nonzero().iter().map(|(k, n)| (*k, *n))),
                    retired: member.retired,
                    sites: member
                        .sites
                        .iter()
                        .map(|(site, counts)| SiteReport {
                            def: austen_lib::sym_text(&site.def),
                            ordinal: site.ordinal,
                            failed: counts.failed,
                            passed: counts.passed,
                        })
                        .collect(),
                })
                .collect()
        }),
    }
}

fn stage(
    outcome: &austen_lib::driver::StageOutcome,
    snapshot_interval_ms: Option<u64>,
) -> StageReport {
    let campaign = outcome.campaign.as_ref();
    let loop_stats = campaign.and_then(|c| c.internal.as_ref());
    let tally = campaign.and_then(|c| c.tally);
    StageReport {
        index: outcome.index,
        engine: outcome.engine.clone(),
        budget: budget_kind(Some(outcome.budget), Budget::None),
        duration: outcome.budget.seconds(),
        seed: outcome.seed,
        harness: outcome.harness.clone(),
        elapsed: outcome.elapsed.as_secs_f64(),
        status: if outcome.failed {
            Status::Error
        } else {
            Status::Ok
        },
        inputs: loop_stats.map(|s| s.inputs),
        rejected: loop_stats.map(|s| s.rejected),
        seen: tally.map(|t| t.seen),
        selected: tally.map(|t| t.selected),
        dispatched: tally.map(|t| t.dispatched),
        crashes: campaign.map(|c| c.findings.confirmed()),
        findings: campaign.map(|c| findings(c, false)),
        corpus: campaign
            .and_then(|c| c.corpus.as_ref())
            .map(|k| CorpusReport {
                artifacts: k.artifacts,
                unique: k.unique,
                out: k.dir.clone(),
            }),
        seeds: campaign
            .and_then(|c| c.external.as_ref())
            .map(|outcome| outcome.seeds.len()),
        oracle: campaign.and_then(|c| c.oracle).map(oracle),
        external: campaign.and_then(|run| {
            run.external
                .as_ref()
                .map(|c| external(run, c, snapshot_interval_ms))
        }),
    }
}

fn oracle(o: austen_lib::wire::OracleStatus) -> OracleReport {
    OracleReport {
        parse_rejected: o.parse_rejected,
        pre_passed: o.pre_passed,
        pre_failed: o.pre_failed,
        pre_unsupported: o.pre_unsupported,
        pre_internal_error: o.pre_internal_error,
        post_unsupported: o.post_unsupported,
    }
}

fn external(
    run: &austen_lib::CampaignReport,
    c: &austen_lib::external::ExternalOutcome,
    snapshot_interval_ms: Option<u64>,
) -> ExternalReport {
    ExternalReport {
        mode: c.mode.map(external_mode),
        duration: c.duration_secs,
        snapshot_interval_ms,
        snapshot_complete: c
            .artifacts
            .as_ref()
            .map(|artifacts| artifacts.capture_error.is_none()),
        instrumented: c.instrumented,
        instrumentation: instrumentation(c.instrumentation),
        execs: c.execs,
        execs_per_sec: c.execs_per_sec,
        out: c.out_dir.clone(),
        crashes: CrashesReport {
            artifacts: run.findings.artifacts,
            unique: run.findings.unique,
            replayed: run.findings.replayed(),
            confirmed: run.findings.confirmed(),
            verdicts: buckets(
                run.findings
                    .verdicts
                    .iter()
                    .map(|(v, n)| (v.token(), *n as u64)),
            ),
        },
    }
}

pub fn build(
    inv: &Invocation<'_>,
    trace: &Trace,
    outcome: Result<&austen_lib::driver::Report, &Error>,
) -> RunReport {
    let error = outcome.err();
    let failure = error.map(|e| (classify(e), e));
    let report = outcome.ok();
    let campaign = report.map(|r| &r.campaign).or_else(|| match error? {
        Error::Driver(austen_lib::driver::DriverError::Campaign(
            austen_lib::CampaignError::WithReport { report, .. },
        )) => Some(report.as_ref()),
        Error::Driver(austen_lib::driver::DriverError::StageFailed { merged, .. }) => {
            merged.as_deref()
        }
        _ => None,
    });
    let stages: Option<&[austen_lib::driver::StageOutcome]> = inv.multi_stage.then(|| {
        report
            .map(|r| r.stages.as_slice())
            .unwrap_or_else(|| match error {
                Some(Error::Driver(austen_lib::driver::DriverError::StageFailed {
                    stages,
                    ..
                })) => stages.as_slice(),
                _ => &[],
            })
    });
    let loop_stats = campaign.and_then(|c| c.internal.as_ref());
    let tally = campaign.and_then(|c| c.tally);
    let (seed, replay) = entropy_keys(
        report
            .and_then(|r| r.entropy.as_ref())
            .or(trace.entropy.as_ref()),
    );
    let budget = report.map(|r| r.budget);
    let snapshot_interval_ms = campaign.filter(|c| c.engine.is_some()).map(|c| {
        c.corpus_snapshot_interval
            .as_millis()
            .try_into()
            .expect("campaigns reject snapshot intervals that do not fit u64")
    });

    RunReport {
        schema_version: SCHEMA_VERSION,
        provenance: trace.provenance.clone().map(|mut provenance| {
            provenance.configuration.resolved_snapshot_interval_ms = snapshot_interval_ms;
            provenance
        }),
        spec: inv.spec.to_path_buf(),
        target: inv.target.map(Path::to_path_buf),
        harness: trace.harness.clone(),
        input: inv.input.map(str::to_owned),
        function: report
            .and_then(|r| r.function.clone())
            .or_else(|| trace.function.clone()),
        work: trace.work.clone(),

        budget: Some(budget_kind(budget.flatten(), inv.planned_budget)),
        duration: budget
            .flatten()
            .map_or(inv.planned_duration, austen_lib::Budget::seconds),
        seed,
        replay,
        engine: campaign.and_then(|c| c.engine).map(str::to_owned),
        function_selector: inv.function_selector.map(function_selector),
        check_heap_ownership: inv.check_heap_ownership,
        effective_check_heap_ownership: inv.effective_check_heap_ownership,

        inputs: loop_stats.map(|s| s.inputs),
        rejected: loop_stats.map(|s| s.rejected),
        heap_base: loop_stats.map(|s| s.heap.base),
        heap_size: loop_stats.map(|s| s.heap.size),
        heap_base_is_default: loop_stats.map(|s| austen_lib::driver::is_default_base(s.heap.base)),
        rejections: loop_stats
            .and_then(|s| s.rejections.as_ref())
            .map(|t| buckets(t.nonzero().iter().map(|(k, n)| (*k, *n)))),
        functions: loop_stats
            .and_then(|s| s.functions.as_ref())
            .map(|functions| functions.iter().map(function).collect()),
        timing: loop_stats.map(|s| timings(&s.timing)),

        seen: tally.map(|t| t.seen),
        selected: tally.map(|t| t.selected),
        dispatched: tally.map(|t| t.dispatched),
        oracle: campaign.and_then(|c| c.oracle).map(oracle),

        crashes: campaign.map(|c| {
            if inv.multi_stage {
                c.findings.filed.len()
            } else {
                c.findings.confirmed()
            }
        }),
        findings: campaign.map(|c| findings(c, inv.multi_stage)),
        corpus: campaign
            .and_then(|c| c.corpus.as_ref())
            .map(|k| CorpusReport {
                artifacts: k.artifacts,
                unique: k.unique,
                out: k.dir.clone(),
            }),
        external: campaign.and_then(|run| {
            run.external
                .as_ref()
                .map(|c| external(run, c, snapshot_interval_ms))
        }),
        stages: stages.map(|stages| {
            stages
                .iter()
                .map(|outcome| stage(outcome, snapshot_interval_ms))
                .collect()
        }),

        suite_minimization: report
            .and_then(|run| run.suite_minimization.as_ref())
            .map(|suite| SuiteMinimizationReport {
                tool: suite.tool.to_string(),
                harness: suite.harness.clone(),
                initial: suite_pass(&suite.initial),
                final_pass: suite.final_pass.as_ref().map(suite_pass),
            }),

        shrink: report
            .and_then(|r| r.shrink.as_ref())
            .map(|s| ShrinkReport {
                tool: s.tool.to_string(),
                harness: s.harness.clone(),
                stored: s.stored,
                eligible: s.eligible,
                attempted: s.attempted,
                shrunk: s.shrunk,
                unchanged: s.unchanged(),
                rejected: s.rejected(),
                bytes_before: s.bytes_before,
                bytes_after: s.bytes_after,
                capped: s.capped(),
                verdicts: buckets(s.verdicts.iter().map(|(v, n)| (v.token(), *n as u64))),
                out: s.out.clone(),
            }),

        coverage: report.map(|r| match &r.coverage {
            CoverageOutcome::Skipped(why) => {
                let mut coverage = CoverageReport::skipped(why);
                coverage.snapshot_interval_ms = snapshot_interval_ms;
                coverage
            }
            CoverageOutcome::Measured(c) => CoverageReport {
                harness: c.harness.clone(),
                harness_digest: c.harness_digest.clone(),
                instrumentation: instrumentation(c.instrumentation),
                sites: c.sites(),
                covered: Some(c.covered()),
                collision_free: c.collision_free(),
                map_bytes: c.map_bytes(),
                snapshot_interval_ms,
                denominator: CoverageDenominator::InstrumentedSites,
                corpus: CoverageCorpus {
                    entries: c.entries,
                    replayed: c.replayed,
                    capped: c.capped(),
                    provenance: c.provenance.map(coverage_provenance),
                },
                verdicts: buckets(c.verdicts.iter().map(|(v, n)| (v.token(), *n as u64))),
                trajectory: c
                    .trajectory
                    .iter()
                    .map(|p| CoveragePoint {
                        elapsed_ms: p.elapsed_ms,
                        covered: p.covered,
                    })
                    .collect(),
                timing: coverage_timing(c.timing),
                edges: c.edges_out.clone(),
                skipped: None,
            },
        }),

        status: if failure.is_some() {
            Status::Error
        } else {
            Status::Ok
        },
        stage: failure.map_or(Stage::Complete, |(f, _)| f.stage),
        error: failure.map(|(f, e)| ErrorReport {
            kind: f.kind,
            blame: f.blame,
            message: e.to_string(),
        }),
        exit: match report {
            None => EXIT_FAILURE,
            Some(_) if inv.interrupted => crate::EXIT_INTERRUPTED,
            Some(r) if inv.fail_on_crash && r.campaign.findings.confirmed() > 0 => EXIT_CRASHES,
            Some(_) => 0,
        },
    }
}

pub fn write(path: &Path, report: &RunReport) -> Result<(), Error> {
    let value = serde_json::to_value(report).expect("a report serializes");
    let text = format!("{value}\n");

    let replaceable = path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_file());
    if !path.exists() {
    } else if !replaceable {
        return std::fs::write(path, &text).map_err(|source| io(path, source));
    }

    let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    let dir = dir.unwrap_or_else(|| Path::new("."));
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp: PathBuf = dir.join(format!(".{name}.{}.tmp", std::process::id()));

    let written = std::fs::write(&tmp, &text).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written.map_err(|source| io(path, source))
}

fn io(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.to_path_buf(),
        source,
    }
}
