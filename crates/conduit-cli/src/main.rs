use clap::{Parser, Subcommand, ValueEnum};
use conduit_core::adapter::SkipReason;
use conduit_core::execution::{
    AdapterOutcome, AdapterReportError, ExecutionReport, ExecutionStatus,
};
use conduit_core::pipeline;
use conduit_core::replay::{ReplayReport, ReplayRunOptions};
use conduit_core::routing::StorageKind;
use conduit_core::runtime::PROJECTION_DEPTH_EXCEEDS_RECOMMENDED_WARNING;

use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "conduit")]
#[command(about = "Event-first projection engine")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    #[default]
    Text,
    Json,
}

#[derive(Subcommand)]
enum Commands {
    Run {
        #[arg(long)]
        config: PathBuf,

        #[arg(long)]
        mappings: PathBuf,

        #[arg(long)]
        event: PathBuf,

        #[arg(long, value_enum, default_value_t)]
        output: OutputFormat,
    },

    Explain {
        #[arg(long)]
        config: PathBuf,

        #[arg(long)]
        event: PathBuf,

        /// If set, run full Phase 8 validation (mappings + routing + capabilities).
        #[arg(long)]
        mappings: Option<PathBuf>,

        #[arg(long, value_enum, default_value_t)]
        output: OutputFormat,
    },

    DryRun {
        #[arg(long)]
        config: PathBuf,

        #[arg(long)]
        mappings: PathBuf,

        #[arg(long)]
        event: PathBuf,

        #[arg(long, value_enum, default_value_t)]
        output: OutputFormat,
    },

    /// Replay events from a directory (sorted *.json) or file (JSON / NDJSON)
    Replay {
        #[arg(long)]
        config: PathBuf,

        #[arg(long)]
        mappings: PathBuf,

        /// Event file or directory of event JSON files
        #[arg(long)]
        events: PathBuf,

        #[arg(long, value_enum, default_value_t)]
        output: OutputFormat,

        /// Max successful per-event rows in the report (failures always listed)
        #[arg(long)]
        max_per_event: Option<usize>,

        /// Print progress to stderr every N events
        #[arg(long)]
        progress_every: Option<usize>,

        /// Fail events with no routing rule before calling adapters
        #[arg(long, default_value_t = false)]
        validate_routing: bool,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let exit_code = match cli.command {
        Commands::Run {
            config,
            mappings,
            event,
            output,
        } => run_cmd(config, mappings, event, output)?,

        Commands::Explain {
            config,
            event,
            mappings,
            output,
        } => explain_cmd(config, event, mappings, output)?,

        Commands::DryRun {
            config,
            mappings,
            event,
            output,
        } => dry_run_cmd(config, mappings, event, output)?,

        Commands::Replay {
            config,
            mappings,
            events,
            output,
            max_per_event,
            progress_every,
            validate_routing,
        } => replay_cmd(
            config,
            mappings,
            events,
            output,
            max_per_event,
            progress_every,
            validate_routing,
        )?,
    };

    std::process::exit(exit_code);
}

// --------------------------------------------------
// Run (normal execution)
// --------------------------------------------------

fn run_cmd(
    config: PathBuf,
    mappings: PathBuf,
    event: PathBuf,
    output: OutputFormat,
) -> Result<i32, Box<dyn std::error::Error>> {
    let report = pipeline::run(&config, &mappings, &event)?;
    render_report(&report, output)?;
    Ok(exit_code_from_status(report.status))
}

// --------------------------------------------------
// Replay (Phase 7)
// --------------------------------------------------

fn replay_cmd(
    config: PathBuf,
    mappings: PathBuf,
    events: PathBuf,
    output: OutputFormat,
    max_per_event: Option<usize>,
    progress_every: Option<usize>,
    validate_routing: bool,
) -> Result<i32, Box<dyn std::error::Error>> {
    let opts = ReplayRunOptions {
        max_per_event_summaries: max_per_event,
        progress_interval: progress_every,
        validate_routing,
    };
    let report = pipeline::replay(&config, &mappings, &events, &opts)?;

    render_replay_report(&report, output)?;

    let exit = if report.events_failed > 0 { 1 } else { 0 };
    Ok(exit)
}

fn render_replay_report(
    report: &ReplayReport,
    output: OutputFormat,
) -> Result<(), Box<dyn std::error::Error>> {
    match output {
        OutputFormat::Text => {
            println!("--- Replay Report ---");
            println!(
                "Processed: {}  Succeeded: {}  Failed: {}",
                report.events_processed, report.events_succeeded, report.events_failed
            );
            println!(
                "Stopped early: {}  Duration: {} ms",
                report.stopped_early,
                report.duration_ms.unwrap_or(0)
            );
            if let Some(ref id) = report.first_failure_event_id {
                println!("First failure event_id: {}", id);
            }
            if report.per_event_summaries_omitted > 0 {
                println!(
                    "Per-event rows omitted (success cap): {}",
                    report.per_event_summaries_omitted
                );
            }
            println!();
            for e in &report.per_event {
                println!(
                    "{} ({}) — {}",
                    e.event_id,
                    e.event_type,
                    format_status(e.status)
                );
            }
            println!("---------------------");
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(report)?);
        }
    }
    Ok(())
}

// --------------------------------------------------
// Explain (routing only)
// --------------------------------------------------

fn explain_cmd(
    config: PathBuf,
    event: PathBuf,
    mappings: Option<PathBuf>,
    output: OutputFormat,
) -> Result<i32, Box<dyn std::error::Error>> {
    let result = pipeline::explain(&config, &event, mappings.as_deref())?;

    if result.depth_warning {
        eprintln!("{}", PROJECTION_DEPTH_EXCEEDS_RECOMMENDED_WARNING);
    }

    match output {
        OutputFormat::Text => {
            println!("--- Explain ---");
            println!(
                "Event: {} ({})",
                result.event.event_type, result.event.event_id
            );
            println!(
                "Routed adapters: {}",
                if result.routed_adapters.is_empty() {
                    "(none)".to_string()
                } else {
                    result.routed_adapters.join(", ")
                }
            );
            println!(
                "Execution order (dependencies resolved): {}",
                if result.execution_order.is_empty() {
                    "(none)".to_string()
                } else {
                    result.execution_order.join(", ")
                }
            );
            if !result.execution_layers.is_empty() {
                println!("Execution layers:");
                for (layer, adapters) in &result.execution_layers {
                    println!("  Layer {}: {}", layer, adapters.join(", "));
                }
            }
            println!("----------------");
        }
        OutputFormat::Json => {
            let execution_layers: Vec<serde_json::Value> = result
                .execution_layers
                .iter()
                .map(|(layer, adapters)| {
                    serde_json::json!({
                        "layer": layer,
                        "adapters": adapters
                    })
                })
                .collect();
            let warnings: Vec<&str> = if result.depth_warning {
                vec![PROJECTION_DEPTH_EXCEEDS_RECOMMENDED_WARNING]
            } else {
                Vec::new()
            };
            let json = serde_json::json!({
                "event_type": result.event.event_type,
                "event_id": result.event.event_id,
                "routed_adapters": result.routed_adapters,
                "execution_order": result.execution_order,
                "execution_layers": execution_layers,
                "warnings": warnings
            });
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
    }

    Ok(0)
}

// --------------------------------------------------
// Dry-run (full simulated execution)
// --------------------------------------------------

fn dry_run_cmd(
    config: PathBuf,
    mappings: PathBuf,
    event: PathBuf,
    output: OutputFormat,
) -> Result<i32, Box<dyn std::error::Error>> {
    let report = pipeline::dry_run(&config, &mappings, &event)?;
    render_report(&report, output)?;
    Ok(exit_code_from_status(report.status))
}

// --------------------------------------------------
// Rendering
// --------------------------------------------------

fn render_report(
    report: &ExecutionReport,
    output: OutputFormat,
) -> Result<(), Box<dyn std::error::Error>> {
    match output {
        OutputFormat::Text => print_text_report(report),
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(report)?);
        }
    }
    Ok(())
}

fn print_text_report(report: &ExecutionReport) {
    println!("--- Execution Report ---");
    println!("Event: {} ({})", report.event_type, report.event_id);
    println!("Trace: {}", report.trace_id);
    println!("Status: {}", format_status(report.status));
    println!("Duration: {} ms", report.duration_ms.unwrap_or(0));
    println!();

    for r in &report.adapter_reports {
        match r.outcome {
            AdapterOutcome::Created => {
                println!(
                    "✓ {} ({}) — created — {} ms",
                    r.adapter_id,
                    format_storage_kind(r.storage_kind),
                    r.duration_ms.unwrap_or(0)
                );
            }
            AdapterOutcome::Updated => {
                println!(
                    "✓ {} ({}) — updated — {} ms",
                    r.adapter_id,
                    format_storage_kind(r.storage_kind),
                    r.duration_ms.unwrap_or(0)
                );
            }
            AdapterOutcome::Deleted => {
                println!(
                    "✓ {} ({}) — deleted — {} ms",
                    r.adapter_id,
                    format_storage_kind(r.storage_kind),
                    r.duration_ms.unwrap_or(0)
                );
            }
            AdapterOutcome::Skipped { reason } => {
                println!(
                    "~ {} ({}) — skipped: {}",
                    r.adapter_id,
                    format_storage_kind(r.storage_kind),
                    format_skip_reason(reason)
                );
            }
            AdapterOutcome::Failed => {
                println!(
                    "✗ {} ({}) — {}",
                    r.adapter_id,
                    format_storage_kind(r.storage_kind),
                    extract_error(r.error.as_ref())
                );
            }
        }
    }

    println!("------------------------");
}

fn format_status(status: ExecutionStatus) -> &'static str {
    match status {
        ExecutionStatus::Succeeded => "succeeded",
        ExecutionStatus::Failed => "failed",
    }
}

fn format_storage_kind(kind: StorageKind) -> &'static str {
    match kind {
        StorageKind::Sql => "sql",
        StorageKind::Document => "document",
        StorageKind::KeyValue => "keyvalue",
        StorageKind::Graph => "graph",
    }
}

fn extract_error(err: Option<&AdapterReportError>) -> &str {
    match err {
        Some(AdapterReportError::WriteFailed { message }) => message,
        Some(AdapterReportError::UnsupportedVersion { message }) => message,
        None => "unknown",
    }
}

fn format_skip_reason(reason: SkipReason) -> &'static str {
    match reason {
        SkipReason::AlreadyProjected => "already projected",
        SkipReason::StaleSequence => "stale sequence",
        SkipReason::UnsupportedVersion => "unsupported version: no upcaster chain",
        SkipReason::AlreadyDeleted => "already deleted",
        SkipReason::Tombstoned => "permanently tombstoned",
        SkipReason::EntityAbsent => "entity absent (facet update with no created entity)",
    }
}

fn exit_code_from_status(status: ExecutionStatus) -> i32 {
    match status {
        ExecutionStatus::Succeeded => 0,
        ExecutionStatus::Failed => 1,
    }
}
