use clap::{Parser, Subcommand};
use conduit_core::event::Event;
use conduit_core::execute_event;
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
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            config,
            mappings,
            event,
        } => {
            let exit_code = run_cmd(config, mappings, event)?;
            std::process::exit(exit_code);
        }
    }
}

fn run_cmd(
    config_path: PathBuf,
    mappings_dir: PathBuf,
    event_path: PathBuf,
) -> Result<i32, Box<dyn std::error::Error>> {
    // --------------------------------------------------
    // Load + validate config (Phase 5)
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

    println!("Event loaded: {}\n", event.event_type);

    // --------------------------------------------------
    // Execute via public API
    // --------------------------------------------------
    let results = execute_event(&config, sql_mappings, doc_mappings, event);

    // --------------------------------------------------
    // Report
    // --------------------------------------------------
    println!("--- Execution Report ---");
    for r in &results {
        if r.success {
            println!("✓ {} ({:?})", r.adapter_id, r.kind);
        } else {
            println!("✗ {} ({:?}) - {:?}", r.adapter_id, r.kind, r.error);
        }
    }
    println!("------------------------");

    let exit_code = if results.iter().all(|r| r.success) {
        0
    } else {
        2
    };

    Ok(exit_code)
}
