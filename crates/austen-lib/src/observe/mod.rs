mod event;
pub mod report;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

pub use event::{
    CampaignKind, CampaignSnapshot, InertCoverage, Outcome, Phase, RunEvent, RunMode, RunStarted,
    SNAPSHOT_TICK, SuitePass, Warning,
};

pub trait RunObserver: Send + Sync {
    fn event(&self, event: RunEvent);
}

#[derive(Clone, Default)]
pub struct Reporter(Option<Arc<dyn RunObserver>>);

impl Reporter {
    pub fn new(observer: Arc<dyn RunObserver>) -> Self {
        Self(Some(observer))
    }

    pub fn silent() -> Self {
        Self(None)
    }

    pub fn is_silent(&self) -> bool {
        self.0.is_none()
    }

    #[inline]
    pub fn emit(&self, event: impl FnOnce() -> RunEvent) {
        if let Some(observer) = &self.0 {
            observer.event(event());
        }
    }

    pub fn phase(&self, phase: Phase) -> PhaseGuard<'_> {
        self.emit(|| RunEvent::PhaseStarted { phase });
        PhaseGuard {
            reporter: self,
            phase,
            started: std::time::Instant::now(),
        }
    }
}

impl std::fmt::Debug for Reporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.is_silent() {
            "Reporter(silent)"
        } else {
            "Reporter(live)"
        })
    }
}

#[must_use = "a phase that is dropped immediately finishes immediately"]
pub struct PhaseGuard<'a> {
    reporter: &'a Reporter,
    phase: Phase,
    started: std::time::Instant,
}

impl Drop for PhaseGuard<'_> {
    fn drop(&mut self) {
        let (phase, elapsed) = (self.phase, self.started.elapsed());
        self.reporter
            .emit(|| RunEvent::PhaseFinished { phase, elapsed });
    }
}

#[derive(Clone, Default, Debug)]
pub struct Cancel(Option<Arc<AtomicBool>>);

impl Cancel {
    pub fn new() -> Self {
        Self(Some(Arc::new(AtomicBool::new(false))))
    }

    pub fn never() -> Self {
        Self(None)
    }

    pub fn request(&self) {
        if let Some(flag) = &self.0 {
            flag.store(true, Ordering::Relaxed);
        }
    }

    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.0
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }
}

#[derive(Default)]
pub struct Collector(Mutex<Vec<RunEvent>>);

impl Collector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> Vec<RunEvent> {
        self.0.lock().expect("the collector lock").clone()
    }
}

impl RunObserver for Collector {
    fn event(&self, event: RunEvent) {
        self.0.lock().expect("the collector lock").push(event);
    }
}

pub struct Tee(pub Vec<Arc<dyn RunObserver>>);

impl RunObserver for Tee {
    fn event(&self, event: RunEvent) {
        for observer in &self.0 {
            observer.event(event.clone());
        }
    }
}
