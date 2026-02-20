use clap::{Parser, Subcommand};
use conduit_core::event::Event;
use conduit_core::execute_event;
use conduit_core::execution::{AdapterOutcome, AdapterReportError, ExecutionStatus};
use conduit_core::routing::StorageKind;
use conduit_core::runtime::config::ConduitConfig;

use std::fs;
use std::path::PathBuf;

use conduit_core::adapter::document::loader::load_document_mappings;
use conduit_core::adapter::sql::loader::load_sql_mappings;

#[derive(Parser)]
#[command(name = "conduit")]
#[command(about = "Event-first projection engine")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Output format for the execution report.
#[derive(Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
enum OutputFormat {
    /// Human-readable report (default)
    #[default]
    Text,
    /// JSON execution report
    Json,
}

#[derive(Subcommand)]
enum Commands {
    /// Execute a single event (writes to adapters)
    Run {
        /// Conduit configuration file (v1 schema)
        #[arg(long)]
        config: PathBuf,

        /// Directory containing SQL + document mappings
        #[arg(long)]
        mappings: PathBuf,

        /// Event JSON file
        #[arg(long)]
        event: PathBuf,

        /// Output format: text (default) or json
        #[arg(long, value_enum, default_value_t)]
        output: OutputFormat,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            config,
            mappings,
            event,
            output,
        } => {
            let exit_code = run_cmd(config, mappings, event, output)?;
            std::process::exit(exit_code);
        }
    }
}

fn run_cmd(
    config_path: PathBuf,
    mappings_dir: PathBuf,
    event_path: PathBuf,
    output: OutputFormat,
) -> Result<i32, Box<dyn std::error::Error>> {
    // --------------------------------------------------
    // Load + validate config
    // --------------------------------------------------
    let config_file = std::fs::File::open(&config_path)?;
    let config: ConduitConfig = serde_yaml::from_reader(config_file)?;
    config.validate()?;

    // --------------------------------------------------
    // Load mappings
    // --------------------------------------------------
    let sql_mappings = load_sql_mappings(mappings_dir.join("sql"))?;
    let doc_mappings = load_document_mappings(mappings_dir.join("document"))?;

    // --------------------------------------------------
    // Load event
    // --------------------------------------------------
    let raw = fs::read_to_string(&event_path)?;
    let event: Event = serde_json::from_str(&raw)?;

    if output == OutputFormat::Text {
        println!("Event loaded: {}\n", event.event_type);
    }

    // --------------------------------------------------
    // Execute
    // --------------------------------------------------
    let report = execute_event(&config, sql_mappings, doc_mappings, event);

    // --------------------------------------------------
    // Output
    // --------------------------------------------------
    match output {
        OutputFormat::Text => print_text_report(&report),
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&report)?;
            println!("{}", json);
        }
    }

    // --------------------------------------------------
    // Exit Code
    // --------------------------------------------------
    let exit_code = match report.status {
        ExecutionStatus::Succeeded => 0,
        ExecutionStatus::Failed => 1,
    };

    Ok(exit_code)
}

fn print_text_report(report: &conduit_core::execution::ExecutionReport) {
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
                let msg = extract_error_message(r.error.as_ref());
                println!(
                    "~ {} ({}) — skipped: {}",
                    r.adapter_id,
                    format_storage_kind(r.storage_kind),
                    msg
                );
            }
            AdapterOutcome::WriteFailed => {
                let msg = extract_error_message(r.error.as_ref());
                println!(
                    "✗ {} ({}) — {}",
                    r.adapter_id,
                    format_storage_kind(r.storage_kind),
                    msg
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

fn extract_error_message(error: Option<&AdapterReportError>) -> &str {
    match error {
        Some(AdapterReportError::WriteFailed { message }) => message,
        Some(AdapterReportError::Skipped { message }) => message,
        None => "unknown",
    }
}
