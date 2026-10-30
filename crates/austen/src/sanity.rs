use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::PathBuf;

use austen_lib::spec::sanity::{
    AnalysisLimitation, IrLocation, SanityOptions, SanityPhase, SanityReport, Severity,
};

use crate::args::{SanityArgs, SanityFormat};

#[derive(Debug, thiserror::Error)]
pub(crate) enum SanityCommandError {
    #[error("read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: austen_lib::spec::SpecError,
    },
    #[error("the specification in {path} declares function {function} more than once")]
    DuplicateFunction { path: PathBuf, function: String },
    #[error("the specification in {path} has no function named {function}")]
    NoSuchFunction { path: PathBuf, function: String },
    #[error("serialize sanity report: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("write {stream}: {source}")]
    Write {
        stream: &'static str,
        #[source]
        source: std::io::Error,
    },
}

pub(crate) fn run(args: &SanityArgs) -> Result<SanityReport, SanityCommandError> {
    let text = std::fs::read_to_string(&args.spec).map_err(|source| SanityCommandError::Read {
        path: args.spec.clone(),
        source,
    })?;
    let module = austen_lib::spec::SpecModule::from_json(&text).map_err(|source| {
        SanityCommandError::Parse {
            path: args.spec.clone(),
            source,
        }
    })?;

    let mut names = BTreeSet::new();
    for function in &module.functions {
        if !names.insert(function.name.as_str()) {
            return Err(SanityCommandError::DuplicateFunction {
                path: args.spec.clone(),
                function: function.name.clone(),
            });
        }
    }
    if let Some(wanted) = &args.function
        && !names.contains(wanted.as_str())
    {
        return Err(SanityCommandError::NoSuchFunction {
            path: args.spec.clone(),
            function: wanted.clone(),
        });
    }

    Ok(austen_lib::spec::sanity::analyze_module(
        &module,
        &SanityOptions {
            function: args.function.clone(),
            pedantic: args.pedantic,
            suppress: args.suppress.clone(),
            ..SanityOptions::default()
        },
    ))
}

pub(crate) fn render(
    format: SanityFormat,
    report: &SanityReport,
) -> Result<(), SanityCommandError> {
    match format {
        SanityFormat::Text => {
            let text = render_text(report);
            std::io::stderr()
                .lock()
                .write_all(text.as_bytes())
                .map_err(|source| SanityCommandError::Write {
                    stream: "stderr",
                    source,
                })
        }
        SanityFormat::Json => {
            let value = serde_json::to_value(report)?;
            let text = format!("{value}\n");
            std::io::stdout()
                .lock()
                .write_all(text.as_bytes())
                .map_err(|source| SanityCommandError::Write {
                    stream: "stdout",
                    source,
                })
        }
    }
}

fn render_text(report: &SanityReport) -> String {
    let mut out = String::new();
    let mut warnings = 0usize;
    let mut informational = 0usize;

    for diagnostic in report.visible_diagnostics() {
        match diagnostic.severity {
            Severity::Warning => warnings += 1,
            Severity::Informational => informational += 1,
        }
        let subject = diagnostic
            .subject
            .as_ref()
            .map(|subject| format!(" ({})", subject.display_name()))
            .unwrap_or_default();
        writeln!(
            out,
            "{}: {}[{}]{subject}: {}",
            diagnostic.function,
            severity_name(diagnostic.severity),
            diagnostic.code,
            diagnostic.message,
        )
        .expect("writing to a String cannot fail");
        writeln!(out, "  {}", diagnostic.explanation).expect("writing to a String cannot fail");
        for location in &diagnostic.locations {
            writeln!(out, "  at {}", display_location(location))
                .expect("writing to a String cannot fail");
        }
    }

    for limitation in &report.limitations {
        render_limitation(&mut out, limitation);
    }

    let suppressed = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.suppressed)
        .count();
    writeln!(
        out,
        "sanity: {warnings} warning{}, {informational} informational finding{}, {} analysis limitation{}, {suppressed} suppressed across {} function{}",
        plural(warnings),
        plural(informational),
        report.limitations.len(),
        plural(report.limitations.len()),
        report.functions.len(),
        plural(report.functions.len()),
    )
    .expect("writing to a String cannot fail");
    out.push_str(
        "note: these are syntactic signals, not proofs that a specification is complete or incorrect.\n",
    );
    out
}

fn render_limitation(out: &mut String, limitation: &AnalysisLimitation) {
    let function = limitation.function.as_deref().unwrap_or("<module>");
    let subject = limitation
        .subject
        .as_ref()
        .map(|subject| format!(" ({})", subject.display_name()))
        .unwrap_or_default();
    writeln!(
        out,
        "{function}: limitation[{}]{subject}: {}",
        limitation.code, limitation.message,
    )
    .expect("writing to a String cannot fail");
    writeln!(out, "  at {}", display_location(&limitation.location))
        .expect("writing to a String cannot fail");
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Informational => "informational",
        Severity::Warning => "warning",
    }
}

fn display_location(location: &IrLocation) -> String {
    let phase = match location.phase {
        SanityPhase::Signature => "signature",
        SanityPhase::Precondition => "precondition",
        SanityPhase::Postcondition => "postcondition",
        SanityPhase::Predicate => "predicate",
        SanityPhase::LogicalFunction => "logical-function",
    };
    match &location.function {
        Some(function) => format!("{function}/{phase}/{}", location.path),
        None => format!("{phase}/{}", location.path),
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}
