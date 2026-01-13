use clap::{Parser, Subcommand};
use std::path::PathBuf;

use event_ingest::adapter::document::{
    loader::load_document_mappings, validate::validate_document_mappings,
};
use event_ingest::adapter::sql::{loader::load_sql_mappings, validate::validate_sql_mappings};
use event_ingest::routing::load_routing;

#[derive(Parser)]
#[command(name = "pipeline")]
#[command(about = "Event ingestion pipeline")]
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
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Validate { routing, mappings } => {
            validate_cmd(routing, mappings)?;
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
