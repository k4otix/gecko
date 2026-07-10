//! Identity newtypes for the epistemic substrate.
//!
//! GECKO's id scheme is string ULIDs (`mem/ep/{ulid}`, `mem/bel/{ulid}`) and
//! `concept-id` is a string `@key`, so the entity/belief/actor/anomaly ids are
//! string newtypes. [`RunId`] is the one exception: a run is minted fresh by the
//! host per op, so it wraps a real [`ulid::Ulid`] value.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Datetime anchor used across the epistemic contract (bitemporal event/ingest
/// times, `valid-from`, etc.). Aliased so call sites read `DateTime`, matching
/// the plan's signatures.
pub type DateTime = chrono::DateTime<chrono::Utc>;

/// Declares a string newtype id with the usual conveniences.
macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub struct $name(pub String);

        impl $name {
            /// Wraps an owned or borrowed string as this id.
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            /// Borrows the underlying string.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }
    };
}

string_id! {
    /// Id of a stored memory node — an episode (`mem/ep/{ulid}`) or belief
    /// (`mem/bel/{ulid}`).
    MemId
}

string_id! {
    /// Id of an authored/record concept — the `concept-id` string `@key`.
    ConceptId
}

string_id! {
    /// Id of an actor (agent | human | system) that authors a write.
    ActorId
}

string_id! {
    /// Id of a contradiction/anomaly node minted by `contest`.
    AnomalyId
}

/// A run identity — one per exec-doc run, sync, tool-call, or manual op.
///
/// The host mints this fresh at the entry of any write-capable op and threads it
/// through [`crate::RunContext`]. Sandbox code never constructs its own (invariant
/// 2): a run id is the provenance stamp that makes every belief write attributable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RunId(pub ulid::Ulid);

impl RunId {
    /// Mints a fresh, monotonic run id. Host-only by convention.
    pub fn new() -> Self {
        Self(ulid::Ulid::new())
    }

    /// Parses a canonical ULID string (e.g. from a persisted run stamp).
    pub fn from_string(s: &str) -> Result<Self, ulid::DecodeError> {
        Ok(Self(ulid::Ulid::from_string(s)?))
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_id_roundtrips_through_str() {
        let id = MemId::new("mem/bel/01H");
        assert_eq!(id.as_str(), "mem/bel/01H");
        assert_eq!(id, MemId::from("mem/bel/01H"));
        assert_eq!(id.to_string(), "mem/bel/01H");
    }

    #[test]
    fn run_id_roundtrips_through_string() {
        let id = RunId::new();
        let parsed = RunId::from_string(&id.to_string()).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn distinct_run_ids_are_unique() {
        assert_ne!(RunId::new(), RunId::new());
    }
}
