//! Async TypeDB connection and transaction management.
//!
//! Core TypeDB router and connection manager for dual Core/Cloud
//! targeting via `TlsMode`.

use std::path::PathBuf;
use std::sync::Arc;

use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::info;

use gecko_extension_api::SchemaMigration;

use super::schema_apply;
use super::surface::BeliefTierChange;
use typedb_driver::{
    Addresses, Credentials, DriverOptions, DriverTlsConfig, Transaction, TransactionType,
    TypeDBDriver,
};

#[derive(Debug, Error)]
pub enum DbError {
    #[error("TypeDB connection error: {0}")]
    Connection(String),

    #[error("TypeDB transaction error: {0}")]
    Transaction(String),

    #[error("TypeDB schema error: {0}")]
    Schema(String),

    #[error("TypeDB query error: {0}")]
    Query(String),

    /// A sync shipped concept IDs that the database already holds outside the
    /// syncing bundle. `bundle` is the incoming bundle's name; `ids` lists every
    /// colliding ID, sorted, with the bundle holding it (`None` when no bundle
    /// contains it).
    #[error("{}", collision_message(.bundle, .ids))]
    ConceptIdCollision {
        bundle: String,
        ids: Vec<(String, Option<String>)>,
    },

    /// A sync changed the belief tier: at least one belief-tier label's instance,
    /// attribute or role-player count differs between the start of the sync
    /// transaction and just before its commit. Nothing was committed.
    #[error("{}", belief_tier_write_message(.changes))]
    BeliefTierWrite { changes: Vec<BeliefTierChange> },
}

/// Renders a refused belief-tier change, listing every differing count.
fn belief_tier_write_message(changes: &[BeliefTierChange]) -> String {
    let list: Vec<String> = changes.iter().map(ToString::to_string).collect();
    format!(
        "sync refused: it changed the belief tier ({}). Only the epistemic write path may \
         change the belief tier; nothing was committed.",
        list.join(", ")
    )
}

/// The most colliding IDs a [`DbError::ConceptIdCollision`] message lists.
const COLLISION_IDS_SHOWN: usize = 20;

/// Renders a concept-ID collision: the first [`COLLISION_IDS_SHOWN`] IDs with
/// their holders, a count of the rest, and how to resolve it.
fn collision_message(bundle: &str, ids: &[(String, Option<String>)]) -> String {
    let shown: Vec<String> = ids
        .iter()
        .take(COLLISION_IDS_SHOWN)
        .map(|(id, holder)| match holder {
            Some(h) => format!("`{id}` (bundle `{h}`)"),
            None => format!("`{id}` (no bundle)"),
        })
        .collect();
    let mut list = shown.join(", ");
    if ids.len() > COLLISION_IDS_SHOWN {
        list.push_str(&format!(" and {} more", ids.len() - COLLISION_IDS_SHOWN));
    }
    format!(
        "bundle `{bundle}` ships {} concept ID(s) this database already holds outside it: {list}. \
         Concept IDs must be unique per database: sync this bundle under the name that holds \
         them, or into a fresh database with `--database`.",
        ids.len()
    )
}

/// TLS mode for TypeDB connections.
#[derive(Debug, Clone, Default)]
pub enum TlsMode {
    /// TypeDB Core (local dev/test) — no TLS.
    #[default]
    Disabled,
    /// TypeDB Cloud — requires TLS, optionally with a custom CA certificate.
    Enabled { ca_cert: Option<PathBuf> },
}

/// Configuration for TypeDB connections.
#[derive(Debug, Clone)]
pub struct DbConfig {
    /// Server address. Core default: "localhost:1729".
    pub address: String,
    /// Target database name. Default: "gecko".
    pub database: String,
    /// Authentication username.
    pub username: String,
    /// Authentication password.
    pub password: String,
    /// TLS mode: Disabled for Core, Enabled for Cloud.
    pub tls: TlsMode,
}

impl Default for DbConfig {
    fn default() -> Self {
        Self {
            address: "localhost:1729".to_string(),
            database: "gecko".to_string(),
            username: "admin".to_string(),
            password: "password".to_string(),
            tls: TlsMode::Disabled,
        }
    }
}

/// Builds a `map_err` closure that turns any displayable driver error into the
/// given [`DbError`] variant via `.to_string()`. Factors out the
/// `.map_err(|e| DbError::X(e.to_string()))` pattern repeated across this
/// module's driver calls; `ctor` is one of `DbError`'s tuple-variant
/// constructors (e.g. `DbError::Connection`).
fn to_db_err<E: std::fmt::Display>(ctor: fn(String) -> DbError) -> impl Fn(E) -> DbError {
    move |e| ctor(e.to_string())
}

/// TypeDB connection and transaction router.
///
/// Owns the driver lifecycle (connect, database creation, schema application) and
/// vends short-lived transactions to callers that hold it exclusively. For the
/// shared read/write hot path, [`Self::graph_handle`] hands out a cheap, cloneable
/// [`GraphHandle`] that opens transactions without going through this router again.
pub struct TypeDbRouter {
    config: DbConfig,
    driver: Option<Arc<TypeDBDriver>>,
    write_gate: Arc<Semaphore>,
}

impl TypeDbRouter {
    pub fn new(config: DbConfig) -> Self {
        Self {
            config,
            driver: None,
            write_gate: Arc::new(Semaphore::new(1)),
        }
    }

    /// Establish connection with the TypeDB server.
    pub async fn connect(&mut self) -> Result<(), DbError> {
        if self.driver.is_some() {
            return Ok(());
        }

        let addresses = Addresses::try_from_address_str(&self.config.address)
            .map_err(to_db_err(DbError::Connection))?;

        let credentials = Credentials::new(&self.config.username, &self.config.password);

        let tls_config = match &self.config.tls {
            TlsMode::Disabled => DriverTlsConfig::disabled(),
            TlsMode::Enabled { ca_cert } => {
                if let Some(cert_path) = ca_cert {
                    DriverTlsConfig::enabled_with_root_ca(cert_path)
                        .map_err(to_db_err(DbError::Connection))?
                } else {
                    DriverTlsConfig::enabled_with_native_root_ca()
                }
            }
        };

        let options = DriverOptions::new(tls_config);

        let driver = TypeDBDriver::new(addresses, credentials, options)
            .await
            .map_err(to_db_err(DbError::Connection))?;

        info!(address = %self.config.address, "Connected to TypeDB");
        self.driver = Some(Arc::new(driver));
        Ok(())
    }

    /// Close connection to TypeDB.
    pub fn close(&mut self) {
        if let Some(driver) = self.driver.take() {
            let _ = driver.force_close();
            info!("TypeDB connection closed");
        }
    }

    /// Returns a shared handle to the active driver, connecting if necessary. The
    /// clone is cheap — `TypeDBDriver` is an `Arc`-backed handle that multiplexes
    /// concurrent transactions.
    async fn driver(&mut self) -> Result<Arc<TypeDBDriver>, DbError> {
        self.connect().await?;
        self.driver
            .clone()
            .ok_or_else(|| DbError::Connection("Driver not available after connect".into()))
    }

    /// Connects if necessary and returns a cheap, cloneable [`GraphHandle`] for the
    /// shared read/write path. The handle opens read transactions concurrently
    /// (lock-free) and serializes writers through this router's single-permit gate,
    /// so belief-tier commits stay one-at-a-time and atomic.
    pub async fn graph_handle(&mut self) -> Result<GraphHandle, DbError> {
        let driver = self.driver().await?;
        Ok(GraphHandle {
            driver,
            database: self.config.database.clone(),
            write_gate: self.write_gate.clone(),
        })
    }

    /// Creates the target database if it does not already exist.
    pub async fn ensure_database(&mut self) -> Result<(), DbError> {
        let db_name = self.config.database.clone();
        let driver = self.driver().await?;

        let exists = driver
            .databases()
            .contains(&db_name)
            .await
            .map_err(to_db_err(DbError::Connection))?;

        if !exists {
            driver
                .databases()
                .create(&db_name)
                .await
                .map_err(to_db_err(DbError::Connection))?;
            info!(database = %db_name, "Created database");
        }

        Ok(())
    }

    /// Deletes a database by name if it exists (a no-op if it does not).
    ///
    /// Refuses to delete the sacrosanct `"gecko"` database — the one real,
    /// long-lived database this engine ships against. Integration tests target
    /// their own uniquely-named throwaway databases and must never reach this
    /// guard.
    pub async fn delete_database(&mut self, name: &str) -> Result<(), DbError> {
        if name == "gecko" {
            return Err(DbError::Connection(
                "refusing to delete the reserved 'gecko' database".to_string(),
            ));
        }
        let driver = self.driver().await?;
        let exists = driver
            .databases()
            .contains(name)
            .await
            .map_err(to_db_err(DbError::Connection))?;
        if exists {
            let database = driver
                .databases()
                .get(name)
                .await
                .map_err(to_db_err(DbError::Connection))?;
            database
                .delete()
                .await
                .map_err(to_db_err(DbError::Connection))?;
            info!(database = %name, "Deleted database");
        }
        Ok(())
    }

    /// Lists the names of all databases on the server.
    pub async fn list_databases(&mut self) -> Result<Vec<String>, DbError> {
        let driver = self.driver().await?;
        let all = driver
            .databases()
            .all()
            .await
            .map_err(to_db_err(DbError::Connection))?;
        Ok(all.iter().map(|d| d.name().to_string()).collect())
    }

    /// The full schema (types and functions) of the existing target database as
    /// TypeQL `define` text; errors when the database does not exist.
    pub async fn database_schema(&mut self) -> Result<String, DbError> {
        let db_name = self.config.database.clone();
        let driver = self.driver().await?;
        driver
            .databases()
            .get(&db_name)
            .await
            .map_err(to_db_err(DbError::Schema))?
            .schema()
            .await
            .map_err(to_db_err(DbError::Schema))
    }

    /// Installs a TypeQL schema definition into the database, safely re-appliable.
    ///
    /// Types are `define`d (idempotent for unchanged definitions). A function the
    /// database holds with a different body is `redefine`d rather than re-`define`d,
    /// which TypeDB refuses (`[FUN5]`); new functions are `define`d; unchanged ones
    /// are skipped. Functions are written callee-first (see [`schema_apply`]).
    /// Everything runs in one schema transaction. Functions the database holds but
    /// the fragment omits are left in place. A changed type annotation needs a
    /// [`SchemaMigration`]; see [`apply_schema_with_migrations`](Self::apply_schema_with_migrations).
    pub async fn apply_schema(&mut self, schema_content: &str) -> Result<(), DbError> {
        self.apply_schema_with_migrations(schema_content, &[]).await
    }

    /// [`apply_schema`](Self::apply_schema), preceded in the same schema transaction
    /// by every migration whose `applies_when` text is still present in the
    /// database's schema, in order.
    pub async fn apply_schema_with_migrations(
        &mut self,
        schema_content: &str,
        migrations: &[SchemaMigration],
    ) -> Result<(), DbError> {
        self.ensure_database().await?;
        let db_name = self.config.database.clone();
        let driver = self.driver().await?;

        let current = self.database_schema().await?;
        let pending: Vec<&SchemaMigration> = migrations
            .iter()
            .filter(|m| current.contains(m.applies_when))
            .collect();
        let stored = schema_apply::stored_functions(&current).map_err(DbError::Schema)?;
        let queries =
            schema_apply::plan_queries(schema_content, &stored).map_err(DbError::Schema)?;

        let tx = driver
            .transaction(&db_name, TransactionType::Schema)
            .await
            .map_err(to_db_err(DbError::Schema))?;

        for m in &pending {
            tx.query(m.tql)
                .await
                .map_err(|e| DbError::Schema(format!("schema migration '{}' failed: {e}", m.id)))?;
        }
        for query in &queries {
            tx.query(query.as_str())
                .await
                .map_err(to_db_err(DbError::Schema))?;
        }

        tx.commit().await.map_err(to_db_err(DbError::Schema))?;

        for m in &pending {
            info!(database = %db_name, migration = m.id, "Schema migration applied");
        }
        info!(database = %db_name, "Schema applied");
        Ok(())
    }

    /// Begins a write transaction. Caller must commit it.
    pub async fn begin_write(&mut self) -> Result<Transaction, DbError> {
        let db_name = self.config.database.clone();
        let driver = self.driver().await?;

        driver
            .transaction(&db_name, TransactionType::Write)
            .await
            .map_err(to_db_err(DbError::Transaction))
    }

    /// Begins a schema transaction, bypassing [`apply_schema`](Self::apply_schema)'s
    /// function-aware planning. Caller must commit it.
    pub async fn begin_schema(&mut self) -> Result<Transaction, DbError> {
        self.ensure_database().await?;
        let db_name = self.config.database.clone();
        let driver = self.driver().await?;

        driver
            .transaction(&db_name, TransactionType::Schema)
            .await
            .map_err(to_db_err(DbError::Schema))
    }

    /// Begins a read transaction.
    pub async fn begin_read(&mut self) -> Result<Transaction, DbError> {
        let db_name = self.config.database.clone();
        let driver = self.driver().await?;

        driver
            .transaction(&db_name, TransactionType::Read)
            .await
            .map_err(to_db_err(DbError::Transaction))
    }

    /// Reads the connected server's version string (e.g. `"3.12.0"`), connecting
    /// if necessary. A server-level RPC — it does not create, open, or touch any
    /// database. Used by `gecko doctor`'s `typedb` precondition check to compare
    /// the live server against the pinned version.
    pub async fn server_version(&mut self) -> Result<String, DbError> {
        let driver = self.driver().await?;
        let version = driver
            .server_version()
            .await
            .map_err(to_db_err(DbError::Connection))?;
        Ok(version.version().to_string())
    }

    /// Returns the database name.
    pub fn database(&self) -> &str {
        &self.config.database
    }

    /// Returns the server address.
    pub fn address(&self) -> &str {
        &self.config.address
    }
}

impl Drop for TypeDbRouter {
    fn drop(&mut self) {
        self.close();
    }
}

/// A cheap, cloneable handle to an established TypeDB connection.
///
/// Opens read transactions lock-free — the driver multiplexes concurrent
/// transactions and TypeDB gives each reader a consistent snapshot — while write
/// transactions pass through a shared single-permit gate so belief-tier commits
/// are serialized (one writer at a time, each batch atomic). Clones share the same
/// driver and gate; construct one via [`TypeDbRouter::graph_handle`].
#[derive(Clone)]
pub struct GraphHandle {
    driver: Arc<TypeDBDriver>,
    database: String,
    write_gate: Arc<Semaphore>,
}

impl GraphHandle {
    /// Opens a read transaction. Independent of any other reader or writer.
    pub async fn begin_read(&self) -> Result<Transaction, DbError> {
        self.driver
            .transaction(&self.database, TransactionType::Read)
            .await
            .map_err(to_db_err(DbError::Transaction))
    }

    /// Acquires the single-writer gate, then opens a write transaction. Acquiring
    /// before opening keeps a queued writer from holding an idle server-side
    /// transaction. The returned [`WriteTransaction`] holds the permit until it is
    /// committed or dropped, so writers never overlap.
    pub async fn begin_write(&self) -> Result<WriteTransaction, DbError> {
        let permit = self
            .write_gate
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| DbError::Transaction("write gate closed".to_string()))?;
        let tx = self
            .driver
            .transaction(&self.database, TransactionType::Write)
            .await
            .map_err(to_db_err(DbError::Transaction))?;
        Ok(WriteTransaction {
            tx,
            _permit: permit,
        })
    }
}

/// A write transaction bound to the single-writer gate.
///
/// Run statements against [`transaction`](Self::transaction), then
/// [`commit`](Self::commit). The gate permit is released only when this value is
/// dropped — after the commit completes — so no other writer can start mid-batch.
pub struct WriteTransaction {
    tx: Transaction,
    _permit: OwnedSemaphorePermit,
}

impl WriteTransaction {
    /// The underlying transaction, for issuing queries.
    pub fn transaction(&self) -> &Transaction {
        &self.tx
    }

    /// Commits the batch, holding the writer gate until the commit resolves.
    pub async fn commit(self) -> Result<(), DbError> {
        let WriteTransaction { tx, _permit } = self;
        tx.commit().await.map_err(to_db_err(DbError::Transaction))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collision_message_lists_at_most_twenty_ids() {
        let ids: Vec<(String, Option<String>)> = (0..25)
            .map(|i| (format!("id{i:02}"), Some("alpha".to_string())))
            .collect();
        let msg = collision_message("beta", &ids);
        assert!(msg.contains("`beta`"), "{msg}");
        assert!(msg.contains("id00"), "{msg}");
        assert!(msg.contains("id19"), "{msg}");
        assert!(!msg.contains("id20"), "{msg}");
        assert!(msg.contains("and 5 more"), "{msg}");
        assert!(msg.contains("fresh database"), "{msg}");
    }

    #[test]
    fn collision_message_names_an_uncontained_holder() {
        let msg = collision_message("beta", &[("x".to_string(), None)]);
        assert!(msg.contains("no bundle"), "{msg}");
    }
}
