//! OKF provenance, trust and lifecycle frontmatter (OKF §5.2–§5.5).
//!
//! A malformed value is never refused (OKF §11 treats these constraints as soft
//! guidance) and never replaced by an invented one: its key stays in the frontmatter
//! map, so it lands in `metadata-json` as written, and a warning names it. A key that
//! holds a subkey OKF does not define also stays in the map, with no warning, so the
//! producer's extension is preserved (§4.1).

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde_yaml::Value;

use super::parser::yaml_as_written;
use super::types::{OkfProvenance, TrustTier, Verification};

/// The `stale-after` stored for a `stale_after` that cannot be read: an instant before
/// every real `now`, so the concept is stale at every read (OKF §5.5).
pub const STALE_SENTINEL: DateTime<Utc> = DateTime::UNIX_EPOCH;

/// What a concept's provenance keys give: the content time (`generated.at`, or the
/// legacy `timestamp`) and the typed provenance facts.
pub(crate) struct ConceptProvenance {
    pub timestamp: Option<DateTime<Utc>>,
    pub provenance: OkfProvenance,
}

/// Takes the OKF provenance keys out of `map`, leaving behind every key that
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
            if let Some(at) = m.get("at").filter(|at| !is_empty_value(at)) {
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

    // The v0.1 `timestamp` is read only when `generated.at` gives no time (Appendix B).
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
                // Only current events count: those at or after the content time, or
                // all of them when there is none (§5.3). Every valid event is stored.
                let mut current = provenance
                    .verifications
                    .iter()
                    .filter(|e| timestamp.is_none_or(|t| e.at >= t))
                    .peekable();
                provenance.trust_tier = if current.peek().is_none() {
                    None
                } else if current.any(|e| is_human(&e.by)) {
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

    // An unreadable `stale_after` is never fresh (§5.5): it is stored as stale.
    if let Some(v) = map.remove("stale_after") {
        match parse_time(&v) {
            Some(t) => provenance.stale_after = Some(t),
            None => {
                warnings.push(format!(
                    "Concept file '{path}' has an invalid 'stale_after' ({}); it is kept in \
                     metadata-json and stored as stale (stale-after {})",
                    yaml_as_written(&v),
                    STALE_SENTINEL.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                ));
                provenance.stale_after = Some(STALE_SENTINEL);
                map.insert("stale_after".to_string(), v);
            }
        }
    }

    ConceptProvenance {
        timestamp,
        provenance,
    }
}

/// A timestamp in one of OKF §5's two forms, as UTC: an RFC 3339 `date-time` (seconds
/// and an offset required), or a `full-date`, which means the start of that day in UTC.
fn parse_time(v: &Value) -> Option<DateTime<Utc>> {
    let s = v.as_str()?;
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&Utc));
    }
    let b = s.as_bytes();
    let full_date = b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        });
    if !full_date {
        return None;
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .map(|d| d.and_time(NaiveTime::MIN).and_utc())
}

/// Whether `v` is null, an empty string or an empty list, which OKF §11.2 reads as an
/// absent key.
pub(crate) fn is_empty_value(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Sequence(seq) => seq.is_empty(),
        _ => false,
    }
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
