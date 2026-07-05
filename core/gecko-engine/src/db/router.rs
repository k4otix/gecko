//! Async TypeDB connection and transaction management.
//!
//! Port of Tyke's `db.py` (`TypeDBManager`), extended with dual Core/Cloud
//! targeting via `TlsMode`.

use std::path::PathBuf;

use thiserror::Error;
use tracing::info;
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

/// TypeDB connection and transaction router.
///
/// Port of Tyke's `TypeDBManager`. Manages driver lifecycle,
/// database creation, schema application, and transaction scoping.
pub struct TypeDbRouter {
    config: DbConfig,
    driver: Option<TypeDBDriver>,
}

impl TypeDbRouter {
    pub fn new(config: DbConfig) -> Self {
        Self {
            config,
            driver: None,
        }
    }

    /// Establish connection with the TypeDB server.
    pub async fn connect(&mut self) -> Result<(), DbError> {
        if self.driver.is_some() {
            return Ok(());
        }

        let addresses = Addresses::try_from_address_str(&self.config.address)
            .map_err(|e| DbError::Connection(e.to_string()))?;

        let credentials = Credentials::new(&self.config.username, &self.config.password);

        let tls_config = match &self.config.tls {
            TlsMode::Disabled => DriverTlsConfig::disabled(),
            TlsMode::Enabled { ca_cert } => {
                if let Some(cert_path) = ca_cert {
                    DriverTlsConfig::enabled_with_root_ca(cert_path)
                        .map_err(|e| DbError::Connection(e.to_string()))?
                } else {
                    DriverTlsConfig::enabled_with_native_root_ca()
                }
            }
        };

        let options = DriverOptions::new(tls_config);

        let driver = TypeDBDriver::new(addresses, credentials, options)
            .await
            .map_err(|e| DbError::Connection(e.to_string()))?;

        info!(address = %self.config.address, "Connected to TypeDB");
        self.driver = Some(driver);
        Ok(())
    }

    /// Close connection to TypeDB.
    pub fn close(&mut self) {
        if let Some(driver) = self.driver.take() {
            let _ = driver.force_close();
            info!("TypeDB connection closed");
        }
    }

    /// Returns a reference to the active driver, connecting if necessary.
    async fn driver(&mut self) -> Result<&TypeDBDriver, DbError> {
        self.connect().await?;
        self.driver
            .as_ref()
            .ok_or_else(|| DbError::Connection("Driver not available after connect".into()))
    }

    /// Creates the target database if it does not already exist.
    pub async fn ensure_database(&mut self) -> Result<(), DbError> {
        let db_name = self.config.database.clone();
        let driver = self.driver().await?;

        let exists = driver
            .databases()
            .contains(&db_name)
            .await
            .map_err(|e| DbError::Connection(e.to_string()))?;

        if !exists {
            driver
                .databases()
                .create(&db_name)
                .await
                .map_err(|e| DbError::Connection(e.to_string()))?;
            info!(database = %db_name, "Created database");
        }

        Ok(())
    }

    /// Installs a TypeQL schema definition into the database.
    pub async fn apply_schema(&mut self, schema_content: &str) -> Result<(), DbError> {
        self.ensure_database().await?;
        let db_name = self.config.database.clone();
        let driver = self.driver().await?;

        let tx = driver
            .transaction(&db_name, TransactionType::Schema)
            .await
            .map_err(|e| DbError::Schema(e.to_string()))?;

        tx.query(schema_content)
            .await
            .map_err(|e| DbError::Schema(e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| DbError::Schema(e.to_string()))?;

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
            .map_err(|e| DbError::Transaction(e.to_string()))
    }

    /// Begins a read transaction.
    pub async fn begin_read(&mut self) -> Result<Transaction, DbError> {
        let db_name = self.config.database.clone();
        let driver = self.driver().await?;

        driver
            .transaction(&db_name, TransactionType::Read)
            .await
            .map_err(|e| DbError::Transaction(e.to_string()))
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
