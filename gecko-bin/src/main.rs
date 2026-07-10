//! # gecko-bin
//!
//! The Assembler executable for the GECKO framework.
//!
//! Parses CLI arguments, instantiates the core engine, wires extension structs
//! (cyber-gecko, mem-gecko), and dispatches to the appropriate handler.
//! This is the only crate that depends on both gecko-engine AND extensions (design §2).

mod config;
mod doctor;
mod orchestrator;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

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
#[cfg(feature = "real-embedder")]
use gecko_semantic_index::CandleEmbedder;
use gecko_semantic_index::{HnswIndex, StubEmbedder};

use crate::config::{GeckoConfig, TypedbMode, build_extensions};

#[derive(Parser)]
#[command(
    name = "gecko",
    about = "GECKO — Graph Execution of Contextual Knowledge Objects",
    version
)]
struct Cli {
    /// TypeDB server address. When unset, `[typedb] endpoint` in gecko.toml is
    /// used (default `localhost:1729`); passing `--address` overrides the config.
    #[arg(long, global = true)]
    address: Option<String>,

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
    /// Write a default gecko.toml if absent (idempotent; never clobbers an
    /// existing config).
    Init,

    /// Bring up TypeDB for the configured run mode. In `orchestrated` mode this
    /// ensures the pinned TypeDB is present, spawns it as a managed child, and
    /// waits for readiness (idempotent — a no-op if already running). In
    /// `compose`/`external` mode it prints the path to take (gecko manages
    /// nothing).
    Up,

    /// Tear down the managed TypeDB child started by `gecko up` (orchestrated
    /// mode only). A no-op if nothing is running; other modes print guidance.
    Down,

    /// Initialize the TypeDB schema (core + extensions)
    SchemaInit {
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

    #[cfg(feature = "cyber")]
    /// Parse and sync a STIX 2.1 bundle to TypeDB
    SyncStix {
        /// Path to the STIX bundle JSON file
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

    /// Manage the embedding model runtime asset (fetch/stage).
    Model {
        #[command(subcommand)]
        command: ModelCommands,
    },

    /// Report every precondition (config, cache, TypeDB, model, index, embedder)
    /// as ✓/⚠/✗ with a remediation for anything not-OK. Exits non-zero iff a hard
    /// precondition (a `✗`) fails — warnings do not fail it. Runs even with a
    /// broken/absent gecko.toml (that is reported as the `config` failure).
    Doctor {
        /// Emit the results as JSON (for machine consumption).
        #[arg(long)]
        json: bool,

        /// Suppress all output; only the exit code is meaningful (used by setup.sh).
        #[arg(long)]
        quiet: bool,

        /// Run only a single named check (clap validates against the known set).
        #[arg(long)]
        check: Option<doctor::DoctorCheck>,
    },
}

#[derive(Subcommand)]
enum ModelCommands {
    /// Download (stage) the embedding model into the runtime cache. Compiled in
    /// every build (it needs no candle). Idempotent: a second run is a cache-hit
    /// no-op. Never a silent pull elsewhere — this is the explicit opt-in.
    Fetch {
        /// Model id `"<model>@<revision>"` to fetch (overrides `[semantic_index]
        /// model_id`).
        #[arg(long)]
        model: Option<String>,

        /// Destination directory (overrides the derived
        /// `cache_dir/models/<model_id>/` and `[semantic_index] model_path`).
        #[arg(long)]
        path: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    // Initialize tracing. Parse the CLI first so `gecko doctor --quiet` can pin the
    // default log floor to `error`: the TypeDB driver/router emit INFO on the
    // doctor typedb check, and `--quiet` promises only the exit code is meaningful
    // (setup.sh depends on it) — so their INFO must not leak to stderr. An explicit
    // RUST_LOG still wins for anyone who wants the chatter back.
    let default_filter = match &cli.command {
        Commands::Doctor { quiet: true, .. } => "error",
        _ => "info",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter)),
        )
        .init();

    // `gecko doctor` owns its own exit code (FAILURE iff a hard precondition fails)
    // and must run even with a broken/absent gecko.toml — so it short-circuits
    // BEFORE run()'s strict config load + extension assembly.
    if let Commands::Doctor { json, quiet, check } = &cli.command {
        return doctor::run_doctor(&cli, *check, *json, *quiet).await;
    }

    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!("{:#}", e);
            ExitCode::FAILURE
        }
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

/// Maps the `--tls` flag (and optional `--ca-cert`) to a [`TlsMode`]. Shared by
/// `make_config`, `gecko doctor`'s typedb check, and the orchestrator's readiness
/// probe so all three connect with the same TLS posture.
pub(crate) fn tls_mode(tls: bool, ca_cert: Option<&Path>) -> TlsMode {
    if tls {
        TlsMode::Enabled {
            ca_cert: ca_cert.map(Path::to_path_buf),
        }
    } else {
        TlsMode::Disabled
    }
}

/// Builds a `DbConfig` for the resolved database using the shared connection args.
/// The address is resolved by `resolve_address` (CLI `--address` overrides the
/// `[typedb] endpoint` config).
fn make_config(cli: &Cli, address: &str, database: String) -> DbConfig {
    DbConfig {
        address: address.to_string(),
        database,
        username: cli.username.clone(),
        password: cli.password.clone(),
        tls: tls_mode(cli.tls, cli.ca_cert.as_deref()),
    }
}

/// Resolves the effective TypeDB address: the CLI `--address` flag wins;
/// otherwise the `[typedb] endpoint` from config (default `localhost:1729`).
fn resolve_address(cli: &Cli, cfg: &GeckoConfig) -> String {
    cli.address
        .clone()
        .unwrap_or_else(|| cfg.typedb.endpoint.clone())
}

async fn run(cli: Cli) -> Result<()> {
    // `gecko init` writes config and must run WITHOUT assembling extensions or
    // touching TypeDB — it exists to bring a gecko.toml into being.
    // Handle this BEFORE loading config so we don't warn about a missing gecko.toml
    // that this command is about to create.
    if let Commands::Init = &cli.command {
        return cmd_init_config(&cli.config);
    }

    // Load the runtime config (design §2: The Assembler Pattern). The config
    // selects which registered extensions are activated; mem is forced-on and
    // registered first so schemas apply in subtyping order. Registration is the
    // gate — only these extensions get functions loaded and write-paths opened.
    //
    // Extensions are assembled per-command, only in the arms that use them
    // (SchemaInit / Sync / Run), so a typo in `[extensions] enabled` can only fail
    // those commands — never server management or read-only ones (up/down/query/
    // status/drop/databases/model).
    let cfg = GeckoConfig::load(&cli.config)?;
    let address = resolve_address(&cli, &cfg);

    // `gecko up`/`down` manage (or point at) the server itself — they route on the
    // run mode and never assemble a DB connection like the query commands below.
    match &cli.command {
        Commands::Up => return cmd_up(&cli, &cfg, &address).await,
        Commands::Down => return cmd_down(&cfg, &address).await,
        _ => {}
    }

    let result = match &cli.command {
        Commands::Init | Commands::Up | Commands::Down => unreachable!("handled above"),
        Commands::Doctor { .. } => unreachable!("handled in main before run()"),
        Commands::SchemaInit { bundle, schema } => {
            let extensions = build_extensions(&cfg)?;
            let derived = match bundle {
                Some(path) => Some(
                    gecko_engine::okf::parser::bundle_name(path)
                        .with_context(|| format!("Failed to read bundle at {}", path.display()))?,
                ),
                None => None,
            };
            let config = make_config(
                &cli,
                &address,
                resolve_db(&cli.database, derived.as_deref()),
            );
            cmd_schema_init(config, &extensions, schema.clone()).await
        }
        Commands::Sync { bundle_path } => {
            let extensions = build_extensions(&cfg)?;
            println!("Parsing bundle at {}...", bundle_path.display());
            let manifest = parse_bundle(bundle_path).context("Failed to parse OKF bundle")?;
            let config = make_config(
                &cli,
                &address,
                resolve_db(&cli.database, Some(&manifest.bundle_name)),
            );
            cmd_sync(config, &extensions, manifest).await
        }
        #[cfg(feature = "cyber")]
        Commands::SyncStix { bundle_path } => {
            let extensions = build_extensions(&cfg)?;
            println!("Parsing STIX bundle at {}...", bundle_path.display());
            let bundle_json =
                std::fs::read_to_string(bundle_path).context("Failed to read STIX JSON")?;
            let (manifest, typed_rels) = cyber_gecko::stix::to_okf(&bundle_json)
                .map_err(|e| anyhow::anyhow!("STIX parse error: {}", e))?;
            let config = make_config(
                &cli,
                &address,
                resolve_db(&cli.database, Some(&manifest.bundle_name)),
            );

            println!(
                "Found {} STIX concepts. Syncing bundle '{}' to database '{}'...",
                manifest.concepts.len(),
                manifest.bundle_name,
                config.database
            );

            let mut db = TypeDbRouter::new(config);
            apply_all_schemas(&mut db, &extensions, None)
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

            println!("Running cyber_post_sync for typed relations...");
            let tx = db
                .begin_write()
                .await
                .context("Failed to begin write transaction")?;
            cyber_gecko::stix::cyber_post_sync(&tx, &typed_rels)
                .await
                .map_err(|e| anyhow::anyhow!("Post sync failed: {}", e))?;
            tx.commit().await.context("Failed to commit post sync")?;

            Ok(())
        }
        Commands::Query { query_str } => {
            cmd_query(
                make_config(&cli, &address, resolve_db(&cli.database, None)),
                query_str,
            )
            .await
        }
        Commands::Status => {
            cmd_status(make_config(&cli, &address, resolve_db(&cli.database, None))).await
        }
        Commands::Run { concept_id } => {
            let extensions = build_extensions(&cfg)?;
            cmd_run(
                make_config(&cli, &address, resolve_db(&cli.database, None)),
                &cfg,
                concept_id,
                extensions,
            )
            .await
        }
        Commands::Drop { database } => {
            cmd_drop(make_config(&cli, &address, database.clone())).await
        }
        Commands::Databases => {
            cmd_databases(make_config(&cli, &address, resolve_db(&cli.database, None))).await
        }
        Commands::Model { command } => match command {
            ModelCommands::Fetch { model, path } => {
                cmd_model_fetch(&cfg, model.clone(), path.clone())
            }
        },
    };

    // In orchestrated mode, a connection failure almost always means the managed
    // server was never started — `gecko *` does NOT auto-spawn TypeDB (that is
    // `gecko up`'s job). Turn the raw driver error into an actionable hint.
    result.map_err(|e| {
        if cfg.typedb.mode == TypedbMode::Orchestrated && looks_like_connection_failure(&e) {
            e.context(format!(
                "could not reach the orchestrated TypeDB at '{address}' — run `gecko up` first \
                 to start it (or set `[typedb] mode` to external/compose)"
            ))
        } else {
            e
        }
    })
}

/// Heuristic: does this error chain look like a failure to reach the server (as
/// opposed to a query/schema error)? Used only to append the `gecko up` hint.
fn looks_like_connection_failure(e: &anyhow::Error) -> bool {
    let msg = format!("{e:#}").to_lowercase();
    msg.contains("connection")
        || msg.contains("connect")
        || msg.contains("unable to connect")
        || msg.contains("transport")
        || msg.contains("refused")
}

/// `gecko up` — bring up TypeDB for the configured run mode.
async fn cmd_up(cli: &Cli, cfg: &GeckoConfig, address: &str) -> Result<()> {
    match cfg.typedb.mode {
        TypedbMode::Orchestrated => {
            let cache_root = config::cache_dir(cfg)?;
            println!(
                "Bringing up orchestrated TypeDB {} (endpoint {address})...",
                orchestrator::PINNED_TYPEDB
            );
            let tls = tls_mode(cli.tls, cli.ca_cert.as_deref());
            match orchestrator::up(&cache_root, address, &cli.username, &cli.password, tls).await? {
                orchestrator::UpOutcome::AlreadyRunning => {
                    println!("✓ TypeDB already running at {address} — nothing to do.");
                }
                orchestrator::UpOutcome::Started { pid } => {
                    println!("✓ Started managed TypeDB (pid {pid}) at {address}, ready.");
                }
            }
        }
        TypedbMode::Compose => {
            println!(
                "[typedb] mode = compose: gecko does not manage the Docker stack.\n\
                 Run:  docker compose up -d\n\
                 (TypeDB will be available at {address}.)"
            );
        }
        TypedbMode::External => {
            println!(
                "[typedb] mode = external: gecko connects to a TypeDB you run yourself.\n\
                 Start your server and point `[typedb] endpoint` (currently {address}) at it;\n\
                 gecko will not spawn or manage a process."
            );
        }
    }
    Ok(())
}

/// `gecko down` — tear down TypeDB for the configured run mode.
async fn cmd_down(cfg: &GeckoConfig, address: &str) -> Result<()> {
    match cfg.typedb.mode {
        TypedbMode::Orchestrated => {
            let cache_root = config::cache_dir(cfg)?;
            // `down` blocks on SIGTERM → wait → SIGKILL (a `std::thread::sleep`
            // poll loop); run it off the async runtime so it never stalls the
            // reactor thread.
            let outcome = tokio::task::spawn_blocking(move || orchestrator::down(&cache_root))
                .await
                .context("TypeDB shutdown task panicked")??;
            match outcome {
                orchestrator::DownOutcome::NotRunning => {
                    println!("No managed TypeDB is running — nothing to stop.");
                }
                orchestrator::DownOutcome::Stopped { pid, graceful } => {
                    if graceful {
                        println!("✓ Stopped managed TypeDB (pid {pid}).");
                    } else {
                        println!("✓ Stopped managed TypeDB (pid {pid}) — needed SIGKILL.");
                    }
                }
            }
        }
        TypedbMode::Compose => {
            println!(
                "[typedb] mode = compose: gecko does not manage the Docker stack.\n\
                 Run:  docker compose down"
            );
        }
        TypedbMode::External => {
            println!(
                "[typedb] mode = external: gecko never started a server, so there is nothing \
                 to stop (the server at {address} is yours to manage)."
            );
        }
    }
    Ok(())
}

/// `gecko model fetch` — stage the embedding model into the runtime cache.
///
/// Resolves the target `model_id` (flag > `[semantic_index] model_id`) and the
/// destination dir (`--path` > `[semantic_index] model_path` > derived
/// `cache_dir/models/<model_id>/`), then downloads the required files from the
/// configured source (mirror > HuggingFace) with progress + checksum logging.
/// Idempotent: a second run is a cache-hit no-op. Load-bearing rule #1: bytes only
/// ever land in the cache dir, OUTSIDE the build tree.
fn cmd_model_fetch(
    cfg: &GeckoConfig,
    model_flag: Option<String>,
    path_flag: Option<PathBuf>,
) -> Result<()> {
    use gecko_semantic_index::fetch::{self, FetchOutcome};

    let model_id = model_flag.unwrap_or_else(|| cfg.semantic_index.model_id.clone());
    // Guard against fetching the unresolved placeholder — it can't map to a real
    // upstream revision.
    if model_id.contains("<revision>") {
        anyhow::bail!(
            "model_id '{model_id}' has an unresolved '<revision>' placeholder; pin a real \
             revision in gecko.toml ([semantic_index] model_id) or pass --model \
             '<model>@<revision-hash>'"
        );
    }

    // Destination precedence: --path > config model_path > derived under cache root.
    let override_dir = path_flag
        .map(|p| p.to_string_lossy().into_owned())
        .or_else(|| cfg.semantic_index.model_path.clone());
    let cache_root = config::cache_dir(cfg)?;
    let dest = fetch::model_dir(&cache_root, &model_id, override_dir.as_deref());

    println!("Fetching model '{model_id}' → {}", dest.display());
    if let Some(src) = cfg.semantic_index.model_source.as_deref() {
        println!("Source (mirror): {src}");
    } else {
        println!("Source: HuggingFace Hub ({})", fetch::DEFAULT_HF_BASE);
    }

    let outcome = fetch::fetch_model(
        &model_id,
        &dest,
        cfg.semantic_index.model_source.as_deref(),
        false,
        fetch::known_checksums(&model_id),
    )
    .context("model fetch failed")?;

    match outcome {
        FetchOutcome::CacheHit => {
            println!("✓ Already staged (cache hit) — nothing to do.");
        }
        FetchOutcome::Downloaded { files } => {
            println!("✓ Staged {} files to {}", files.len(), dest.display());
        }
    }
    Ok(())
}

/// `gecko init` — write a default `gecko.toml` at `path` if absent. Idempotent:
/// an existing file is NEVER clobbered.
fn cmd_init_config(path: &std::path::Path) -> Result<()> {
    if path.exists() {
        println!("✓ {} already exists — leaving it untouched", path.display());
        return Ok(());
    }
    std::fs::write(path, config::default_gecko_toml())
        .with_context(|| format!("Failed to write {}", path.display()))?;
    println!("✓ Wrote default config to {}", path.display());
    Ok(())
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
        tokio::fs::read_to_string(&path)
            .await
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
async fn cmd_schema_init(
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
    println!("  Links attempted:    {}", result.links_attempted);
    println!("  Citations attempted: {}", result.citations_attempted);

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
        // Propagate a stream error instead of silently ending the loop on the
        // first `Err` (which would truncate results and hide a real DB fault).
        while let Some(item) = stream.next().await {
            let row = item.context("error reading query row stream")?;
            for col in row.get_column_names() {
                if let Ok(Some(concept)) = row.get(col) {
                    println!("${col}: {concept}");
                }
            }
            println!("---");
        }
    } else if answer.is_document_stream() {
        let mut stream = answer.into_documents();
        while let Some(item) = stream.next().await {
            let doc = item.context("error reading query document stream")?;
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
        while let Some(item) = stream.next().await {
            let doc = item.context("error reading bundle document stream")?;
            found = true;
            println!("{}", doc.into_json());
        }
    }

    if !found {
        println!("No bundles synced yet.");
    }
    Ok(())
}

/// Builds mem's epistemic writer, wiring the semantic-index accelerator when the
/// `[semantic_index]` config enables it.
///
/// - **Disabled** ⇒ `MemWriter::without_index` (recall falls back to the non-vector
///   path; no drag).
/// - **Enabled** ⇒ concrete `HnswIndex` + configured `Embedder`, rebuilt from the
///   graph before serving recalls, with retrieval-provenance recording per config.
///   The `SemanticIndex` trait is the swap seam: a future `typedb-native` backend
///   drops in here.
async fn build_epistemic_writer(
    store: std::sync::Arc<dyn GraphStore>,
    cfg: &GeckoConfig,
) -> Result<std::sync::Arc<dyn EpistemicWriter>> {
    let sic = &cfg.semantic_index;
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

    // Embedder selection: `"stub"` → the deterministic hash embedder (always
    // compiled); `"candle"` → the real `CandleEmbedder`, which exists ONLY in a
    // `--features real-embedder` build. Any other value is a hard, actionable error
    // rather than a silent fallback to candle.
    let (embedder, embedder_kind): (std::sync::Arc<dyn Embedder>, &str) = if sic.embedder == "stub"
    {
        (std::sync::Arc::new(StubEmbedder::new(384)), "stub")
    } else if sic.embedder == "candle" {
        #[cfg(feature = "real-embedder")]
        {
            // The model is a runtime asset — resolved to a PATH; loaded lazily on
            // the first embed (never here, never on a doc-only run).
            let model_dir = config::model_path(cfg)?;
            let candle = CandleEmbedder::new(
                sic.model_id.clone(),
                model_dir,
                sic.auto_fetch,
                sic.model_source.clone(),
            );
            (std::sync::Arc::new(candle), "candle")
        }
        #[cfg(not(feature = "real-embedder"))]
        {
            anyhow::bail!(
                "semantic_index.embedder 'candle' selects the real embedder, which is not \
                 compiled into this binary (build with `--features real-embedder`); only \
                 'stub' is available in this build"
            );
        }
    } else {
        anyhow::bail!(
            "unknown semantic_index.embedder '{}': expected 'stub' or 'candle'",
            sic.embedder
        );
    };
    let dim = embedder.dim();

    // Resolve the effective index path: derived under the cache root by default,
    // or an explicit `[semantic_index] path` override.
    let resolved_index_path = config::index_path(cfg)?;
    let index = std::sync::Arc::new(HnswIndex::open(
        &resolved_index_path,
        embedder.model_id(),
        dim,
    )?);
    let index: std::sync::Arc<dyn SemanticIndex> = index;

    let writer = mem_gecko::MemWriter::new(store, Some(embedder), Some(index))
        .with_retrieval_provenance(sic.record_provenance());
    // Reconstruct the accelerator from the graph (the source of truth) before
    // serving recalls. For the real embedder this stays lazy: an empty/doc-only
    // graph enumerates no embeddables, so the 1.3GB model is never loaded here.
    writer.rebuild_index_from_graph().await?;
    info!(
        path = %resolved_index_path.display(),
        embedder = embedder_kind,
        dim,
        record_provenance = sic.record_provenance(),
        "Semantic index enabled (hnsw); rebuilt from graph"
    );
    Ok(std::sync::Arc::new(writer))
}

/// A concept's executable program plus the metadata `gecko run` needs to execute
/// it: the (informational) engine token and the capability scopes it grants.
struct FetchedPlaybook {
    code_block: String,
    engine_type: String,
    scopes: Vec<String>,
}

/// Fetches a concept's single code-block, engine, and granted scopes from the graph.
///
/// Single-bundle-scoped: concept IDs are exact bundle-relative paths, and each
/// concept has exactly one executable program (its engine-matched code fences were
/// concatenated at parse time), so there is no ambiguity to resolve. `code` and
/// `scopes` are list projections to tolerate the 0-or-1 / 0-or-N cardinalities.
async fn fetch_playbook(db: &mut TypeDbRouter, concept_id: &str) -> Result<FetchedPlaybook> {
    let tx = db
        .begin_read()
        .await
        .context("Failed to begin read transaction")?;

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
        // Surface a stream error rather than masking it as "playbook not found".
        if let Some(item) = stream.next().await {
            let doc = item.context("error reading playbook document stream")?;
            doc_json = serde_json::to_value(doc.into_json()).ok();
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
    // (QuickJS) boundary.
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

    Ok(FetchedPlaybook {
        code_block,
        engine_type,
        scopes,
    })
}

/// Builds the sandbox host-call bridge for a run. Constructs mem's epistemic writer
/// over its own graph connection and injects it into each activated extension; when
/// an epistemic writer is present it wraps the base import dispatcher so `mem.*`
/// host fns reach the writer stamped with a host-minted `RunContext` (invariant 2:
/// the sandbox cannot forge or omit `run_id`/`actor`/`source`). Registration is the
/// gate — only these extensions' imports are reachable from the sandbox.
async fn build_host_bridge(
    concept_id: &str,
    cfg: &GeckoConfig,
    writer_config: DbConfig,
    extensions: Vec<Box<dyn GeckoExtension>>,
) -> Result<gecko_engine::sandbox::engine::ExtensionCallback> {
    let graph_router =
        std::sync::Arc::new(tokio::sync::Mutex::new(TypeDbRouter::new(writer_config)));
    let store = std::sync::Arc::new(RouterGraphStore::new(graph_router));
    let writer: std::sync::Arc<dyn EpistemicWriter> = build_epistemic_writer(store, cfg).await?;

    // mem (forced-on, first) binds the writer into the sandbox context; other
    // extensions inherit the no-op.
    let mut sandbox_ctx = SandboxCtx::new();
    for ext in &extensions {
        ext.inject_epistemic_host_fns(&mut sandbox_ctx, writer.clone());
    }

    // The base bridge dispatches plain host imports over the activated extensions.
    let base_cb: gecko_engine::sandbox::engine::ExtensionCallback =
        std::sync::Arc::new(move |ext_name, func_name, args| {
            for ext in &extensions {
                if ext.name() == ext_name {
                    return ext.call_import(func_name, &args);
                }
            }
            Err(format!("Extension '{ext_name}' not found"))
        });

    // If an epistemic extension was activated, wrap the base bridge so mem host fns
    // (`remember`/`derive`/`supersede`/`contest`) reach the injected writer — each
    // stamped with a host-minted RunContext bound to the executing concept
    // (`ExecutableDoc`). Exactly one RunContext is minted per run.
    let ext_cb = match sandbox_ctx.epistemic_writer() {
        Some(writer) => {
            let run_ctx = RunContext::new(
                ActorId::new("system"),
                ProvenanceSource::ExecutableDoc {
                    concept_id: ConceptId::new(concept_id),
                },
                chrono::Utc::now(),
            );
            // mem's writer is also a reader, so wire the `mem.recall` reader bridge
            // from the same object (shared per-run scratch). See
            // docs/refactor/KNOWN-LIMITATIONS.md.
            let reader = writer.clone().as_epistemic_reader();
            epistemic_extension_callback(run_ctx, writer, reader, base_cb)
        }
        None => base_cb,
    };
    Ok(ext_cb)
}

/// Prints the outcome of a pipeline execution.
fn render_execution_result(result: &gecko_engine::sandbox::engine::ExecutionResult) {
    if result.success {
        println!("Execution successful!");
        println!("Output: {}", result.output);
        println!("Duration: {} ms", result.duration_ms);
    } else {
        println!("Execution failed!");
        if let Some(err) = &result.error {
            println!("Error: {err}");
        }
    }
}

/// Execute a playbook concept in the sandbox.
async fn cmd_run(
    config: DbConfig,
    cfg: &GeckoConfig,
    concept_id: &str,
    extensions: Vec<Box<dyn GeckoExtension>>,
) -> Result<()> {
    println!("Executing playbook: {concept_id}");

    // The epistemic writer needs its own connection to the same database: the
    // pipeline borrows `db` mutably for the run, while belief-tier writes commit
    // through a second router (belief writes are sparse — invariant 4 — so a
    // dedicated serialised connection is not a bottleneck).
    let writer_config = config.clone();
    let mut db = TypeDbRouter::new(config);

    let playbook = fetch_playbook(&mut db, concept_id).await?;
    println!("Resolved to: {concept_id}");
    println!(
        "Extensions loaded: {:?}",
        extensions.iter().map(|e| e.name()).collect::<Vec<_>>()
    );
    println!("Running script using engine: {}...", playbook.engine_type);

    let ext_cb = build_host_bridge(concept_id, cfg, writer_config, extensions).await?;

    // Execute through the pipeline: it owns the state-handle lifecycle (RAII), the
    // shared sandbox pool, and the execution record — no execution logic is
    // duplicated here. (timeout-ms is not persisted, so the sandbox default applies;
    // the pipeline honors an explicit timeout when given.)
    let state_registry = gecko_engine::state::registry::StateRegistry::new();
    let sandbox_pool = gecko_engine::sandbox::wasm_pool::SandboxPool::new();
    let host_imports = HostImports::default();

    let run = gecko_engine::pipeline::PlaybookRun {
        concept_id,
        program: &playbook.code_block,
        scopes: &playbook.scopes,
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

    render_execution_result(&pipeline_result.execution);
    Ok(())
}
