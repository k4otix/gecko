//! # gecko-bin
//!
//! The Assembler executable for the GECKO framework.
//!
//! Parses CLI arguments, instantiates the core engine, wires extension structs
//! (cyber-gecko, mem-gecko), and dispatches to the appropriate handler.
//! This is the only crate that depends on both gecko-engine AND extensions (design §2).

mod config;

use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use gecko_engine::db::RouterGraphStore;
use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};
use gecko_engine::okf::parser::parse_bundle;
use gecko_engine::okf::types::OkfBundle;
use gecko_engine::sandbox::engine::HostImports;
use gecko_engine::sandbox::mem_host::epistemic_extension_callback;
use gecko_engine::syncer::bundle::sync_bundle;

use gecko_extension_api::{
    ActorId, ConceptId, Embedder, EpistemicWriter, GeckoExtension, GraphStore, ProvenanceSource,
    RunContext, SandboxCtx, SemanticIndex,
};
use gecko_semantic_index::{HnswIndex, StubEmbedder};

use crate::config::{GeckoConfig, SemanticIndexConfig, build_extensions};

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

    /// TypeDB database name. Defaults to the bundle's declared name (from
    /// bundle.json) for `init`/`sync`, otherwise "gecko". One database per bundle.
    #[arg(long, global = true)]
    database: Option<String>,

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

    /// Path to the runtime extension-selection config
    #[arg(long, default_value = "gecko.toml", global = true)]
    config: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize the TypeDB schema (core + extensions)
    Init {
        /// Optional bundle directory; its bundle.json name selects the database
        bundle: Option<PathBuf>,

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

    /// Delete a TypeDB database by name
    Drop {
        /// Name of the database to delete
        database: String,
    },

    /// List all databases on the server
    Databases,
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

/// Resolves the target database: an explicit `--database` wins; otherwise the
/// bundle's declared name (when there is a bundle context); otherwise "gecko".
fn resolve_db(explicit: &Option<String>, derived: Option<&str>) -> String {
    explicit
        .clone()
        .or_else(|| derived.map(str::to_string))
        .unwrap_or_else(|| "gecko".to_string())
}

/// Builds a `DbConfig` for the resolved database using the shared connection args.
fn make_config(cli: &Cli, database: String) -> DbConfig {
    let tls = if cli.tls {
        TlsMode::Enabled {
            ca_cert: cli.ca_cert.clone(),
        }
    } else {
        TlsMode::Disabled
    };
    DbConfig {
        address: cli.address.clone(),
        database,
        username: cli.username.clone(),
        password: cli.password.clone(),
        tls,
    }
}

async fn run(cli: Cli) -> Result<()> {
    // Assemble extensions (design §2: The Assembler Pattern). The runtime config
    // selects which registered extensions are activated; mem is forced-on and
    // registered first so schemas apply in subtyping order. Registration is the
    // gate — only these extensions get functions loaded and write-paths opened.
    let cfg = GeckoConfig::load(&cli.config)?;
    let extensions = build_extensions(&cfg)?;

    match &cli.command {
        Commands::Init { bundle, schema } => {
            let derived = match bundle {
                Some(path) => Some(
                    gecko_engine::okf::parser::bundle_name(path)
                        .with_context(|| format!("Failed to read bundle at {}", path.display()))?,
                ),
                None => None,
            };
            let config = make_config(&cli, resolve_db(&cli.database, derived.as_deref()));
            cmd_init(config, &extensions, schema.clone()).await
        }
        Commands::Sync { bundle_path } => {
            println!("Parsing bundle at {}...", bundle_path.display());
            let manifest = parse_bundle(bundle_path).context("Failed to parse OKF bundle")?;
            let config = make_config(&cli, resolve_db(&cli.database, Some(&manifest.bundle_name)));
            cmd_sync(config, &extensions, manifest).await
        }
        Commands::Query { query_str } => {
            cmd_query(
                make_config(&cli, resolve_db(&cli.database, None)),
                query_str,
            )
            .await
        }
        Commands::Status => cmd_status(make_config(&cli, resolve_db(&cli.database, None))).await,
        Commands::Run { concept_id } => {
            cmd_run(
                make_config(&cli, resolve_db(&cli.database, None)),
                &cfg,
                concept_id,
            )
            .await
        }
        Commands::Drop { database } => cmd_drop(make_config(&cli, database.clone())).await,
        Commands::Databases => {
            cmd_databases(make_config(&cli, resolve_db(&cli.database, None))).await
        }
    }
}

/// Delete a database by name.
async fn cmd_drop(config: DbConfig) -> Result<()> {
    let name = config.database.clone();
    let mut db = TypeDbRouter::new(config);
    db.delete_database(&name)
        .await
        .with_context(|| format!("Failed to delete database '{name}'"))?;
    println!("✓ Dropped database '{name}'");
    Ok(())
}

/// List all databases on the server.
async fn cmd_databases(config: DbConfig) -> Result<()> {
    let mut db = TypeDbRouter::new(config);
    let names = db
        .list_databases()
        .await
        .context("Failed to list databases")?;
    if names.is_empty() {
        println!("No databases.");
    } else {
        for n in names {
            println!("{n}");
        }
    }
    Ok(())
}

/// Applies the core schema (embedded or a custom file) plus every extension
/// schema, and runs each extension's init hook. Idempotent — TypeDB `define` is a
/// no-op for already-defined types, so this is safe to re-run on every sync.
async fn apply_all_schemas(
    db: &mut TypeDbRouter,
    extensions: &[Box<dyn GeckoExtension>],
    custom_schema: Option<PathBuf>,
) -> Result<()> {
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

    for ext in extensions {
        let schema = ext.schema();
        if !schema.trim().is_empty() {
            db.apply_schema(schema)
                .await
                .with_context(|| format!("Failed to apply {} schema", ext.name()))?;
            info!(extension = ext.name(), "Extension schema applied");
        }

        ext.on_init()
            .map_err(|e| anyhow::anyhow!("Extension {} init failed: {}", ext.name(), e))?;
    }
    Ok(())
}

/// Initialize TypeDB with core schema + extension schemas.
async fn cmd_init(
    config: DbConfig,
    extensions: &[Box<dyn GeckoExtension>],
    custom_schema: Option<PathBuf>,
) -> Result<()> {
    let database = config.database.clone();
    let mut db = TypeDbRouter::new(config);
    apply_all_schemas(&mut db, extensions, custom_schema).await?;
    println!(
        "✓ Schema initialized in '{}' (core + {} extensions)",
        database,
        extensions.len()
    );
    Ok(())
}

/// Sync a parsed OKF bundle. The bundle's database is ensured (schema applied,
/// idempotently) so the single-bundle-per-database flow works from one command.
async fn cmd_sync(
    config: DbConfig,
    extensions: &[Box<dyn GeckoExtension>],
    manifest: OkfBundle,
) -> Result<()> {
    println!(
        "Found {} concepts. Syncing bundle '{}' to database '{}'...",
        manifest.concepts.len(),
        manifest.bundle_name,
        config.database
    );

    let mut db = TypeDbRouter::new(config);
    apply_all_schemas(&mut db, extensions, None)
        .await
        .context("Failed to ensure database schema")?;

    let result = sync_bundle(&mut db, &manifest)
        .await
        .context("Failed to sync bundle")?;

    println!("✓ Sync complete!");
    println!("  Concepts inserted: {}", result.concepts_inserted);
    println!("  Concepts updated:  {}", result.concepts_updated);
    println!("  Concepts skipped:  {}", result.concepts_skipped);
    println!("  Concepts deleted:  {}", result.concepts_deleted);
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
                    println!("${col}: {concept}");
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
        println!("{answer:?}");
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
        .query(
            r#"match $b isa bundle, has bundle-name $name; fetch {"name": $name, "path": $b.bundle-path};"#,
        )
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
/// Builds mem's epistemic writer, wiring the A5 semantic-index accelerator when the
/// `[semantic_index]` config enables it.
///
/// - **Disabled** ⇒ `MemWriter::without_index` (recall falls back to the non-vector
///   path; no drag — the A4 shape).
/// - **Enabled** ⇒ concrete `HnswIndex` + stub `Embedder`, rebuilt from the graph
///   (A5.4) before serving recalls, with retrieval-provenance recording per config
///   (A5.7). The trait is the swap seam (A5.6): a future `typedb-native` backend
///   drops in here.
async fn build_epistemic_writer(
    store: std::sync::Arc<dyn GraphStore>,
    sic: &SemanticIndexConfig,
) -> Result<std::sync::Arc<dyn EpistemicWriter>> {
    if !sic.enabled {
        return Ok(std::sync::Arc::new(mem_gecko::MemWriter::without_index(
            store,
        )));
    }
    if sic.backend != "hnsw" {
        anyhow::bail!(
            "unsupported semantic_index.backend '{}': only 'hnsw' is compiled \
             (future drop-in: 'typedb-native')",
            sic.backend
        );
    }
    if sic.embedder != "stub" {
        anyhow::bail!(
            "unsupported semantic_index.embedder '{}': only 'stub' is compiled \
             (real bge-large-en-v1.5 is deferred behind a cargo feature)",
            sic.embedder
        );
    }

    let dim = 384usize;
    let embedder = std::sync::Arc::new(StubEmbedder::new(dim));
    let index = std::sync::Arc::new(HnswIndex::open(&sic.path, embedder.model_id(), dim)?);
    let embedder: std::sync::Arc<dyn Embedder> = embedder;
    let index: std::sync::Arc<dyn SemanticIndex> = index;

    let writer = mem_gecko::MemWriter::new(store, Some(embedder), Some(index))
        .with_retrieval_provenance(sic.record_provenance());
    // A5.4: reconstruct the accelerator from the graph (SoR) before serving recalls.
    writer.rebuild_index_from_graph().await?;
    info!(
        path = %sic.path,
        record_provenance = sic.record_provenance(),
        "Semantic index enabled (hnsw + stub embedder); rebuilt from graph"
    );
    Ok(std::sync::Arc::new(writer))
}

async fn cmd_run(config: DbConfig, cfg: &GeckoConfig, concept_id: &str) -> Result<()> {
    println!("Executing playbook: {concept_id}");

    // The epistemic writer needs its own connection to the same database: the
    // pipeline borrows `db` mutably for the run, while belief-tier writes commit
    // through this second router (belief writes are sparse — invariant 4 — so a
    // dedicated serialised connection is not a bottleneck).
    let writer_config = config.clone();
    let mut db = TypeDbRouter::new(config);
    let tx = db
        .begin_read()
        .await
        .context("Failed to begin read transaction")?;

    // Single-bundle-scoped: concept IDs are exact bundle-relative paths. Each
    // concept has exactly one executable program — its engine-matched code fences
    // were concatenated at parse time — so there is no ambiguity to resolve. We
    // fetch the concept's single code-block plus its engine and granted scopes
    // (both as lists to tolerate the 0-or-1 / 0-or-N cardinalities).
    let query = format!(
        r#"
        match $c isa concept, has concept-id "{}";
        fetch {{
            "engine": $c.engine,
            "code": [ $c.code-block ],
            "scopes": [ $c.scope ]
        }};
    "#,
        gecko_engine::syncer::bundle::escape_tql(concept_id)
    );

    let answer = tx
        .query(&query)
        .await
        .map_err(|e| anyhow::anyhow!("Query failed: {e}"))?;

    let mut doc_json: Option<serde_json::Value> = None;
    if answer.is_document_stream() {
        let mut stream = answer.into_documents();
        if let Some(Ok(doc)) = stream.next().await {
            doc_json = serde_json::from_str(&doc.into_json().to_string()).ok();
        }
    }

    let Some(json) = doc_json else {
        anyhow::bail!("Playbook '{concept_id}' not found.");
    };

    // `code` is a list projection: zero elements if the concept has no program.
    let code_block = json
        .get("code")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if code_block.trim().is_empty() {
        anyhow::bail!(
            "Playbook '{concept_id}' has no executable program \
             (no code fence matching its engine)."
        );
    }

    // The engine token is informational — all concepts run in the one WASM
    // (QuickJS) boundary now.
    let engine_type = json
        .get("engine")
        .and_then(|v| v.as_str())
        .unwrap_or("quickjs")
        .to_string();

    // Granted capability scopes gate host-extension calls in the sandbox (S3).
    let scopes: Vec<String> = json
        .get("scopes")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    println!("Resolved to: {concept_id}");

    // Rebuild the activated extension set from config for the host-call bridge.
    // Registration is the gate: only these extensions' imports are reachable from
    // the sandbox — a disabled extension exposes no write-path.
    let cb_extensions = build_extensions(cfg)?;

    println!(
        "Extensions loaded: {:?}",
        cb_extensions.iter().map(|e| e.name()).collect::<Vec<_>>()
    );
    println!("Running script using engine: {engine_type}...");

    // Activation (plan A4.2 + A5.5): construct mem's epistemic writer over its own
    // graph connection, then run each activated extension's
    // `inject_epistemic_host_fns` hook. mem (forced-on, first) binds the writer into
    // the sandbox context; other extensions inherit the no-op. When the
    // `[semantic_index]` accelerator is enabled we build the concrete `HnswIndex` +
    // stub `Embedder`, rebuild it from the graph (A5.4), and inject them; otherwise
    // the writer runs with no index (recall falls back, drag-free — the A4 shape).
    let graph_router =
        std::sync::Arc::new(tokio::sync::Mutex::new(TypeDbRouter::new(writer_config)));
    let store = std::sync::Arc::new(RouterGraphStore::new(graph_router));
    let writer: std::sync::Arc<dyn EpistemicWriter> =
        build_epistemic_writer(store, &cfg.semantic_index).await?;
    let mut sandbox_ctx = SandboxCtx::new();
    for ext in &cb_extensions {
        ext.inject_epistemic_host_fns(&mut sandbox_ctx, writer.clone());
    }

    // The base bridge dispatches plain host imports over the activated extensions.
    let base_cb: gecko_engine::sandbox::engine::ExtensionCallback =
        std::sync::Arc::new(move |ext_name, func_name, args| {
            for ext in &cb_extensions {
                if ext.name() == ext_name {
                    return ext.call_import(func_name, &args);
                }
            }
            Err(format!("Extension '{ext_name}' not found"))
        });

    // If an epistemic extension was activated, wrap the base bridge so mem host fns
    // (`remember`/`derive`/`supersede`/`contest`) reach the injected writer — each
    // stamped with a host-minted RunContext (invariant 2: the sandbox cannot forge
    // or omit `run_id`/`actor`/`source`). The host mints exactly one RunContext per
    // run, bound to the executing concept (`ExecutableDoc`).
    let ext_cb: gecko_engine::sandbox::engine::ExtensionCallback =
        match sandbox_ctx.epistemic_writer() {
            Some(writer) => {
                let run_ctx = RunContext::new(
                    ActorId::new("system"),
                    ProvenanceSource::ExecutableDoc {
                        concept_id: ConceptId::new(concept_id),
                    },
                    chrono::Utc::now(),
                );
                epistemic_extension_callback(run_ctx, writer, base_cb)
            }
            None => base_cb,
        };

    // Execute through the pipeline: it owns the state-handle lifecycle (RAII), the
    // shared sandbox pool, and the execution record — no execution logic is
    // duplicated here. `gecko run` sources the program from the graph, so build the
    // run descriptor from the fetched row. (timeout-ms is not persisted yet, so the
    // sandbox default applies; the pipeline honors an explicit timeout when given.)
    let state_registry = gecko_engine::state::registry::StateRegistry::new();
    let sandbox_pool = gecko_engine::sandbox::wasm_pool::SandboxPool::new();
    let host_imports = HostImports::default();

    let run = gecko_engine::pipeline::PlaybookRun {
        concept_id,
        program: &code_block,
        scopes: &scopes,
        timeout_ms: None,
    };

    let pipeline_result = gecko_engine::pipeline::execute_playbook(
        &run,
        &mut db,
        &state_registry,
        &sandbox_pool,
        &host_imports,
        Some(ext_cb),
    )
    .await
    .context("Pipeline execution failed")?;

    let result = pipeline_result.execution;
    if result.success {
        println!("Execution successful!");
        println!("Output: {}", result.output);
        println!("Duration: {} ms", result.duration_ms);
    } else {
        println!("Execution failed!");
        if let Some(err) = result.error {
            println!("Error: {err}");
        }
    }

    Ok(())
}
