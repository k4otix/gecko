//! Extension trait for pluggable domain logic.
//!
//! The [`GeckoExtension`] contract now lives in the standalone
//! [`gecko-extension-api`] crate so that extensions can implement it without
//! depending on the whole engine. This module re-exports it for engine-internal
//! use and to preserve the `gecko_engine::extension::GeckoExtension` path.
//!
//! [`gecko-extension-api`]: gecko_extension_api

pub use gecko_extension_api::GeckoExtension;
