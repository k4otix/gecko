//! # gecko-bin
//!
//! The Assembler executable for the GECKO framework.
//!
//! Parses CLI arguments, instantiates the core engine, wires extension structs
//! (cyber-gecko, mem-gecko), and dispatches to the appropriate handler.
//! This is the only crate that depends on both gecko-engine AND extensions (design §2).

use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};
use gecko_engine::extension::GeckoExtension;
use gecko_engine::okf::parser::parse_bundle;
use gecko_engine::syncer::bundle::sync_bundle;

use cyber_gecko::CyberGecko;
use mem_gecko::MemGecko;

#[derive(Parser)]
#[command(
    name = "gecko",
    about = "GECKO — Graph Execution of Contextual Knowledge Objects",
    version
)]
struct Cli {
    /// TypeDB server address
    #[arg(long, default_value = "localhost:1729", global = true)]
    address: String,

    /// TypeDB database name
    #[arg(long, default_value = "gecko", global = true)]
    database: String,

    /// TypeDB username
    #[arg(long, default_value = "admin", global = true)]
    username: String,

    /// TypeDB password
    #[arg(long, default_value = "password", global = true)]
    password: String,

    /// Enable TLS for TypeDB Cloud connections
    #[arg(long, global = true)]
    tls: bool,

    /// Path to CA certificate for TLS
    #[arg(long, global = true)]
    ca_cert: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize the TypeDB schema (core + extensions)
    Init {
        /// Path to custom core schema file (default: embedded)
        #[arg(long)]
        schema: Option<PathBuf>,
    },

    /// Parse and sync an OKF bundle to TypeDB
    Sync {
        /// Path to the OKF bundle directory
        bundle_path: PathBuf,
    },

    /// Run an ad-hoc TypeQL read query
    Query {
        /// The TypeQL query string
        query_str: String,
    },

    /// Show synced bundles and concept counts
    Status,

    /// Execute a playbook concept in the sandbox
    Run {
        /// Concept ID of the playbook to execute
        concept_id: String,
    },
}

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    if let Err(e) = run(cli).await {
        error!("{:#}", e);
        process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    let tls = if cli.tls {
        TlsMode::Enabled {
            ca_cert: cli.ca_cert,
        }
    } else {
        TlsMode::Disabled
    };

    let config = DbConfig {
        address: cli.address,
        database: cli.database,
        username: cli.username,
        password: cli.password,
        tls,
    };

    // Assemble extensions (design §2: The Assembler Pattern)
    let extensions: Vec<Box<dyn GeckoExtension>> =
        vec![Box::new(CyberGecko::new()), Box::new(MemGecko::new())];

    match cli.command {
        Commands::Init { schema } => cmd_init(config, &extensions, schema).await,
        Commands::Sync { bundle_path } => cmd_sync(config, bundle_path).await,
        Commands::Query { query_str } => cmd_query(config, &query_str).await,
        Commands::Status => cmd_status(config).await,
        Commands::Run { concept_id } => cmd_run(config, &extensions, &concept_id).await,
    }
}

/// Initialize TypeDB with core schema + extension schemas.
async fn cmd_init(
    config: DbConfig,
    extensions: &[Box<dyn GeckoExtension>],
    custom_schema: Option<PathBuf>,
) -> Result<()> {
    let mut db = TypeDbRouter::new(config);

    // Apply core schema
    let core_schema = if let Some(path) = custom_schema {
        std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read schema file: {}", path.display()))?
    } else {
        include_str!("../../core/gecko-engine/schema/core_schema.tql").to_string()
    };

    db.apply_schema(&core_schema)
        .await
        .context("Failed to apply core schema")?;
    info!("Core schema applied");

    // Apply extension schemas
    for ext in extensions {
        let schema = ext.schema();
        if !schema.trim().is_empty() {
            db.apply_schema(schema)
                .await
                .with_context(|| format!("Failed to apply {} schema", ext.name()))?;
            info!(extension = ext.name(), "Extension schema applied");
        }

        // Run extension init hook
        ext.on_init()
            .map_err(|e| anyhow::anyhow!("Extension {} init failed: {}", ext.name(), e))?;
    }

    println!(
        "✓ Schema initialized (core + {} extensions)",
        extensions.len()
    );
    Ok(())
}

/// Parse and sync an OKF bundle.
async fn cmd_sync(config: DbConfig, bundle_path: PathBuf) -> Result<()> {
    println!("Parsing bundle at {}...", bundle_path.display());

    let manifest = parse_bundle(&bundle_path).with_context(|| "Failed to parse OKF bundle")?;

    println!(
        "Found {} concepts. Syncing to TypeDB...",
        manifest.concepts.len()
    );

    let mut db = TypeDbRouter::new(config);
    let result = sync_bundle(&mut db, &manifest)
        .await
        .context("Failed to sync bundle")?;

    println!("✓ Sync complete!");
    println!("  Concepts inserted: {}", result.concepts_inserted);
    println!("  Concepts updated:  {}", result.concepts_updated);
    println!("  Concepts skipped:  {}", result.concepts_skipped);
    println!("  Links created:     {}", result.links_created);
    println!("  Citations created: {}", result.citations_created);

    Ok(())
}

/// Run an ad-hoc TypeQL query.
async fn cmd_query(config: DbConfig, query_str: &str) -> Result<()> {
    let mut db = TypeDbRouter::new(config);
    let tx = db
        .begin_read()
        .await
        .context("Failed to begin read transaction")?;

    let answer = tx
        .query(query_str)
        .await
        .map_err(|e| gecko_engine::db::router::DbError::Query(e.to_string()))?;

    if answer.is_row_stream() {
        let mut stream = answer.into_rows();
        while let Some(Ok(row)) = stream.next().await {
            for col in row.get_column_names() {
                if let Ok(Some(concept)) = row.get(col) {
                    println!("${}: {}", col, concept);
                }
            }
            println!("---");
        }
    } else if answer.is_document_stream() {
        let mut stream = answer.into_documents();
        while let Some(Ok(doc)) = stream.next().await {
            println!("{}", doc.into_json());
        }
    } else {
        println!("{:?}", answer);
    }

    Ok(())
}

/// Show synced bundles and concept counts.
async fn cmd_status(config: DbConfig) -> Result<()> {
    let mut db = TypeDbRouter::new(config);
    let tx = db
        .begin_read()
        .await
        .context("Failed to begin read transaction")?;

    let answer = tx
        .query(r#"match $b isa bundle, has bundle_path $path; fetch {"path": $path};"#)
        .await
        .map_err(|e| gecko_engine::db::router::DbError::Query(e.to_string()))?;

    let mut found = false;
    if answer.is_document_stream() {
        let mut stream = answer.into_documents();
        while let Some(Ok(doc)) = stream.next().await {
            found = true;
            println!("{}", doc.into_json());
        }
    }

    if !found {
        println!("No bundles synced yet.");
    }
    Ok(())
}

/// Execute a playbook concept in the sandbox.
async fn cmd_run(
    config: DbConfig,
    extensions: &[Box<dyn GeckoExtension>],
    concept_id: &str,
) -> Result<()> {
    println!("Executing playbook: {}", concept_id);

    let mut db = TypeDbRouter::new(config);
    let tx = db
        .begin_read()
        .await
        .context("Failed to begin read transaction")?;

    let query = if concept_id.contains(':') {
        format!(
            r#"
            match 
                $c isa concept, 
                    has concept-id "{}",
                    has concept-id $id,
                    has code-block $cb;
            fetch {{"id": $id, "code": $cb, "engine": $c.engine}};
        "#,
            gecko_engine::syncer::bundle::escape_tql(concept_id)
        )
    } else {
        format!(
            r#"
            match 
                $c isa concept, 
                    has concept-id $id,
                    has code-block $cb;
                $id like ".*:{}";
            fetch {{"id": $id, "code": $cb, "engine": $c.engine}};
        "#,
            gecko_engine::syncer::bundle::escape_tql(concept_id)
        )
    };

    let answer = tx
        .query(&query)
        .await
        .map_err(|e| anyhow::anyhow!("Query failed: {}", e))?;

    let mut matches = Vec::new();

    if answer.is_document_stream() {
        let mut stream = answer.into_documents();
        while let Some(Ok(doc)) = stream.next().await {
            let json_str = doc.into_json().to_string();
            let json: serde_json::Value = serde_json::from_str(&json_str).unwrap_or_default();

            let id = json
                .as_object()
                .and_then(|m| m.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let code = json
                .as_object()
                .and_then(|m| m.get("code"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let engine = json
                .as_object()
                .and_then(|m| m.get("engine"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            matches.push((id, code, engine));
        }
    }

    if matches.is_empty() {
        anyhow::bail!("Playbook '{}' not found or has no code blocks.", concept_id);
    } else if matches.len() > 1 {
        let found_ids: Vec<String> = matches.into_iter().map(|(id, _, _)| id).collect();
        anyhow::bail!(
            "Ambiguous playbook ID '{}'. Please specify the namespace. Found:\n  - {}",
            concept_id,
            found_ids.join("\n  - ")
        );
    }

    let (resolved_id, code_block, engine_type) = matches.pop().unwrap();
    println!("Resolved to: {}", resolved_id);

    use gecko_engine::sandbox::engine::{HostImports, ScriptExecutor};
    use gecko_engine::sandbox::rhai_executor::RhaiExecutor;
    use gecko_engine::sandbox::wasm_executor::WasmExecutor;
    use uuid::Uuid;

    println!(
        "Extensions loaded: {:?}",
        extensions.iter().map(|e| e.name()).collect::<Vec<_>>()
    );
    println!(
        "Running script using engine: {}...",
        if engine_type.is_empty() {
            "rhai"
        } else {
            &engine_type
        }
    );

    let executor: Box<dyn ScriptExecutor> = match engine_type.as_str() {
        "quickjs" | "wasm" => {
            Box::new(WasmExecutor::new().context("Failed to initialize Wasm engine")?)
        }
        _ => Box::new(RhaiExecutor::new()),
    };

    let cb_extensions: Vec<Box<dyn GeckoExtension>> = extensions
        .iter()
        .map(|e| -> Result<Box<dyn GeckoExtension>> {
            // Re-instantiate the extension based on name, because we can't easily clone Box<dyn GeckoExtension>
            match e.name() {
                "cyber-gecko" => {
                    Ok(Box::new(cyber_gecko::CyberGecko::new()) as Box<dyn GeckoExtension>)
                }
                "mem-gecko" => Ok(Box::new(mem_gecko::MemGecko::new()) as Box<dyn GeckoExtension>),
                _ => anyhow::bail!("Unknown extension: {}", e.name()),
            }
        })
        .collect::<Result<Vec<_>>>()?;

    let ext_cb: gecko_engine::sandbox::engine::ExtensionCallback =
        std::sync::Arc::new(move |ext_name, func_name, args| {
            for ext in &cb_extensions {
                if ext.name() == ext_name {
                    return ext.call_import(func_name, &args);
                }
            }
            Err(format!("Extension '{}' not found", ext_name))
        });

    let result = tokio::task::block_in_place(|| {
        executor.evaluate(
            &code_block,
            Uuid::new_v4(),
            &HostImports::default(),
            None,
            Some(ext_cb),
        )
    });

    if result.success {
        println!("Execution successful!");
        println!("Output: {}", result.output);
        println!("Duration: {} ms", result.duration_ms);
    } else {
        println!("Execution failed!");
        if let Some(err) = result.error {
            println!("Error: {}", err);
        }
    }

    Ok(())
}
