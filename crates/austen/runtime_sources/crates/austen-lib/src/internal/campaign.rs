use crate::observe;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use crate::gen_dsl::recording;
use crate::observe::{CampaignKind, CampaignSnapshot, Reporter, RunEvent, SNAPSHOT_TICK};

use crate::campaign::{
    Budget, CampaignConfig, CampaignError, CampaignReport, Engine, GeneratorPhase, LoopStats,
};
use crate::coverage::{Journal, Novelty, Provenance, Timing};
use crate::findings::{
    ArtifactFolders, ArtifactValidator, Corpus, Crashes, Replay, RetainedArtifact,
    RetainedArtifacts, Store, validate_all_disk_backed,
};
use crate::gen_dsl::{Timings, merge_timings};
use crate::generator::{
    AnyGenerator, GenerateCx, GeneratePoll, GeneratorBuild, GeneratorError, GeneratorFactory,
    StopReason,
};
use crate::input::RETRIES_EXHAUSTED;
use crate::internal::spool::{LayoutError, Retention, Spool, SpoolLayout};
use crate::internal::target;
use crate::tyche::{Execution, TycheCase, TycheStatus, TycheWriter};
use crate::{FunctionStats, TimingStats};

fn last_timings(source: &dyn crate::Generator) -> BTreeMap<&'static str, f64> {
    source
        .last_attempt()
        .map(|attempt| {
            attempt
                .timing
                .iter()
                .map(|(event, timing)| (*event, timing.last_secs()))
                .collect()
        })
        .unwrap_or_default()
}

fn merge_generation_timing(source: &dyn crate::Generator, timing: &mut Timings) {
    if let Some(attempt) = source.last_attempt() {
        merge_timings(timing, &attempt.timing);
    }
}

fn observe_execution(
    function: &mut FunctionStats,
    dispatched: u64,
    pre_failed: bool,
    parse_rejected: bool,
    execute: TimingStats,
) {
    function.dispatched += dispatched;
    function.pre_failed += u64::from(pre_failed);
    function.parse_rejected += u64::from(parse_rejected);
    function.timing.entry("execute").or_default().merge(execute);
}

fn dispatched_coverage(
    novelty: Option<&mut Novelty>,
    map: Option<&[u8]>,
    dispatched_before: u64,
    dispatched_after: u64,
) -> Option<Provenance> {
    if dispatched_after <= dispatched_before {
        return None;
    }
    novelty
        .zip(map)
        .and_then(|(novelty, map)| novelty.showed_anything_new(map))
}

fn open_spool_then<T>(layout: SpoolLayout, next: impl FnOnce() -> T) -> (Spool, T) {
    let spool = Spool::open(layout);
    let result = next();
    (spool, result)
}

fn campaign_deadline(started: Instant, budget: Budget) -> Option<Instant> {
    budget
        .seconds()
        .and_then(|seconds| started.checked_add(std::time::Duration::from_secs(seconds)))
}

fn timed_budget_spent(budget: Budget, started: Instant, now: Instant) -> bool {
    budget.seconds().is_some_and(|seconds| {
        now.duration_since(started) >= std::time::Duration::from_secs(seconds)
    })
}

fn stop_after_poll(
    cancel: &observe::Cancel,
    strict_duration: bool,
    budget: Budget,
    started: Instant,
    now: Instant,
) -> Option<StopReason> {
    if cancel.is_cancelled() {
        Some(StopReason::Cancelled)
    } else if strict_duration && timed_budget_spent(budget, started, now) {
        Some(StopReason::Budget)
    } else {
        None
    }
}

struct LiveGeneratorFailure {
    primary: GeneratorError,
    secondary: Vec<String>,
}

impl LiveGeneratorFailure {
    fn new(primary: GeneratorError) -> Self {
        Self {
            primary,
            secondary: Vec::new(),
        }
    }

    fn attach(&mut self, during: &'static str, error: impl std::fmt::Display) {
        self.secondary
            .push(format!("{during} also failed: {error}"));
    }

    fn into_campaign(self, generator: &'static str) -> CampaignError {
        let process_backed = matches!(&self.primary, GeneratorError::Tool(_));
        let mut message = self.primary.to_string();
        for secondary in self.secondary {
            message.push_str("; ");
            message.push_str(&secondary);
        }
        if process_backed {
            CampaignError::Generator {
                generator,
                phase: GeneratorPhase::Live,
                message,
            }
        } else {
            CampaignError::Unrunnable(message)
        }
    }
}

fn refuse_live(
    live: &mut dyn crate::Generator,
    refusal: impl FnOnce(String) -> CampaignError,
) -> CampaignError {
    let cleanup = live
        .finish(StopReason::Failure)
        .err()
        .map(|error| format!("; cleanup also failed: {error}"))
        .unwrap_or_default();
    refusal(cleanup)
}

fn require_live_trust(
    generator: &'static str,
    prepared: crate::wire::Trust,
    live: &mut dyn crate::Generator,
) -> Result<(), CampaignError> {
    let claimed = live.trust();
    if claimed == prepared {
        return Ok(());
    }
    Err(refuse_live(live, |cleanup| {
        CampaignError::GeneratorTrustMismatch {
            generator,
            prepared,
            live: claimed,
            cleanup,
        }
    }))
}

fn require_live_name(
    expected: &'static str,
    live: &mut dyn crate::Generator,
) -> Result<(), CampaignError> {
    let actual = live.name();
    if actual == expected {
        return Ok(());
    }
    Err(refuse_live(live, |cleanup| CampaignError::Generator {
        generator: expected,
        phase: GeneratorPhase::Open,
        message: format!(
            "prepared generator {expected:?} opened a live generator named {actual:?}{cleanup}"
        ),
    }))
}

fn absorb_secondary<E: std::fmt::Display>(
    during: &'static str,
    error: E,
    generator_failure: &mut Option<LiveGeneratorFailure>,
    campaign_failed: bool,
    reporter: &Reporter,
) -> Option<E> {
    if let Some(primary) = generator_failure {
        primary.attach(during, error);
        None
    } else if campaign_failed {
        reporter.emit(|| {
            RunEvent::Warning(observe::Warning::FinalizationFailed {
                during,
                message: error.to_string(),
            })
        });
        None
    } else {
        Some(error)
    }
}

#[derive(Debug, Default)]
struct Archived {
    filed: BTreeSet<PathBuf>,
    kept: BTreeSet<PathBuf>,
    retained: RetainedArtifacts,
}

#[derive(Debug)]
struct ArchiveFailure {
    archived: Archived,
    error: Box<CampaignError>,
}

fn resolve_spooled_archive(
    spool: &Spool,
    attempt: Result<Archived, ArchiveFailure>,
) -> Result<(Archived, Option<String>), CampaignError> {
    match attempt {
        Ok(archived) => Ok((archived, None)),
        Err(failure)
            if spool.broken().is_some()
                && matches!(
                    failure.error.as_ref(),
                    CampaignError::Findings(inner) if inner.is_storage_or_journal_failure()
                ) =>
        {
            let message = failure.error.to_string();
            Ok((failure.archived, Some(message)))
        }
        Err(failure) => Err(*failure.error),
    }
}

fn archive_fail(archived: Archived, error: impl Into<CampaignError>) -> ArchiveFailure {
    ArchiveFailure {
        archived,
        error: Box::new(error.into()),
    }
}

fn retention_complete(spool: &Spool, journal: &Journal) -> bool {
    spool.broken().is_none() && journal.broken().is_none()
}

fn archive_spooled(
    spool: &Spool,
    validator: &dyn ArtifactValidator,
    replay: &Replay<'_>,
    folders: &ArtifactFolders,
    reporter: &Reporter,
    journal: &mut Journal,
) -> Result<Archived, ArchiveFailure> {
    let canonical = match spool.population() {
        Some(dir) => validate_all_disk_backed(dir, validator, reporter, journal)
            .map_err(|error| archive_fail(Archived::default(), error))?,
        None => Vec::new(),
    };

    let mut archived = Archived::default();
    for token in canonical {
        let Some(retention) = Retention::parse(token.source()) else {
            return Err(archive_fail(
                archived,
                CampaignError::Unrunnable(format!(
                    "validation returned unknown internal candidate {}",
                    token.source().display(),
                )),
            ));
        };
        let artifact = match token.materialize() {
            Ok(artifact) => artifact,
            Err(error) => return Err(archive_fail(archived, error)),
        };
        let measured = match replay.measure(artifact.bytes()) {
            Ok(measured) => measured,
            Err(error) => {
                reporter.emit(|| {
                    RunEvent::Warning(observe::Warning::ReplayFailed {
                        message: error.to_string(),
                    })
                });
                continue;
            }
        };
        let Some(classified) = artifact.classify(&measured) else {
            reporter.emit(|| {
                RunEvent::Warning(observe::Warning::ReplayFailed {
                    message: format!(
                        "checked validation and replay disagreed for {}",
                        artifact.source().display(),
                    ),
                })
            });
            continue;
        };
        tracing::trace!(
            target: "austen::artifact",
            source = %artifact.source().display(),
            observed_killed = retention.killed,
            observed_pre_failed = retention.pre_failed,
            observed_parse_rejected = retention.parse_rejected,
            outcome = classified.outcome().token(),
        );
        let outcome = classified.outcome();
        let is_crash_finding = outcome.is_crash_finding();
        let path = match Store::promote_outcome(
            folders,
            &classified,
            journal,
            Some(retention.elapsed_ms),
            retention.coverage.unwrap_or(Provenance::Recorded),
            Timing::Observed,
        ) {
            Ok(path) => path,
            Err(error) => return Err(archive_fail(archived, error)),
        };
        archived
            .retained
            .insert(RetainedArtifact::new(outcome, path.clone()));
        if is_crash_finding {
            archived.filed.insert(path);
        } else {
            archived.kept.insert(path);
        }
    }
    Ok(archived)
}

impl Engine for AnyGenerator {
    fn name(&self) -> &'static str {
        GeneratorFactory::name(self)
    }

    fn display_name(&self) -> &'static str {
        GeneratorFactory::display_name(self)
    }

    fn profiles(&self) -> Vec<crate::gen_dsl::passes::PassProfile> {
        GeneratorFactory::profiles(self)
    }

    fn campaign(&self, cfg: CampaignConfig) -> Result<CampaignReport, CampaignError> {
        campaign_with_generator_factory(self, cfg)
    }
}

#[doc(hidden)]
pub fn campaign_with_generator_factory(
    factory: &dyn GeneratorFactory,
    mut cfg: CampaignConfig,
) -> Result<CampaignReport, CampaignError> {
    if cfg.exit_fast && matches!(cfg.budget, Budget::Build | Budget::Tape) {
        return Err(CampaignError::Unrunnable(
            "--exit-fast requires a live campaign rather than build-only or replay".into(),
        ));
    }
    if cfg.strict_duration && !matches!(cfg.budget, Budget::Seconds(seconds) if seconds > 0) {
        return Err(CampaignError::Unrunnable(
            "--strict-duration requires a live campaign with an explicit nonzero seconds budget"
                .into(),
        ));
    }
    if cfg.tyche.is_some() && cfg.budget == Budget::Build {
        return Err(CampaignError::Unrunnable(
            "Tyche output requires executed attempts and cannot accompany build-only".into(),
        ));
    }
    crate::campaign::validate_functions(&cfg.functions)?;
    let generator_name = factory.name();
    let mut journal = Journal::open(&cfg.journal, generator_name, std::time::SystemTime::now());
    crate::campaign::validate_generator_modules(
        &cfg.functions,
        &cfg.wire_module,
        &cfg.production_modules,
    )?;
    let prepared = factory.prepare(&cfg, &mut journal)?;
    let prepared_name = prepared.name();
    if prepared_name != generator_name {
        return Err(CampaignError::Generator {
            generator: generator_name,
            phase: GeneratorPhase::Prepare,
            message: format!(
                "generator factory {generator_name:?} prepared a generator named {prepared_name:?}"
            ),
        });
    }
    let trust = prepared.trust();
    let folders = ArtifactFolders::new(&cfg.corpus, &cfg.crashes);
    let protected = cfg.output_paths(&folders);
    let spool_layout =
        SpoolLayout::validate(&cfg.work, protected).map_err(|error| match error {
            LayoutError::Conflict(conflict) => CampaignError::SpoolPathConflict {
                spool: conflict.spool,
                role: conflict.role,
                path: conflict.path,
            },
            LayoutError::Resolve { path, source } => {
                CampaignError::SpoolPathResolution { path, source }
            }
        })?;
    let (entropy, mut recorder) = match cfg.record.as_ref() {
        Some(path) => {
            let rec = std::fs::File::create(path)
                .and_then(|f| recording::Recorder::new(std::io::BufWriter::new(f)))
                .map_err(|source| CampaignError::Io {
                    path: path.clone(),
                    source,
                })?;
            let (entropy, log) = cfg.entropy.recording();
            (entropy, Some((path.clone(), rec, log)))
        }
        None => (cfg.entropy, None),
    };

    let mut harness = target::Harness::spawn(&target::Spawn {
        harness: &cfg.harness,
        generator_path: &cfg.generator_path,
        trust,
        work: &cfg.work,
        heap_bytes: cfg.heap_bytes,
        allow_unstable_addresses: cfg.allow_unstable_addresses,
        debug_child: cfg.debug_child,
        coverage: cfg.instrumented && !matches!(cfg.budget, Budget::Build),
        coverage_guards: cfg.coverage_guards,
        reporter: &cfg.reporter,
        generator_module: &cfg.wire_module,
    })?;
    let incoming = harness.incoming().clone();
    let region = incoming.heap();

    if matches!(cfg.budget, Budget::Build) {
        let oracle = harness.oracle_status();
        crate::campaign::warn_oracle_unsupported(&cfg.reporter, oracle);
        return Ok(CampaignReport {
            engine: Some(generator_name),
            corpus_snapshot_interval: cfg.corpus_snapshot_interval,
            findings: Crashes::default(),
            corpus: None,
            retained: RetainedArtifacts::default(),
            tally: harness.tally(),
            oracle,
            internal: Some(LoopStats {
                functions: None,
                timing: BTreeMap::new(),
                inputs: 0,
                rejected: 0,
                heap: region,
                retention_complete: true,
                rejections: Some(crate::input::Rejections::default()),
            }),
            external: None,
        });
    }

    let mut functions: BTreeMap<u8, FunctionStats> = cfg
        .functions
        .iter()
        .map(|function| {
            (
                function.target_id,
                FunctionStats {
                    id: function.target_id,
                    name: function.name.clone(),
                    ..FunctionStats::default()
                },
            )
        })
        .collect();
    let generator_build = GeneratorBuild {
        functions: std::mem::take(&mut cfg.functions),
        wire_module: std::mem::take(&mut cfg.wire_module),
        production_modules: std::mem::take(&mut cfg.production_modules),
        incoming,
        max_array_length: cfg.max_array_length,
        generator_timeout: cfg.generator_timeout,
        chain_discard_limit: cfg.chain_discard_limit,
        function_selector: cfg.function_selector,
        size_selector: cfg.size_selector,
        max_test_case_size: cfg.max_test_case_size,
        entropy,
    };

    let (mut inputs, mut crashes, mut rejected) = (0usize, 0usize, 0usize);
    let mut dropped_input: Option<(u8, &'static str)> = None;
    let mut post_failed = 0usize;
    let mut last_post = harness.tally().map_or(0, |t| t.post_failed);
    let recording = cfg.instrumented && !matches!(cfg.budget, Budget::Build);
    let mut novelty = recording.then(Novelty::new);
    let watched = !cfg.reporter.is_silent();
    let (mut spool, source) = open_spool_then(spool_layout, || prepared.open(generator_build));
    let mut source = source?;
    require_live_name(generator_name, &mut *source)?;
    require_live_trust(generator_name, trust, &mut *source)?;
    let started = Instant::now();
    let mut tyche = TycheWriter::open(cfg.tyche.as_deref(), SystemTime::now(), &cfg.reporter);
    let mut timing = Timings::new();
    let mut next_tick = started + SNAPSHOT_TICK;
    let deadline = campaign_deadline(started, cfg.budget);
    let mut stop_reason = StopReason::Budget;
    let mut generator_failure = None;
    let mut campaign_failure = None;
    loop {
        let now = Instant::now();

        if cfg.cancel.is_cancelled() {
            stop_reason = StopReason::Cancelled;
            break;
        }
        let left = source.replay_remaining().unwrap_or(0);
        match cfg.budget {
            Budget::Build => unreachable!("build-only returned before opening a generator"),
            Budget::Tape if left == 0 => break,
            Budget::Seconds(_) if timed_budget_spent(cfg.budget, started, now) => break,
            _ => {}
        }

        if now >= next_tick {
            next_tick = now + SNAPSHOT_TICK;
            if watched {
                let tally = harness.tally();
                cfg.reporter.emit(|| {
                    RunEvent::Snapshot(CampaignSnapshot {
                        kind: CampaignKind::Internal,
                        elapsed: now.duration_since(started),
                        budget: cfg.budget.seconds().map(std::time::Duration::from_secs),
                        inputs: Some(inputs as u64),
                        rejected: Some(rejected as u64),
                        rejections_total: Some(source.rejections().backtracks()),
                        replay_left: matches!(cfg.budget, Budget::Tape).then_some(left as u64),
                        seen: tally.map(|t| t.seen),
                        selected: tally.map(|t| t.selected),
                        dispatched: tally.map(|t| t.dispatched),
                        execs: None,
                        execs_per_sec: None,
                        crashes: crashes as u64,
                    })
                });
            }
        }

        let wake_at = deadline.map_or(next_tick, |deadline| deadline.min(next_tick));
        let poll = source.poll_next(
            GenerateCx::new(&cfg.cancel, wake_at, matches!(cfg.budget, Budget::Tape))
                .with_deadline(deadline),
        );
        let poll_completed_at = if cfg.strict_duration {
            Instant::now()
        } else {
            now
        };
        if matches!(
            &poll,
            Ok(GeneratePoll::Input(_) | GeneratePoll::Rejected(_) | GeneratePoll::Interrupted)
                | Err(_)
        ) {
            merge_generation_timing(&*source, &mut timing);
        }
        match poll {
            Ok(GeneratePoll::Input(input)) => {
                if let Some(reason) = stop_after_poll(
                    &cfg.cancel,
                    cfg.strict_duration,
                    cfg.budget,
                    started,
                    poll_completed_at,
                ) {
                    dropped_input = source
                        .last_attempt()
                        .map(|attempt| (attempt.function_id, attempt.engine));
                    stop_reason = reason;
                    break;
                }
                let input_bytes = input.len();
                let dispatched_before = harness.tally().map_or(0, |tally| tally.dispatched);
                let status_before = harness.oracle_status();
                let pre_failed_before = status_before.map_or(0, |status| status.pre_failed);
                let parse_rejected_before = status_before.map_or(0, |status| status.parse_rejected);
                let execution_started = Instant::now();
                let execution = harness.run(&input);
                let observed_at = Instant::now();
                let mut execute = TimingStats::default();
                execute.observe(observed_at.duration_since(execution_started));
                timing.entry("execute").or_default().merge(execute);
                let status_after = harness.oracle_status();
                let pre_failed =
                    status_after.is_some_and(|status| status.pre_failed > pre_failed_before);
                let parse_rejected_bumped = status_after
                    .is_some_and(|status| status.parse_rejected > parse_rejected_before);
                let parse_rejected = trust == crate::wire::Trust::Checked && parse_rejected_bumped;
                let dispatched_after = harness.tally().map_or(0, |tally| tally.dispatched);
                let function_id = input.parts().ok().map(|parts| parts.function_id);
                if let Some(function) = function_id.and_then(|id| functions.get_mut(&id)) {
                    observe_execution(
                        function,
                        dispatched_after.saturating_sub(dispatched_before),
                        pre_failed,
                        parse_rejected_bumped,
                        execute,
                    );
                }
                let killed = match execution {
                    Ok(killed) => killed,
                    Err(_) if cfg.cancel.is_cancelled() => {
                        stop_reason = StopReason::Cancelled;
                        break;
                    }
                    Err(error) => {
                        stop_reason = StopReason::Failure;
                        campaign_failure = Some(error);
                        break;
                    }
                };
                let mut postcondition_failed = false;
                let coverage = dispatched_coverage(
                    novelty.as_mut(),
                    harness.coverage_map(),
                    dispatched_before,
                    dispatched_after,
                );
                if killed {
                    crashes += 1;

                    let now_post = harness.tally().map_or(last_post, |t| t.post_failed);
                    if now_post > last_post {
                        post_failed += 1;
                        postcondition_failed = true;
                    }
                    last_post = now_post;

                    cfg.reporter.emit(|| RunEvent::CrashObserved {
                        index: inputs as u64,
                        input_bytes,
                    });
                }
                let function = function_id.and_then(|id| functions.get_mut(&id));
                let property = if let Some(function) = function {
                    function.inputs += 1;
                    function.crashes += u64::from(killed);
                    function.post_failed += u64::from(postcondition_failed);
                    function.name.as_str()
                } else {
                    "<unknown>"
                };
                if tyche.enabled() {
                    let unsupported = status_after.is_some_and(|after| {
                        let before = status_before.unwrap_or_default();
                        after.pre_unsupported > before.pre_unsupported
                            || after.pre_internal_error > before.pre_internal_error
                            || after.post_unsupported > before.post_unsupported
                    });
                    let (status, status_reason) = TycheStatus::classify(Execution {
                        killed,
                        post_failed: postcondition_failed,
                        pre_failed,
                        parse_rejected: parse_rejected_bumped,
                        oracle_unsupported: unsupported,
                        dispatched: dispatched_after > dispatched_before,
                    });
                    let mut case_timing = last_timings(&*source);
                    case_timing.insert("execute", execute.last_secs());
                    let attempt = source.last_attempt();
                    tyche.write(
                        TycheCase {
                            property,
                            status,
                            status_reason,
                            input: Some(&input),
                            backtracks: attempt.map(|attempt| attempt.backtracks),
                            owned_bytes: attempt.map(|attempt| attempt.owned_bytes),
                            timing: case_timing,
                        },
                        &cfg.reporter,
                    );
                }
                if coverage.is_some() || killed || pre_failed || parse_rejected {
                    spool.retain(
                        &Retention {
                            index: inputs,
                            elapsed_ms: observed_at
                                .duration_since(started)
                                .as_millis()
                                .min(u128::from(u64::MAX))
                                as u64,
                            coverage,
                            killed,
                            pre_failed,
                            parse_rejected,
                        },
                        input.as_bytes(),
                    );
                }
                inputs += 1;
                if let Some((path, rec, log)) = &mut recorder
                    && let Err(source) = rec.push(&log.line())
                {
                    stop_reason = StopReason::Failure;
                    campaign_failure = Some(CampaignError::Io {
                        path: path.clone(),
                        source,
                    });
                    break;
                }
                if killed && cfg.exit_fast {
                    stop_reason = StopReason::Finding;
                    break;
                }
            }
            Ok(GeneratePoll::Rejected(reason)) => {
                rejected += 1;
                if tyche.enabled() {
                    let attempt = source.last_attempt();
                    let property = attempt
                        .and_then(|attempt| functions.get(&attempt.function_id))
                        .map_or("<unknown>", |function| function.name.as_str());
                    tyche.write(
                        TycheCase {
                            property,
                            status: TycheStatus::GaveUp,
                            status_reason: if reason == RETRIES_EXHAUSTED {
                                "Generation backtracked all the way to the top"
                            } else {
                                reason
                            },
                            input: None,
                            backtracks: attempt.map(|attempt| attempt.backtracks),
                            owned_bytes: attempt.map(|attempt| attempt.owned_bytes),
                            timing: last_timings(&*source),
                        },
                        &cfg.reporter,
                    );
                }
            }
            Ok(GeneratePoll::Pending) => continue,
            Ok(GeneratePoll::Interrupted) => {
                stop_reason = if cfg.cancel.is_cancelled() {
                    StopReason::Cancelled
                } else {
                    StopReason::Budget
                };
                break;
            }
            Ok(GeneratePoll::Exhausted) => break,
            Err(error) => {
                stop_reason = StopReason::Failure;
                generator_failure = Some(LiveGeneratorFailure::new(error));
                break;
            }
        }
    }

    tyche.finish(&cfg.reporter);

    if let Err(error) = source.finish(stop_reason)
        && let Some(error) = absorb_secondary(
            "generator finalization",
            error,
            &mut generator_failure,
            campaign_failure.is_some(),
            &cfg.reporter,
        )
    {
        generator_failure = Some(LiveGeneratorFailure::new(error));
    }

    if let Some((path, rec, ..)) = &mut recorder
        && let Err(source) = rec.flush()
    {
        let failure = CampaignError::Io {
            path: path.clone(),
            source,
        };
        if let Some(failure) = absorb_secondary(
            "recording finalization",
            failure,
            &mut generator_failure,
            campaign_failure.is_some(),
            &cfg.reporter,
        ) {
            campaign_failure = Some(failure);
        }
    }

    let replay_tally = cfg.work.join("internal-outcome-tally");
    let replay_env = crate::external::harness_env_for(
        &cfg.generator_path,
        crate::wire::Trust::Checked,
        cfg.heap_bytes,
        &replay_tally,
        cfg.allow_unstable_addresses,
    );
    let replay = Replay {
        harness: &cfg.harness,
        env: &replay_env,
        tally: &replay_tally,
        cover: None,
    };
    let archive = resolve_spooled_archive(
        &spool,
        archive_spooled(
            &spool,
            cfg.validator.as_ref(),
            &replay,
            &folders,
            &cfg.reporter,
            &mut journal,
        ),
    );
    let (archived, archive_loss) = match archive {
        Ok(result) => result,
        Err(error) => {
            if let Some(error) = absorb_secondary(
                "retained-input archival",
                error,
                &mut generator_failure,
                campaign_failure.is_some(),
                &cfg.reporter,
            ) {
                return Err(error);
            }
            return Err(match generator_failure {
                Some(primary) => primary.into_campaign(generator_name),
                None => campaign_failure
                    .expect("a secondary failure is absorbed only behind a primary one"),
            });
        }
    };
    let Archived {
        filed,
        kept,
        retained: retained_artifacts,
    } = archived;

    if let Some(why) = spool.broken() {
        let mut message = if spool.population().is_some() {
            format!(
                "the retained-input spool could not be written ({why}); \
                     inputs retained after the failure are missing from the archive"
            )
        } else {
            format!(
                "the retained-input spool could not be safely read ({why}); \
                     its entire population was withheld from the archive"
            )
        };
        if let Some(archive_loss) = archive_loss {
            message.push_str(&format!(
                "; archiving the valid spool prefix also stopped ({archive_loss})"
            ));
        }
        cfg.reporter
            .emit(|| RunEvent::Warning(observe::Warning::CoverageUnavailable { message }));
    }

    if let Some(why) = journal.broken() {
        let message = format!("the corpus journal could not be written ({why})");
        cfg.reporter
            .emit(|| RunEvent::Warning(observe::Warning::CoverageUnavailable { message }));
    }

    let oracle = harness.oracle_status();
    crate::campaign::warn_oracle_unsupported(&cfg.reporter, oracle);

    let report = CampaignReport {
        engine: Some(generator_name),
        corpus_snapshot_interval: cfg.corpus_snapshot_interval,
        findings: Crashes::observed(crashes, filed, post_failed),
        corpus: (!matches!(cfg.budget, Budget::Build)).then(|| Corpus {
            artifacts: kept.len(),
            unique: kept.len(),
            dir: cfg.corpus.clone(),
        }),
        retained: retained_artifacts,
        tally: harness.tally(),
        oracle,
        internal: Some(LoopStats {
            functions: source.per_function().map(|generated| {
                generated
                    .into_iter()
                    .map(|mut function| {
                        if let Some((function_id, engine)) = dropped_input
                            && function.id == function_id
                            && let Some(member) = function
                                .members
                                .iter_mut()
                                .find(|member| member.engine == engine)
                        {
                            member.inputs = member.inputs.saturating_sub(1);
                        }
                        if let Some(executed) = functions.get(&function.id) {
                            function.absorb_execution(executed);
                        }
                        function
                    })
                    .collect()
            }),
            timing,
            inputs,
            rejected,
            heap: region,
            retention_complete: retention_complete(&spool, &journal),
            rejections: Some(source.rejections()),
        }),
        external: None,
    };

    if let Some(failure) = generator_failure {
        return Err(CampaignError::WithReport {
            source: Box::new(failure.into_campaign(generator_name)),
            report: Box::new(report),
        });
    }
    if let Some(error) = campaign_failure {
        return Err(error);
    }

    Ok(report)
}
