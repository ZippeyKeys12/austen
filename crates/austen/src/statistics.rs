use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

use austen_lib::observe::report::{FunctionReport, SiteReport};

use crate::args::StatBlock;

const BLOCKS: [(StatBlock, &str); 5] = [
    (StatBlock::Backtracks, "BACKTRACKING"),
    (StatBlock::Satisfaction, "SATISFACTION"),
    (StatBlock::Sizes, "SIZE"),
    (StatBlock::Discards, "DISCARD"),
    (StatBlock::Timing, "TIMING"),
];

pub(crate) fn render(
    functions: Option<&[FunctionReport]>,
    requested: &[StatBlock],
    dsl_log_dir: Option<&Path>,
) -> String {
    let mut out = String::new();
    for (block, title) in BLOCKS {
        if !requested.contains(&block) && !requested.contains(&StatBlock::All) {
            continue;
        }
        writeln!(out, "\n=== {title} STATISTICS ===").unwrap();
        let Some(functions) = functions else {
            writeln!(out, "  Per-function measurements unavailable for this run.").unwrap();
            continue;
        };
        if matches!(block, StatBlock::Backtracks | StatBlock::Satisfaction) {
            let snapshot = dsl_log_dir.map_or_else(
                || "<dir>/annotate_pointees.log (write with --dsl-log-dir=<dir>)".to_owned(),
                |dir| dir.join("annotate_pointees.log").display().to_string(),
            );
            writeln!(
                out,
                "  Sites: definition + zero-based pre-order ordinal in {snapshot}"
            )
            .unwrap();
        }
        for function in functions {
            writeln!(out, "{} (id {}):", function.name, function.id).unwrap();
            match block {
                StatBlock::Backtracks => backtracks(&mut out, function),
                StatBlock::Satisfaction => satisfaction(&mut out, function),
                StatBlock::Sizes => {
                    histogram(
                        &mut out,
                        "Memory allocated (bytes)",
                        &function.sizes.owned_bytes,
                        "inputs",
                    );
                    histogram(
                        &mut out,
                        "Test-case size",
                        &function.sizes.test_case_size,
                        "attempts",
                    );
                }
                StatBlock::Discards => {
                    writeln!(
                        out,
                        "  {} inputs, {} exhausted generation attempts, {} dispatched",
                        function.inputs, function.rejected, function.dispatched
                    )
                    .unwrap();
                    writeln!(out, "  {} precondition failures, {} parse rejections, {} crashes ({} postcondition failures)",
                        function.pre_failed, function.parse_rejected, function.crashes, function.post_failed).unwrap();
                    for (reason, count) in &function.rejections {
                        writeln!(out, "  {reason}: {count} failures raised").unwrap();
                    }
                    if function.gave_up {
                        writeln!(out, "  FAILED TO GENERATE VALID INPUT (no input after at least 100 exhausted attempts)").unwrap();
                    }
                    for member in function.members.iter().flatten() {
                        writeln!(
                            out,
                            "  {}: {} inputs, {} exhausted generation attempts{}",
                            member.engine,
                            member.inputs,
                            member.rejected,
                            if member.retired { ", retired" } else { "" }
                        )
                        .unwrap();
                    }
                }
                StatBlock::Timing => {
                    for (event, timing) in &function.timing {
                        writeln!(out, "  {event}: count {}, min {:.6} s, max {:.6} s, avg {:.6} s, last {:.6} s",
                            timing.count, timing.min, timing.max, timing.avg, timing.last).unwrap();
                    }
                    if function.timing.is_empty() {
                        writeln!(out, "  No timing data").unwrap();
                    }
                }
                StatBlock::All => unreachable!("only concrete blocks are iterated"),
            }
        }
    }
    out
}

fn backtracks(out: &mut String, function: &FunctionReport) {
    let total = austen_lib::input::backtrack_total(
        function
            .rejections
            .iter()
            .map(|(reason, count)| (reason.as_str(), *count)),
    );
    writeln!(out, "  {total} backtracks").unwrap();
    let mut generators = BTreeMap::<&str, u64>::new();
    for site in &function.sites {
        *generators.entry(&site.def).or_default() += site.failed;
    }
    let mut generators: Vec<_> = generators.into_iter().collect();
    generators.sort_by_key(|(name, failed)| (std::cmp::Reverse(*failed), *name));
    writeln!(out, "  By generator (top 10):").unwrap();
    for (name, failed) in generators.into_iter().filter(|(_, n)| *n > 0).take(10) {
        writeln!(out, "    {name}: {failed} backtracks").unwrap();
    }
    writeln!(out, "  By site (top 10):").unwrap();
    for site in ranked_sites(&function.sites, |site| site.failed)
        .into_iter()
        .filter(|site| site.failed > 0)
        .take(10)
    {
        writeln!(
            out,
            "    {} @{}: {} backtracks",
            site.def, site.ordinal, site.failed
        )
        .unwrap();
    }
    for member in function.members.iter().flatten() {
        writeln!(out, "  By site, {} (top 10):", member.engine).unwrap();
        for site in ranked_sites(&member.sites, |site| site.failed)
            .into_iter()
            .filter(|site| site.failed > 0)
            .take(10)
        {
            writeln!(
                out,
                "    {} @{}: {} backtracks",
                site.def, site.ordinal, site.failed
            )
            .unwrap();
        }
    }
}

fn ranked_sites(sites: &[SiteReport], score: impl Fn(&SiteReport) -> u64) -> Vec<&SiteReport> {
    let mut sites: Vec<_> = sites.iter().collect();
    sites.sort_by_key(|site| (std::cmp::Reverse(score(site)), &site.def, site.ordinal));
    sites
}

fn satisfaction(out: &mut String, function: &FunctionReport) {
    writeln!(
        out,
        "  Site                         passed  failed  % satisfied  % hit"
    )
    .unwrap();
    let attempts = function.inputs + function.rejected;
    for site in ranked_sites(&function.sites, |site| site.failed + site.passed)
        .into_iter()
        .take(10)
    {
        let hits = site.passed + site.failed;
        writeln!(
            out,
            "    {} @{}: {}  {}  {:.2}%  {:.2}%",
            site.def,
            site.ordinal,
            site.passed,
            site.failed,
            percent(site.passed, hits),
            percent(hits, attempts)
        )
        .unwrap();
    }
    if function.sites.is_empty() {
        writeln!(out, "  No site data").unwrap();
    }
    writeln!(
        out,
        "  % hit counts visits per generation attempt, including retries."
    )
    .unwrap();
}

fn percent(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 * 100.0 / denominator as f64
    }
}

fn histogram(out: &mut String, name: &str, samples: &BTreeMap<u64, u64>, unit: &str) {
    writeln!(out, "  {name}:").unwrap();
    for (value, count) in samples {
        writeln!(out, "    {value}: {count} {unit}").unwrap();
    }
    if samples.is_empty() {
        writeln!(out, "    No samples").unwrap();
    }
}
