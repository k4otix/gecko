//! Schema-label validation for labels interpolated into query text.

use crate::db::router::DbError;

/// Validates a TypeDB type or attribute label before it is interpolated into
/// query text. Values always flow through parameterized `given` stages, but a
/// schema *label* (an entity subtype from a concept's `type_hint`, or a typed
/// attribute name) is part of the query grammar and cannot be parameterized, so
/// an extension-supplied label must be constrained to the TypeQL identifier
/// charset first — otherwise a crafted label would be an injection vector.
pub fn validate_schema_label(label: &str) -> Result<(), DbError> {
    let valid = !label.is_empty()
        && !label.starts_with(|c: char| c.is_ascii_digit())
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid {
        Ok(())
    } else {
        Err(DbError::Schema(format!(
            "invalid schema label {label:?}: must be a non-empty TypeQL identifier \
             (ASCII alphanumeric, '-' or '_', no leading digit)"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_schema_label_accepts_identifiers_and_refuses_the_rest() {
        for ok in ["memory-item", "attack_pattern", "t1"] {
            assert!(validate_schema_label(ok).is_ok(), "{ok:?} must be accepted");
        }
        for bad in ["", "1abc", "belief;", "a b", "x\""] {
            assert!(
                matches!(validate_schema_label(bad), Err(DbError::Schema(_))),
                "{bad:?} must be refused with a schema error"
            );
        }
    }
}
