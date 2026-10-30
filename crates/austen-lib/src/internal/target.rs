use core::time::Duration;
use std::path::Path;

use crate::gen_dsl::{GenModule, Incoming, incoming_from_published};
use crate::observe::{Reporter, RunEvent, Warning};
use crate::wire::{
    AustenInput, DEBUG_CHILD_ENV, DEFAULT_HEAP_BASE, EXTERNAL_INPUT_BYTES, INCOMING_ENV,
    IncomingInfo, IncomingPollError, ORACLE_STATUS_ENV, OracleStatus, OracleStatusReader,
    PUBLISH_TIMEOUT, SHM_ENV_VAR, SHM_FUZZ_ENV_VAR, Tally, TallyReader, Trust,
};

use libafl::{
    corpus::InMemoryCorpus,
    events::SimpleEventManager,
    executors::{Executor, StdChildArgs, forkserver::ForkserverExecutor},
    feedbacks::{CrashFeedback, MaxMapFeedback},
    fuzzer::StdFuzzer,
    monitors::{Monitor, stats::ClientStatsManager},
    observers::{CanTrack, HitcountsMapObserver, ObserversTuple, StdMapObserver},
    schedulers::QueueScheduler,
    state::StdState,
};
use libafl_bolts::{
    ClientId, InputLocation, StdTargetArgs, ToSliceMut,
    fs::InputFile,
    rands::StdRand,
    shmem::{ShMem, ShMemProvider, UnixShMemProvider},
    tuples::tuple_list,
};
use libafl_targets::EDGES_MAP_DEFAULT_SIZE;

use crate::campaign::{CampaignError, LibAflStep};

const RUN_TIMEOUT: Duration = crate::wire::INPUT_TIMEOUT;

fn reclaim_forkserver_shmem(coverage: i32, inherited: Option<&str>) {
    fn mark(shm: i32) {
        unsafe { libc::shmctl(shm, libc::IPC_RMID, core::ptr::null_mut()) };
    }
    mark(coverage);
    if let Ok(announced) = std::env::var(SHM_FUZZ_ENV_VAR)
        && Some(announced.as_str()) != inherited
        && let Ok(input) = announced.parse()
    {
        mark(input);
    }
}

fn harness_contract_env(
    generator: &Path,
    heap_bytes: u64,
    incoming: &Path,
    tally: &Path,
    trust: Trust,
    allow_unstable_addresses: bool,
) -> Vec<(String, String)> {
    let mut env = crate::external::harness_env_for(
        generator,
        trust,
        heap_bytes,
        tally,
        allow_unstable_addresses,
    );
    env.push((INCOMING_ENV.to_string(), incoming.display().to_string()));
    env
}

pub struct Spawn<'a> {
    pub harness: &'a Path,
    pub generator_path: &'a Path,
    pub trust: Trust,
    pub work: &'a Path,
    pub heap_bytes: u64,
    pub allow_unstable_addresses: bool,
    pub debug_child: bool,
    pub coverage: bool,
    pub coverage_guards: Option<u64>,
    pub reporter: &'a Reporter,
    pub generator_module: &'a GenModule,
}

type DriverState =
    StdState<InMemoryCorpus<AustenInput>, AustenInput, StdRand, InMemoryCorpus<AustenInput>>;

trait Runner {
    fn run(&mut self, input: &AustenInput) -> Result<libafl::executors::ExitKind, libafl::Error>;
    fn worker(&self) -> Option<WorkerVerdict>;
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerVerdict {
    Stopped(i32),
    Exited(i32),
    Signaled,
    TimedOut,
}

fn worker_verdict(
    status: i32,
    timed_out: bool,
    stopped_pid: impl FnOnce() -> i32,
) -> Option<WorkerVerdict> {
    if timed_out {
        return Some(WorkerVerdict::TimedOut);
    }

    if libc::WIFSTOPPED(status) {
        Some(WorkerVerdict::Stopped(stopped_pid()))
    } else if libc::WIFSIGNALED(status) {
        Some(WorkerVerdict::Signaled)
    } else if libc::WIFEXITED(status) {
        Some(WorkerVerdict::Exited(libc::WEXITSTATUS(status)))
    } else {
        None
    }
}

trait WorkerObserved {
    fn worker(&self) -> Option<WorkerVerdict>;
}

impl<I, OT, S, SHM> WorkerObserved for ForkserverExecutor<I, OT, S, SHM>
where
    OT: ObserversTuple<I, S>,
    SHM: ShMem,
{
    fn worker(&self) -> Option<WorkerVerdict> {
        let forkserver = self.forkserver();
        worker_verdict(forkserver.status(), forkserver.last_run_timed_out(), || {
            forkserver.child_pid().as_raw()
        })
    }
}

struct Live<E, EM, S, Z> {
    executor: E,
    mgr: EM,
    state: S,
    fuzzer: Z,
}

impl<E, EM, S, Z> Runner for Live<E, EM, S, Z>
where
    E: Executor<EM, AustenInput, S, Z> + WorkerObserved,
{
    fn run(&mut self, input: &AustenInput) -> Result<libafl::executors::ExitKind, libafl::Error> {
        self.executor
            .run_target(&mut self.fuzzer, &mut self.state, &mut self.mgr, input)
    }

    fn worker(&self) -> Option<WorkerVerdict> {
        self.executor.worker()
    }
}

#[derive(Debug)]
struct TracingMonitor;

impl Monitor for TracingMonitor {
    fn display(
        &mut self,
        stats: &mut ClientStatsManager,
        event_msg: &str,
        sender_id: ClientId,
    ) -> Result<(), libafl::Error> {
        let global = stats.global_stats();
        tracing::trace!(
            target: "austen::campaign",
            event = event_msg,
            client = sender_id.0,
            corpus = global.corpus_size,
            objectives = global.objective_size,
            execs = global.total_execs,
            execs_per_sec = global.execs_per_sec,
        );
        Ok(())
    }
}

fn incoming_from_info(ctx: &GenModule, info: &IncomingInfo) -> Result<Incoming, String> {
    incoming_from_published(
        ctx,
        "harness",
        &info.layout,
        info.heap,
        info.globals.iter().cloned(),
    )
}

pub struct Harness {
    runner: Box<dyn Runner>,
    incoming: Incoming,
    tally: TallyReader,
    oracle_status: OracleStatusReader,
    ran: usize,
    last_worker: Option<WorkerVerdict>,
    map: Option<(*mut u8, usize)>,
}

#[derive(Clone, Copy)]
enum GuardRegistration {
    Once,
    MayRepeat,
}

impl GuardRegistration {
    const HOST: Self = if cfg!(target_os = "macos") {
        Self::MayRepeat
    } else {
        Self::Once
    };
}

fn live_map_extent(guards: Option<u64>, map_len: usize, registration: GuardRegistration) -> usize {
    match (registration, guards) {
        (GuardRegistration::Once, Some(guards)) if guards > 0 => {
            usize::try_from(guards).map_or(map_len, |guards| guards.min(map_len))
        }
        _ => map_len,
    }
}

impl Harness {
    pub fn spawn(cfg: &Spawn<'_>) -> Result<Self, CampaignError> {
        let _phase = cfg.reporter.phase(crate::observe::Phase::Spawn);
        const MAP_SIZE: usize = EDGES_MAP_DEFAULT_SIZE;
        let mut shmem_provider = UnixShMemProvider::new().map_err(|e| CampaignError::LibAfl {
            during: LibAflStep::ShmemProvider,
            message: e.to_string(),
        })?;
        let shmem = Box::leak(Box::new(shmem_provider.new_shmem(MAP_SIZE).map_err(
            |e| CampaignError::LibAfl {
                during: LibAflStep::CoverageShmem,
                message: e.to_string(),
            },
        )?));
        let coverage_shm: i32 = shmem.id().into();
        let inherited_input_shm = std::env::var(SHM_FUZZ_ENV_VAR).ok();
        unsafe {
            shmem
                .write_to_env(SHM_ENV_VAR)
                .map_err(|e| CampaignError::LibAfl {
                    during: LibAflStep::ExportShmId,
                    message: e.to_string(),
                })?;
        }
        let shmem_buf = shmem.to_slice_mut();
        let map: *mut u8 = shmem_buf.as_mut_ptr();
        let map_len = shmem_buf.len();

        let edges_observer = unsafe {
            HitcountsMapObserver::new(StdMapObserver::new("edges", shmem_buf)).track_indices()
        };

        let mut feedback = MaxMapFeedback::new(&edges_observer);
        let mut objective = CrashFeedback::new();

        let state = StdState::new(
            StdRand::new(),
            InMemoryCorpus::<AustenInput>::new(),
            InMemoryCorpus::<AustenInput>::new(),
            &mut feedback,
            &mut objective,
        )
        .map_err(|e| CampaignError::LibAfl {
            during: LibAflStep::State,
            message: e.to_string(),
        })?;

        let mgr: SimpleEventManager<AustenInput, _, DriverState> =
            SimpleEventManager::new(TracingMonitor);
        let scheduler = QueueScheduler::new();
        let fuzzer = StdFuzzer::new(scheduler, feedback, objective);

        let info_path = cfg.work.join("incoming_info");
        let _ = std::fs::remove_file(&info_path);
        let tally_path = cfg.work.join("tally");
        Tally::stage(&tally_path).map_err(|source| CampaignError::Io {
            path: tally_path.clone(),
            source,
        })?;
        let mut child_env = harness_contract_env(
            cfg.generator_path,
            cfg.heap_bytes,
            &info_path,
            &tally_path,
            cfg.trust,
            cfg.allow_unstable_addresses,
        );
        let tally = TallyReader::open(tally_path);
        let oracle_status_path = cfg.work.join("oracle-status");
        OracleStatus::stage(&oracle_status_path).map_err(|source| CampaignError::Io {
            path: oracle_status_path.clone(),
            source,
        })?;
        child_env.push((
            ORACLE_STATUS_ENV.to_string(),
            oracle_status_path.display().to_string(),
        ));
        let oracle_status = OracleStatusReader::open(oracle_status_path);
        if cfg.debug_child {
            child_env.push((DEBUG_CHILD_ENV.to_string(), "1".to_string()));
        }

        let input_path = cfg.work.join("cur_input");
        let input_file = InputFile::create(&input_path).map_err(|e| CampaignError::LibAfl {
            during: LibAflStep::BuildExecutor,
            message: format!("creating {}: {e}", input_path.display()),
        })?;

        let executor = ForkserverExecutor::builder()
            .program(cfg.harness)
            .input(InputLocation::StdIn {
                input_file: Some(input_file),
            })
            .debug_child(cfg.debug_child)
            .envs(child_env)
            .shmem_provider(&mut shmem_provider)
            .coverage_map_size(MAP_SIZE)
            .max_input_size(EXTERNAL_INPUT_BYTES)
            .min_input_size(0)
            .is_persistent(true)
            .timeout(RUN_TIMEOUT)
            .build(tuple_list!(edges_observer));
        reclaim_forkserver_shmem(coverage_shm, inherited_input_shm.as_deref());
        let executor = executor.map_err(|e| CampaignError::LibAfl {
            during: LibAflStep::BuildExecutor,
            message: format!(
                "{e}; harness startup failed. Set {DEBUG_CHILD_ENV}=1 for its diagnostics. \
                 If fixed heap placement failed, choose another {}, or unset it and pass \
                 --allow-unstable-addresses",
                crate::wire::HEAP_BASE_ENV,
            ),
        })?;

        let info = IncomingInfo::poll(&info_path, PUBLISH_TIMEOUT).map_err(|e| match e {
            IncomingPollError::Timeout { path, timeout } => {
                CampaignError::HeapTimeout { path, timeout }
            }
            IncomingPollError::Malformed { line, source } => CampaignError::HeapMalformed {
                line,
                reason: source.to_string(),
            },
        })?;
        let incoming = incoming_from_info(cfg.generator_module, &info).map_err(|reason| {
            CampaignError::HeapMalformed {
                line: info.to_line(),
                reason,
            }
        })?;
        let region = incoming.heap();
        cfg.reporter.emit(|| RunEvent::HeapPublished {
            base: region.base,
            size: region.size,
        });
        if !crate::wire::is_default_base(region.base) {
            cfg.reporter.emit(|| {
                RunEvent::Warning(Warning::HeapBaseMoved {
                    base: region.base,
                    default: DEFAULT_HEAP_BASE,
                })
            });
        }

        let live_len = live_map_extent(cfg.coverage_guards, map_len, GuardRegistration::HOST);
        tracing::debug!(
            target: "austen::campaign",
            declared_guards = cfg.coverage_guards,
            live_bytes = live_len,
            map_bytes = map_len,
            "coverage map live extent"
        );

        Ok(Self {
            runner: Box::new(Live {
                executor,
                mgr,
                state,
                fuzzer,
            }),
            incoming,
            tally,
            oracle_status,
            ran: 0,
            last_worker: None,
            map: cfg.coverage.then_some((map, live_len)),
        })
    }

    pub fn incoming(&self) -> &Incoming {
        &self.incoming
    }

    pub fn coverage_map(&self) -> Option<&[u8]> {
        self.map
            .map(|(ptr, len)| unsafe { std::slice::from_raw_parts(ptr.cast_const(), len) })
    }

    #[doc(hidden)]
    pub fn scrub_execution_sentinel(&mut self) {
        if let Some((ptr, _)) = self.map {
            unsafe { ptr.write(0) };
        }
    }

    #[doc(hidden)]
    pub fn worker_after_last_run(&self) -> Option<WorkerVerdict> {
        self.last_worker
    }

    pub fn run(&mut self, input: &AustenInput) -> Result<bool, CampaignError> {
        self.last_worker = None;
        let before = self.oracle_status().unwrap_or_default();
        let tally_before = self.tally();
        let selected_before = tally_before.map_or(0, |tally| tally.selected);
        let dispatched_before = tally_before.map_or(0, |tally| tally.dispatched);
        let kind = self.runner.run(input).map_err(|e| CampaignError::LibAfl {
            during: LibAflStep::RunTarget,
            message: e.to_string(),
        })?;
        self.last_worker = self.runner.worker();
        let after = self.oracle_status();
        if let Some(after) = &after
            && after.pre_internal_error > before.pre_internal_error
        {
            return Err(CampaignError::OracleInternal(
                "the harness reported an internal precondition-oracle error".into(),
            ));
        }
        let observed_grew = after
            .as_ref()
            .is_some_and(|after| after.observed() > before.observed());
        let tally_after = self.tally();
        let selected_grew = tally_after.is_some_and(|tally| tally.selected > selected_before);
        let dispatched_grew = tally_after.is_some_and(|tally| tally.dispatched > dispatched_before);
        if missing_oracle_result_is_error(
            observed_grew,
            selected_grew,
            dispatched_grew,
            matches!(kind, libafl::executors::ExitKind::Timeout),
        ) {
            return Err(CampaignError::OracleInternal(
                "the harness did not report an oracle result for a selected input".into(),
            ));
        }
        tracing::trace!(
            target: "austen::input",
            index = self.ran,
            bytes = input.len(),
            outcome = ?kind,
        );
        self.ran += 1;
        Ok(matches!(kind, libafl::executors::ExitKind::Crash))
    }

    pub fn tally(&self) -> Option<Tally> {
        self.tally.read()
    }

    pub fn oracle_status(&self) -> Option<OracleStatus> {
        self.oracle_status.read()
    }
}

fn missing_oracle_result_is_error(
    observed_grew: bool,
    selected_grew: bool,
    dispatched_grew: bool,
    timed_out: bool,
) -> bool {
    !observed_grew && selected_grew && (!timed_out || dispatched_grew)
}
