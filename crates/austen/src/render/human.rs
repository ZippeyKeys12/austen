use austen_lib::observe;
use std::time::{Duration, Instant};

use anstream::AutoStream;
use austen_lib::observe::{
    CampaignSnapshot, Outcome, Phase, RunEvent, RunObserver, RunStarted, Warning,
};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

use super::{Output, When, style};

const REDRAW_HZ: u8 = (1000 / observe::SNAPSHOT_TICK.as_millis() as u64) as u8;

const STATUS_INTERVAL: Duration = Duration::from_secs(30);

pub struct HumanRenderer {
    multi: MultiProgress,
    campaign: std::sync::Mutex<Option<ProgressBar>>,
    dynamic: bool,
    color: bool,
    next_status: std::sync::Mutex<Instant>,
    announced_crash: std::sync::atomic::AtomicBool,
}

impl HumanRenderer {
    pub fn new(out: &Output) -> Self {
        let dynamic = out.dynamic();
        let multi = MultiProgress::with_draw_target(if dynamic {
            ProgressDrawTarget::stderr_with_hz(REDRAW_HZ)
        } else {
            ProgressDrawTarget::hidden()
        });
        Self {
            multi,
            campaign: std::sync::Mutex::new(None),
            dynamic,
            color: colors_on(out),
            next_status: std::sync::Mutex::new(Instant::now() + STATUS_INTERVAL),
            announced_crash: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn line(&self, text: impl AsRef<str>) {
        if self.dynamic {
            let _ = self.multi.println(text.as_ref());
        } else {
            eprintln!("{}", text.as_ref());
        }
    }

    fn styled(&self, style: anstyle::Style, text: &str) -> String {
        if self.color {
            format!("{style}{text}{style:#}")
        } else {
            text.to_string()
        }
    }

    fn started(&self, run: &RunStarted) {
        let target = run
            .target
            .as_deref()
            .or(run.harness.as_deref())
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.line(format!(
            "{} {} {}",
            self.styled(style::HEADING, "austen"),
            run.spec.display(),
            self.styled(style::DIM, &target),
        ));
        self.line(format!(
            "  {} {}",
            self.styled(style::DIM, "run directory:"),
            run.work.display()
        ));
    }

    fn snapshot(&self, snap: &CampaignSnapshot) {
        if self.dynamic {
            if let Some(bar) = self.campaign.lock().expect("the bar lock").as_ref() {
                match (snap.budget, snap.replay_left) {
                    (Some(budget), _) => {
                        bar.set_length(budget.as_secs().max(1));
                        bar.set_position(snap.elapsed.as_secs().min(budget.as_secs()));
                    }
                    (None, Some(left)) => bar.set_position(bar.length().unwrap_or(left) - left),
                    (None, None) => bar.tick(),
                }
                bar.set_message(summarize(snap));
            }
            return;
        }
        let mut due = self.next_status.lock().expect("the status lock");
        if Instant::now() >= *due {
            *due = Instant::now() + STATUS_INTERVAL;
            self.line(format!("  {}", summarize(snap)));
        }
    }

    fn campaign_message(&self, message: String) {
        if self.dynamic
            && let Some(bar) = self.campaign.lock().expect("lock").as_ref()
        {
            bar.set_message(message);
        }
    }

    fn warn(&self, warning: &Warning) {
        self.line(format!(
            "{} {}",
            self.styled(style::WARN, "austen: warning:"),
            warning_text(warning)
        ));
    }
}

fn warning_text(warning: &Warning) -> String {
    match warning {
        Warning::StaleRuntime {
            dir,
            installed,
            running,
        } => format!(
            "the runtime in {} was installed by austen {installed}, but this is \
                 {running}; reinstall with `cargo install --path crates/austen`",
            dir.display()
        ),
        Warning::ForeignRuntime {
            dir,
            staged_from,
            running_from,
            overridden: true,
        } => format!(
            "AUSTEN_RUNTIME points at {}, staged from {staged_from} rather than this \
                 austen's checkout ({running_from}); that is the documented way to test an \
                 uninstalled build — unset it to link this checkout's own install",
            dir.display()
        ),
        Warning::ForeignRuntime {
            dir,
            staged_from,
            running_from,
            overridden: false,
        } => format!(
            "the INSTALLED runtime in {} claims it was staged from {staged_from}, but this \
                 austen was built in {running_from} — that should be unreachable, since every \
                 checkout stages into its own keyed directory, so something else wrote that \
                 directory; reinstall with `cargo install --path crates/austen` and check \
                 what shares the prefix",
            dir.display()
        ),
        Warning::FindingsCapped { unique, replaying } => format!(
            "{unique} distinct crash inputs, replaying the first {replaying} \
                 (the rest are imported but unclassified)"
        ),
        Warning::ReplayFailed { message } => {
            format!("a crash replay could not run ({message}); counting it as harness-error")
        }
        Warning::HeapBaseMoved { base, default } => format!(
            "the pointer heap is at 0x{base:x}, not the default 0x{default:x}; \
                 absolute-pointer tapes from this run replay only against a harness \
                 whose heap sits there"
        ),
        Warning::TycheWriteFailed { path, message } => {
            format!("could not write Tyche log {}: {message}", path.display())
        }
        Warning::ReportWriteFailed { path, message } => {
            format!("could not write {}: {message}", path.display())
        }
        Warning::UninstrumentedTarget {
            target,
            engine,
            inert: None,
        } => {
            let engine = crate::engine_label(engine);
            format!(
                "{} carries no coverage instrumentation {engine} can steer by — \
                     austen can only instrument a `.c` source it compiles itself",
                target.display()
            )
        }
        Warning::UninstrumentedTarget {
            target,
            engine,
            inert: Some(inert),
        } => {
            let engine = crate::engine_label(engine);
            let fix = if inert.usable_by.is_empty() {
                String::new()
            } else {
                format!(
                    " ({} would)",
                    inert
                        .usable_by
                        .iter()
                        .map(|e| crate::engine_label(e))
                        .collect::<Vec<_>>()
                        .join(" or ")
                )
            };
            format!(
                "{} carries `{}` instrumentation, which {engine} does not \
                     steer by{fix} — running blind",
                target.display(),
                inert.carries,
            )
        }
        Warning::InstrumentationUnknown { target, message } => format!(
            "could not read {}'s symbol table ({message}); reporting the \
                 build's intent rather than a measurement",
            target.display()
        ),
        Warning::StaticFunctionInPrebuiltArchive { function } => format!(
            "archive contains static function `{function}`, but Austen cannot access static \
             functions in a prebuilt archive"
        ),
        Warning::ExternalConfigurationFallback {
            engine,
            from,
            to,
            message,
        } => format!(
            "{} fell back from `{from}` to `{to}` ({message})",
            crate::engine_label(engine)
        ),
        Warning::HarnessKindUnknown { harness, message } => format!(
            "could not read {}'s symbol table ({message}); which minimizer \
                 can drive it is unchecked, and a bare --shrink assumes the \
                 ordinary forkserver harness — name one with --shrink=<tool> to \
                 be sure",
            harness.display()
        ),
        Warning::ShrinkFailed { input, message } => format!(
            "could not shrink {} ({message}); keeping it as it was found",
            input.display()
        ),
        Warning::ShrinkRejected { input, verdict } => format!(
            "the reduction of {} replayed as `{verdict}` rather than a crash; \
                 keeping the original",
            input.display()
        ),
        Warning::ShrinkCapped { stored, shrinking } => format!(
            "{stored} stored crash inputs, shrinking the first {shrinking} \
                 (the rest are kept as they were found)"
        ),
        Warning::SuiteMinimizationFailed {
            pass,
            outcome,
            message,
        } => match outcome {
            None => format!(
                "{} suite output failed ({message}); its published \
                     directory may be missing, incomplete, or stale",
                pass.token()
            ),
            Some(outcome) => format!(
                "{} suite minimization failed for `{outcome}` ({message}); \
                     retaining all eligible inputs for that outcome",
                pass.token()
            ),
        },
        Warning::LinkLineUnreadable { entry, message } => format!(
            "could not read {} from the harness link line ({message}); what \
                 it defines is not counted, so an unusable instrumentation may \
                 be refused rather than run blind",
            entry.display()
        ),
        Warning::CoverageUnavailable { message } => {
            format!("coverage: N/A ({message}); the run is unaffected")
        }
        Warning::FinalizationFailed { during, message } => {
            format!("{during} also failed after the campaign error: {message}")
        }
        Warning::CoverageTimingDegraded { message } => {
            format!("coverage-over-time input chronology: incomplete ({message})")
        }
        Warning::CoverageCapped { entries, replaying } => format!(
            "{entries} kept inputs, replaying the first {replaying} \
                 (so the covered count is a lower bound)"
        ),
        Warning::CoverageAliased { sites, map_bytes } => format!(
            "the evaluation binary instruments {sites} blocks into {map_bytes} \
                 map slots, so distinct blocks share an index — the covered count \
                 undercounts, and must not be compared with one taken on a target \
                 that fits"
        ),
        Warning::CoverageEntriesLost { verdicts } => {
            let what = verdicts
                .iter()
                .map(|(v, n)| format!("{n} {v}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{what} among the inputs replayed for coverage, so their edges \
                     are missing from the total"
            )
        }
        Warning::CoverageReplayFailed { entry, message } => format!(
            "could not replay {} for coverage ({message}); its edges are \
                 missing from the total",
            entry.display()
        ),
        Warning::CoverageDenominatorMissing { harness } => format!(
            "{} published a coverage page but no instrumented-site \
                 denominator, so the observed edges have no exact site count to \
                 be reported against",
            harness.display()
        ),
        Warning::CoverageEdgesWriteFailed { path, message } => format!(
            "could not write the coverage edge set to {} ({message}); the \
                 covered count stands, the id-set artifact is missing",
            path.display()
        ),
        Warning::OracleUnsupported {
            pre_unsupported,
            post_unsupported,
        } => format!(
            "the oracle could not evaluate {} judgment(s) \
                 ({pre_unsupported} precondition, {post_unsupported} postcondition); \
                 those inputs are capability evidence in the report's `oracle` \
                 block, not findings",
            pre_unsupported + post_unsupported
        ),
    }
}

impl RunObserver for HumanRenderer {
    fn event(&self, event: RunEvent) {
        match event {
            RunEvent::ToolSelected { .. } | RunEvent::BuildCommand { .. } => {}
            RunEvent::RunStarted(run) => self.started(&run),

            RunEvent::PhaseStarted { phase } => {
                if phase == Phase::Campaign {
                    let bar = self.multi.add(if self.dynamic {
                        ProgressBar::new(1)
                    } else {
                        ProgressBar::hidden()
                    });
                    bar.set_style(
                        ProgressStyle::with_template("{spinner} {prefix} {wide_msg}")
                            .expect("a valid template"),
                    );
                    bar.set_prefix(phase.label());
                    *self.campaign.lock().expect("the bar lock") = Some(bar);
                } else if !self.dynamic {
                    self.line(format!("  {}", self.styled(style::PHASE, phase.label())));
                }
            }

            RunEvent::PhaseFinished { phase, .. } => {
                if phase == Phase::Campaign
                    && let Some(bar) = self.campaign.lock().expect("the bar lock").take()
                {
                    bar.finish_and_clear();
                }
            }

            RunEvent::HarnessReused { .. } => self.line(format!(
                "  {}",
                self.styled(style::DIM, "harness up to date, not rebuilt")
            )),

            RunEvent::CompilerCoverage {
                engine,
                controls,
                flavors,
            } => {
                let text = compiler_coverage_summary(&engine, &controls, &flavors);
                self.line(format!("  {}", self.styled(style::DIM, &text)));
            }

            RunEvent::ReplayLoaded {
                path,
                inputs,
                bytes,
            } => self.line(format!(
                "  replaying {inputs} recorded input{} ({bytes} bytes of entropy) from {}",
                if inputs == 1 { "" } else { "s" },
                path.display()
            )),

            RunEvent::HeapPublished { base, size } => self.line(format!(
                "  {} base 0x{base:x}, {size} bytes",
                self.styled(style::DIM, "harness heap:")
            )),

            RunEvent::Snapshot(snap) => self.snapshot(&snap),

            RunEvent::CrashObserved { index, .. } => {
                if !self
                    .announced_crash
                    .swap(true, std::sync::atomic::Ordering::Relaxed)
                {
                    self.line(format!(
                        "  {} at input {index}",
                        self.styled(style::FINDING, "crash")
                    ));
                }
            }

            RunEvent::ValidationProgress { validated, total } => {
                self.campaign_message(format!("validated {validated}/{total}"));
            }

            RunEvent::ImportProgress { replayed, total } => {
                self.campaign_message(format!("replayed {replayed}/{total}"));
            }

            RunEvent::ShrinkProgress { shrunk, total } => {
                self.campaign_message(format!("shrunk {shrunk}/{total}"));
            }

            RunEvent::SuiteMinimizationProgress {
                pass,
                outcome,
                retained,
                total,
            } => {
                self.campaign_message(format!(
                    "{} `{outcome}` suite retained {retained}/{total}",
                    pass.token()
                ));
            }

            RunEvent::StageStarted {
                index,
                count,
                engine,
                budget_secs,
                seed,
            } => {
                let budget = budget_secs.map_or_else(
                    || "until interrupted".to_string(),
                    |secs| format!("{secs} s"),
                );
                let seed = seed.map_or_else(String::new, |seed| format!(", seed {seed:016x}"));
                self.line(format!(
                    "  {} {}",
                    self.styled(style::PHASE, &format!("stage {}/{count}:", index + 1)),
                    self.styled(style::DIM, &format!("{engine} ({budget}{seed})")),
                ));
            }

            RunEvent::Warning(warning) => self.warn(&warning),

            RunEvent::RunFinished { outcome, .. } => {
                if outcome == Outcome::Interrupted {
                    self.line(format!(
                        "  {}",
                        self.styled(style::WARN, "interrupted — reporting what ran")
                    ));
                }
            }
        }
    }
}

fn compiler_coverage_summary(engine: &str, controls: &[String], flavors: &[String]) -> String {
    let engine = crate::engine_label(engine);
    let controls = controls
        .iter()
        .map(|control| {
            if control.chars().any(char::is_control) {
                format!("{control:?}")
            } else {
                crate::harness::shell_quote(control)
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if flavors.is_empty() {
        format!("compiler coverage: none ({engine} runs blind; from {controls})")
    } else {
        format!(
            "compiler coverage: {} (supported by {engine}; from {controls})",
            flavors.join(", "),
        )
    }
}

fn summarize(snap: &CampaignSnapshot) -> String {
    let mut parts = Vec::new();
    if let Some(inputs) = snap.inputs {
        parts.push(format!("{inputs} inputs"));
        let secs = snap.elapsed.as_secs_f64();
        if secs > 0.5 {
            parts.push(format!("{:.0}/s", inputs as f64 / secs));
        }
    }
    if let Some(execs) = snap.execs {
        parts.push(format!("{execs} execs"));
    }
    if let Some(rate) = snap.execs_per_sec {
        parts.push(format!("{rate:.0}/s"));
    }
    if let Some(rejected) = snap.rejected.filter(|n| *n > 0) {
        parts.push(format!("{rejected} rejected"));
    }
    if let Some(backtracks) = snap.rejections_total {
        parts.push(format!("{backtracks} backtracks"));
    }
    if matches!(snap.kind, observe::CampaignKind::External)
        && let Some(seen) = snap.seen
    {
        parts.push(format!("{seen} seen"));
    }
    if let Some(selected) = snap.selected {
        parts.push(format!("{selected} selected"));
    }
    if let Some(dispatched) = snap.dispatched {
        parts.push(format!("{dispatched} dispatched"));
    }
    parts.push(format!("{} crashes", snap.crashes));
    parts.join("  ")
}

fn colors_on(out: &Output) -> bool {
    match out.color {
        When::Always => true,
        When::Never => false,
        When::Auto => AutoStream::choice(&std::io::stderr()) != anstream::ColorChoice::Never,
    }
}
