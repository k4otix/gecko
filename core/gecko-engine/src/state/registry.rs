//! Host-Managed State Registry with RAII lifecycle.
//!
//! Implementation of design §3.4. Data serialization across the Wasm boundary
//! is eliminated by keeping state on the host and providing opaque UUID handles.
//! The `StateHandleGuard` auto-cleans on drop via Rust's RAII pattern.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use std::sync::RwLock;
use uuid::Uuid;

/// An investigation ledger entry tracked in the state registry.
#[derive(Debug, Clone)]
pub struct InvestigationLedger {
    /// Accumulated state entries (JSON-serializable data).
    pub entries: Vec<serde_json::Value>,
    /// When this ledger was created.
    pub created_at: DateTime<Utc>,
}

impl InvestigationLedger {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            created_at: Utc::now(),
        }
    }
}

impl Default for InvestigationLedger {
    fn default() -> Self {
        Self::new()
    }
}

/// Host-side state registry. Wasm guests interact with state via opaque UUID handles.
///
/// Thread-safe via `Arc<RwLock<...>>` — safe for concurrent Tokio tasks.
#[derive(Debug, Clone)]
pub struct StateRegistry {
    ledgers: Arc<RwLock<HashMap<Uuid, InvestigationLedger>>>,
}

impl StateRegistry {
    pub fn new() -> Self {
        Self {
            ledgers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Allocates a new state handle, returning a RAII guard.
    ///
    /// When the guard is dropped (task completion, panic, or rollback),
    /// the associated ledger is automatically removed from the registry.
    pub fn allocate(&self) -> StateHandleGuard {
        let id = Uuid::new_v4();
        let mut map = self.ledgers.write().unwrap();
        map.insert(id, InvestigationLedger::new());

        StateHandleGuard {
            id,
            registry: Arc::clone(&self.ledgers),
        }
    }

    /// Returns the number of active handles.
    pub fn active_count(&self) -> usize {
        self.ledgers.read().unwrap().len()
    }

    /// Reads a ledger by handle ID (for host-side introspection).
    pub fn read_ledger(&self, id: &Uuid) -> Option<InvestigationLedger> {
        self.ledgers.read().unwrap().get(id).cloned()
    }
}

impl Default for StateRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard for a state handle.
///
/// When this guard goes out of scope (Tokio task completes, panics, or rolls back),
/// the `Drop` implementation automatically removes the associated ledger from
/// the `StateRegistry`, freeing the memory instantly.
///
/// Design §3.4: "When the Tokio task finishes (by success, panic, or rollback),
/// this Drop implementation automatically executes, freeing the memory instantly."
pub struct StateHandleGuard {
    /// Opaque 128-bit handle passed to Wasm guests.
    pub id: Uuid,
    registry: Arc<RwLock<HashMap<Uuid, InvestigationLedger>>>,
}

impl StateHandleGuard {
    /// Appends an entry to this handle's ledger.
    pub fn append(&self, entry: serde_json::Value) {
        let mut map = self.registry.write().unwrap();
        if let Some(ledger) = map.get_mut(&self.id) {
            ledger.entries.push(entry);
        }
    }

    /// Reads all entries from this handle's ledger.
    pub fn read_entries(&self) -> Vec<serde_json::Value> {
        let map = self.registry.read().unwrap();
        map.get(&self.id)
            .map(|l| l.entries.clone())
            .unwrap_or_default()
    }
}

impl Drop for StateHandleGuard {
    fn drop(&mut self) {
        let mut map = self.registry.write().unwrap();
        map.remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allocate_and_drop() {
        let registry = StateRegistry::new();
        assert_eq!(registry.active_count(), 0);

        let handle = registry.allocate();
        let handle_id = handle.id;
        assert_eq!(registry.active_count(), 1);
        assert!(registry.read_ledger(&handle_id).is_some());

        // Drop the handle -- should auto-clean
        drop(handle);
        assert_eq!(registry.active_count(), 0);
        assert!(registry.read_ledger(&handle_id).is_none());
    }

    #[test]
    fn test_append_and_read() {
        let registry = StateRegistry::new();
        let handle = registry.allocate();

        handle.append(serde_json::json!({"alert": "suspicious_login"}));
        handle.append(serde_json::json!({"action": "isolate_host"}));

        let entries = handle.read_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["alert"], "suspicious_login");
        assert_eq!(entries[1]["action"], "isolate_host");
    }

    #[test]
    fn test_multiple_concurrent_handles() {
        let registry = StateRegistry::new();

        let h1 = registry.allocate();
        let h2 = registry.allocate();
        let h3 = registry.allocate();

        assert_eq!(registry.active_count(), 3);

        drop(h2);
        assert_eq!(registry.active_count(), 2);

        drop(h1);
        drop(h3);
        assert_eq!(registry.active_count(), 0);
    }
}
