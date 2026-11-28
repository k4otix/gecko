//! OKF v0.2 provenance, trust and lifecycle frontmatter (OKF §5.2–§5.5).
//!
//! A malformed value is never refused (OKF §11 treats these constraints as soft
//! guidance) and never replaced by an invented one: its key stays in the frontmatter
//! map, so it lands in `metadata-json` as written, and a warning names it. A key that
//! holds a subkey OKF does not define also stays in the map, with no warning, so the
//! producer's extension is preserved (§4.1).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_yaml::Value;

use super::parser::yaml_as_written;
use super::types::{OkfProvenance, TrustTier, Verification};

/// What a concept's provenance keys give: the content time (`generated.at`, or the
/// legacy `timestamp`) and the typed provenance facts.
pub(crate) struct ConceptProvenance {
    pub timestamp: Option<DateTime<Utc>>,
    pub provenance: OkfProvenance,
}

/// Takes the OKF v0.2 provenance keys out of `map`, leaving behind every key that
/// is malformed or carries an unknown subkey, and appends a warning for each
/// malformed value of the concept file at `path`.
pub(crate) fn take_provenance(
    map: &mut HashMap<String, Value>,
    path: &str,
    warnings: &mut Vec<String>,
) -> ConceptProvenance {
    let mut warn = |key: &str, value: &Value| {
        warnings.push(format!(
            "Concept file '{path}' has an invalid '{key}' ({}); it is kept in \
             metadata-json and not typed",
            yaml_as_written(value)
        ));
    };
    let mut provenance = OkfProvenance::default();

    // `generated: { by, at }`: `by` is required, `at` optional.
    let mut generated_at = None;
    if let Some(v) = map.remove("generated") {
        let mut keep = true;
        if let Value::Mapping(m) = &v {
            keep = m.keys().any(|k| !matches!(k.as_str(), Some("by" | "at")));
            match m.get("by") {
                Some(by) if let Some(by) = non_blank(by) => provenance.generated_by = Some(by),
                Some(by) => {
                    warn("generated.by", by);
                    keep = true;
                }
                None => {
                    warn("generated.by", &v);
                    keep = true;
                }
            }
            if let Some(at) = m.get("at") {
                match parse_time(at) {
                    Some(t) => generated_at = Some(t),
                    None => {
                        warn("generated.at", at);
                        keep = true;
                    }
                }
            }
        } else {
            warn("generated", &v);
        }
        if keep {
            map.insert("generated".to_string(), v);
        }
    }

    // The v0.1 `timestamp` is read only when `generated.at` gives no time (§13.1).
    let timestamp = match generated_at {
        Some(t) => Some(t),
        None => map.remove("timestamp").and_then(|v| match parse_time(&v) {
            Some(t) => Some(t),
            None => {
                warn("timestamp", &v);
                map.insert("timestamp".to_string(), v);
                None
            }
        }),
    };

    // `verified`: a list of `{ by, at }` events; a bare mapping is one event (§5.2).
    if let Some(v) = map.remove("verified") {
        let entries = match &v {
            Value::Sequence(seq) => Some(seq.as_slice()),
            Value::Mapping(_) => Some(std::slice::from_ref(&v)),
            _ => None,
        };
        let mut keep = entries.is_none();
        match entries {
            None => warn("verified", &v),
            Some(entries) => {
                for (i, entry) in entries.iter().enumerate() {
                    let event = entry.as_mapping().and_then(|m| {
                        keep |= m.keys().any(|k| !matches!(k.as_str(), Some("by" | "at")));
                        Some(Verification {
                            by: non_blank(m.get("by")?)?,
                            at: parse_time(m.get("at")?)?,
                        })
                    });
                    match event {
                        Some(e) if !provenance.verifications.contains(&e) => {
                            provenance.verifications.push(e)
                        }
                        Some(_) => {}
                        None => {
                            warn(&format!("verified[{i}]"), entry);
                            keep = true;
                        }
                    }
                }
                provenance.trust_tier = if entries.is_empty() {
                    Some(TrustTier::Unverified)
                } else if provenance.verifications.is_empty() {
                    None
                } else if provenance.verifications.iter().any(|e| is_human(&e.by)) {
                    Some(TrustTier::HumanReviewed)
                } else {
                    Some(TrustTier::MachineConfirmed)
                };
            }
        }
        if keep {
            map.insert("verified".to_string(), v);
        }
    }

    if let Some(v) = map.remove("status") {
        match non_blank(&v) {
            Some(s) => provenance.status = Some(s),
            None => {
                warn("status", &v);
                map.insert("status".to_string(), v);
            }
        }
    }

    if let Some(v) = map.remove("stale_after") {
        match parse_time(&v) {
            Some(t) => provenance.stale_after = Some(t),
            None => {
                warn("stale_after", &v);
                map.insert("stale_after".to_string(), v);
            }
        }
    }

    ConceptProvenance {
        timestamp,
        provenance,
    }
}

/// A datetime chrono reads as RFC 3339 (with seconds and an explicit offset), as UTC.
/// Other ISO 8601 forms OKF §5 allows, such as `14:00Z` or a `+09` offset, and a date
/// alone, are `None`.
fn parse_time(v: &Value) -> Option<DateTime<Utc>> {
    let s = v.as_str()?;
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// A string that is not blank, as written.
fn non_blank(v: &Value) -> Option<String> {
    v.as_str()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
}

/// Whether `actor` is a person under the OKF actor convention: `human:` followed by
/// a non-blank id (§7). The prefix is matched exactly.
fn is_human(actor: &str) -> bool {
    actor
        .strip_prefix("human:")
        .is_some_and(|id| !id.trim().is_empty())
}
