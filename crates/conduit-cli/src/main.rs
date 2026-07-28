use clap::{Parser, Subcommand, ValueEnum};
use conduit_core::event::Event;
use conduit_core::execute_event;
use conduit_core::execute_event_with_mode;
use conduit_core::execution::{
    AdapterOutcome, AdapterReportError, ExecutionMode, ExecutionReport, ExecutionStatus,
};
use conduit_core::replay::{
    events_from_path, ReplayContext, ReplayReport, ReplayRunOptions,
};
use conduit_core::routing::{StorageKind, load_routing, route_with_rules};
use conduit_core::runtime::config::ConduitConfig;
use conduit_core::{
    adapter_metadata_map, dependency_depth_exceeds_recommended, dependency_layers_grouped,
    dependency_layers_parallel, validate_projection_config,
    validate_routing_and_dependencies_for_event_type, ValidationReport,
    PROJECTION_DEPTH_EXCEEDS_RECOMMENDED_WARNING,
};

use conduit_core::adapter::document::loader::load_document_mappings;
use conduit_core::adapter::sql::loader::load_sql_mappings;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

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
    config_path: PathBuf,
    mappings_dir: PathBuf,
    event_path: PathBuf,
    output: OutputFormat,
) -> Result<i32, Box<dyn std::error::Error>> {
    let config = load_config(&config_path)?;
    let (sql_mappings, doc_mappings) = load_all_mappings(&mappings_dir)?;
    let routing_path = resolve_routing_path(&config_path, &config.routing.file);
    let routing_rules = load_routing(&routing_path)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    validate_projection_config(
        &config,
        &routing_rules,
        &sql_mappings,
        &doc_mappings,
    )
    .map_err(validation_io_err)?;
    let event = load_event(&event_path)?;

    let report = execute_event(&config, sql_mappings, doc_mappings, event);

    render_report(&report, output)?;

    Ok(exit_code_from_status(report.status))
}

// --------------------------------------------------
// Replay (Phase 7)
// --------------------------------------------------

fn replay_cmd(
    config_path: PathBuf,
    mappings_dir: PathBuf,
    events_path: PathBuf,
    output: OutputFormat,
    max_per_event: Option<usize>,
    progress_every: Option<usize>,
    validate_routing: bool,
) -> Result<i32, Box<dyn std::error::Error>> {
    let config = load_config(&config_path)?;
    let (sql_mappings, doc_mappings) = load_all_mappings(&mappings_dir)?;

    let routing_path = resolve_routing_path(&config_path, &config.routing.file);
    let routing_rules = load_routing(&routing_path)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    validate_projection_config(
        &config,
        &routing_rules,
        &sql_mappings,
        &doc_mappings,
    )
    .map_err(validation_io_err)?;

    let mut iter = events_from_path(&events_path)?;
    let mut ctx = ReplayContext::new(&config, routing_rules, sql_mappings, doc_mappings);

    let opts = ReplayRunOptions {
        max_per_event_summaries: max_per_event,
        progress_interval: progress_every,
        validate_routing,
    };
    let report = ctx.run_stream_with_options(&mut iter, &opts)?;

    render_replay_report(&report, output)?;

    let exit = if report.events_failed > 0 {
        1
    } else {
        0
    };
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
    config_path: PathBuf,
    event_path: PathBuf,
    mappings_dir: Option<PathBuf>,
    output: OutputFormat,
) -> Result<i32, Box<dyn std::error::Error>> {
    let config = load_config(&config_path)?;
    let event = load_event(&event_path)?;

    let routing_path = resolve_routing_path(&config_path, &config.routing.file);
    let rules = load_routing(&routing_path)?;
    if let Some(ref dir) = mappings_dir {
        let (sql, doc) = load_all_mappings(dir)?;
        validate_projection_config(&config, &rules, &sql, &doc).map_err(validation_io_err)?;
    }

    let execution_order = validate_routing_and_dependencies_for_event_type(
        &config,
        &rules,
        &event.event_type,
    )
    .map_err(validation_io_err)?;

    let adapter_ids = route_with_rules(&event, &rules);
    let routed_set: HashSet<_> = adapter_ids.iter().cloned().collect();
    let adapter_meta = adapter_metadata_map(&config);
    let layer_vec = if execution_order.is_empty() {
        Vec::new()
    } else {
        dependency_layers_parallel(&execution_order, &adapter_meta, &routed_set)
    };
    let grouped_layers = if execution_order.is_empty() {
        Vec::new()
    } else {
        dependency_layers_grouped(&execution_order, &layer_vec)
    };
    let depth_warn = dependency_depth_exceeds_recommended(&layer_vec);
    if depth_warn {
        eprintln!("{}", PROJECTION_DEPTH_EXCEEDS_RECOMMENDED_WARNING);
    }

    match output {
        OutputFormat::Text => {
            println!("--- Explain ---");
            println!("Event: {} ({})", event.event_type, event.event_id);
            println!(
                "Routed adapters: {}",
                if adapter_ids.is_empty() {
                    "(none)".to_string()
                } else {
                    adapter_ids.join(", ")
                }
            );
            println!(
                "Execution order (dependencies resolved): {}",
                if execution_order.is_empty() {
                    "(none)".to_string()
                } else {
                    execution_order.join(", ")
                }
            );
            if !grouped_layers.is_empty() {
                println!("Execution layers:");
                for (layer, adapters) in &grouped_layers {
                    println!("  Layer {}: {}", layer, adapters.join(", "));
                }
            }
            println!("----------------");
        }
        OutputFormat::Json => {
            let execution_layers: Vec<serde_json::Value> = grouped_layers
                .iter()
                .map(|(layer, adapters)| {
                    serde_json::json!({
                        "layer": layer,
                        "adapters": adapters
                    })
                })
                .collect();
            let warnings: Vec<&str> = if depth_warn {
                vec![PROJECTION_DEPTH_EXCEEDS_RECOMMENDED_WARNING]
            } else {
                Vec::new()
            };
            let json = serde_json::json!({
                "event_type": event.event_type,
                "event_id": event.event_id,
                "routed_adapters": adapter_ids,
                "execution_order": execution_order,
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
    config_path: PathBuf,
    mappings_dir: PathBuf,
    event_path: PathBuf,
    output: OutputFormat,
) -> Result<i32, Box<dyn std::error::Error>> {
    let config = load_config(&config_path)?;
    let (sql_mappings, doc_mappings) = load_all_mappings(&mappings_dir)?;
    let routing_path = resolve_routing_path(&config_path, &config.routing.file);
    let routing_rules = load_routing(&routing_path)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    validate_projection_config(
        &config,
        &routing_rules,
        &sql_mappings,
        &doc_mappings,
    )
    .map_err(validation_io_err)?;
    let event = load_event(&event_path)?;

    let report = execute_event_with_mode(
        &config,
        sql_mappings,
        doc_mappings,
        event,
        ExecutionMode::DryRun,
    );

    render_report(&report, output)?;

    Ok(exit_code_from_status(report.status))
}

// --------------------------------------------------
// Shared helpers
// --------------------------------------------------

fn load_config(path: &Path) -> Result<ConduitConfig, Box<dyn std::error::Error>> {
    let file = fs::File::open(path)?;
    let config: ConduitConfig = serde_yaml::from_reader(file)?;
    config.validate()?;
    Ok(config)
}

fn load_all_mappings(
    mappings_dir: &Path,
) -> Result<
    (
        std::collections::HashMap<String, conduit_core::adapter::sql::mapping::SqlMapping>,
        std::collections::HashMap<
            String,
            conduit_core::adapter::document::mapping::DocumentMapping,
        >,
    ),
    Box<dyn std::error::Error>,
> {
    let sql = load_sql_mappings(mappings_dir.join("sql"))?;
    let doc = load_document_mappings(mappings_dir.join("document"))?;
    Ok((sql, doc))
}

fn load_event(path: &Path) -> Result<Event, Box<dyn std::error::Error>> {
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn resolve_routing_path(config_path: &Path, routing_file: &str) -> PathBuf {
    config_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(routing_file)
}

fn validation_io_err(r: ValidationReport) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, r.to_string())
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
            AdapterOutcome::Succeeded => {
                println!(
                    "✓ {} ({}) — {} ms",
                    r.adapter_id,
                    format_storage_kind(r.storage_kind),
                    r.duration_ms.unwrap_or(0)
                );
            }
            AdapterOutcome::Skipped => {
                println!(
                    "~ {} ({}) — skipped: {}",
                    r.adapter_id,
                    format_storage_kind(r.storage_kind),
                    extract_error(r.error.as_ref())
                );
            }
            AdapterOutcome::WriteFailed => {
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
        Some(AdapterReportError::Skipped { message }) => message,
        None => "unknown",
    }
}

fn exit_code_from_status(status: ExecutionStatus) -> i32 {
    match status {
        ExecutionStatus::Succeeded => 0,
        ExecutionStatus::Failed => 1,
    }
}
