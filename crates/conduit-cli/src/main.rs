use clap::{Parser, Subcommand};
use conduit_core::adapter::document::preview::DocumentPreviewBuilder;
use conduit_core::adapter::sql::preview::SqlPreviewBuilder;
use conduit_core::dispatch::dispatch;
use conduit_core::event::Event;
use conduit_core::runtime::build_adapters;
use std::fs;
use std::path::PathBuf;

use conduit_core::adapter::document::{
    loader::load_document_mappings, validate::validate_document_mappings,
};
use conduit_core::adapter::sql::{loader::load_sql_mappings, validate::validate_sql_mappings};
use conduit_core::routing::{load_routing, route, StorageKind};

#[derive(Parser)]
#[command(name = "conduit")]
#[command(about = "Event-first projection engine")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Validate routing and mappings
    Validate {
        #[arg(long)]
        routing: PathBuf,
        #[arg(long)]
        mappings: PathBuf,
    },

    /// Test a single event against mappings (no writes)
    TestEvent {
        #[arg(long)]
        routing: PathBuf,
        #[arg(long)]
        mappings: PathBuf,
        #[arg(long)]
        event: PathBuf,
    },

    /// Execute a single event (writes to adapters)
    Run {
        #[arg(long)]
        routing: PathBuf,
        #[arg(long)]
        mappings: PathBuf,
        #[arg(long)]
        event: PathBuf,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Validate { routing, mappings } => {
            validate_cmd(routing, mappings)?;
        }
        Commands::TestEvent {
            routing,
            mappings,
            event,
        } => {
            test_event_cmd(routing, mappings, event)?;
        }
        Commands::Run {
            routing,
            mappings,
            event,
        } => {
            let exit_code = run_cmd(routing, mappings, event)?;
            std::process::exit(exit_code);
        }
    }

    Ok(())
}

fn validate_cmd(
    routing_path: PathBuf,
    mappings_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Loading routing from {:?}", routing_path);
    let routing = load_routing(&routing_path)?;

    let sql_dir = mappings_dir.join("sql");
    let doc_dir = mappings_dir.join("document");

    println!("Loading SQL mappings from {:?}", sql_dir);
    let sql_mappings = load_sql_mappings(&sql_dir)?;

    println!("Loading Document mappings from {:?}", doc_dir);
    let doc_mappings = load_document_mappings(&doc_dir)?;

    println!("Validating SQL mappings...");
    validate_sql_mappings(&routing, &sql_mappings)?;

    println!("Validating Document mappings...");
    validate_document_mappings(&routing, &doc_mappings)?;

    println!("Validation successful ✔");
    Ok(())
}

fn test_event_cmd(
    routing_path: PathBuf,
    mappings_dir: PathBuf,
    event_path: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    // Load routing
    let routing = load_routing(&routing_path)?;

    // Load mappings
    let sql_mappings = load_sql_mappings(mappings_dir.join("sql"))?;
    let doc_mappings = load_document_mappings(mappings_dir.join("document"))?;

    let sql_builder = SqlPreviewBuilder::new(&sql_mappings);
    let doc_builder = DocumentPreviewBuilder::new(&doc_mappings);

    // Validate configs
    validate_sql_mappings(&routing, &sql_mappings)?;
    validate_document_mappings(&routing, &doc_mappings)?;

    // Load event
    let raw = fs::read_to_string(&event_path)?;
    let event: Event = serde_json::from_str(&raw)?;

    println!("Event: {}\n", event.event_type);

    // Routing decision
    let targets = route(&event);

    println!("Routing:");
    for t in &targets {
        println!("  → {:?}", t);
    }
    println!();

    // SQL projection
    if targets.contains(&StorageKind::Sql) {
        let (sql, values) = sql_builder.preview(&event)?;

        println!("SQL Projection:");
        println!("  {}", sql);
        println!("  Values: {:?}", values);
        println!();
    }

    // Document projection
    if targets.contains(&StorageKind::Document) {
        let mapping = doc_mappings
            .get(&event.event_type)
            .ok_or("No document mapping for event")?;

        let document = doc_builder.preview(&event)?;

        println!("Document Projection (collection={}):", mapping.collection);
        println!("{}", serde_json::to_string_pretty(&document)?);
        println!();
    }

    println!("Status: OK");
    Ok(())
}

fn run_cmd(
    routing_path: PathBuf,
    mappings_dir: PathBuf,
    event_path: PathBuf,
) -> Result<i32, Box<dyn std::error::Error>> {
    println!("Loading routing from {:?}", routing_path);
    let routing = load_routing(&routing_path)?;

    // Load mappings
    let sql_mappings = load_sql_mappings(mappings_dir.join("sql"))?;
    let doc_mappings = load_document_mappings(mappings_dir.join("document"))?;

    println!("Validating configurations...");
    validate_sql_mappings(&routing, &sql_mappings)?;
    validate_document_mappings(&routing, &doc_mappings)?;

    // Load event
    println!("Loading event from {:?}", event_path);
    let raw = fs::read_to_string(&event_path)?;
    let event: Event = serde_json::from_str(&raw)?;

    println!("Event loaded: {}\n", event.event_type);

    // Build adapters via factory
    let mut adapters = build_adapters(sql_mappings, doc_mappings);

    let results = dispatch(&event, &mut adapters);

    // Step 6: Wrap results and print report
    println!("--- Execution Report ---");
    for r in &results {
        if r.success {
            println!("✓ {} ({:?})", r.adapter_id, r.kind);
        } else {
            println!("✗ {} ({:?}) - {:?}", r.adapter_id, r.kind, r.error);
        }
    }
    println!("------------------------");

    // Step 7: Determine exit code
    let exit_code = if results.iter().all(|r| r.success) {
        0
    } else {
        2
    };

    Ok(exit_code)
}
