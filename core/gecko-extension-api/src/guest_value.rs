//! Reading values a sandboxed guest sends to a host bridge.
//!
//! Every bridge reads guest JSON the same way, so a playbook sees one rule for what
//! counts as an integer and one shape for a refusal, whichever extension it calls.

use serde_json::Value;

/// The largest integer a JS number represents exactly (`Number.MAX_SAFE_INTEGER`).
pub const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// The longest serialized value a refusal quotes; a longer one is named by type.
const MAX_SHOWN_VALUE_LEN: usize = 64;

/// A non-negative whole number from the guest. A JSON integer is accepted at any
/// `u64` value; a float (`5.0`, as `Math.floor` returns) only when it is whole and
/// within `0..=MAX_SAFE_INTEGER`, so a huge float never saturates.
pub fn whole_number(v: &Value) -> Option<u64> {
    match (v.as_u64(), v.as_f64()) {
        (Some(n), _) => Some(n),
        (None, Some(f))
            if f.is_finite() && f.fract() == 0.0 && (0.0..=MAX_SAFE_INTEGER).contains(&f) =>
        {
            Some(f as u64)
        }
        _ => None,
    }
}

/// A guest value as a refusal shows it: the JSON text when short, otherwise its type
/// and size, so an error never echoes a large payload back.
pub fn shown(v: &Value) -> String {
    let text = v.to_string();
    if text.len() <= MAX_SHOWN_VALUE_LEN {
        return text;
    }
    let kind = match v {
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
        _ => "a value",
    };
    format!("{kind} of {} bytes", text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn whole_number_accepts_integers_and_whole_floats() {
        assert_eq!(whole_number(&json!(5)), Some(5));
        assert_eq!(whole_number(&json!(5.0)), Some(5));
        assert_eq!(whole_number(&json!(-0.0)), Some(0));
        assert_eq!(
            whole_number(&json!(MAX_SAFE_INTEGER)),
            Some(9_007_199_254_740_991)
        );
        assert_eq!(whole_number(&json!(u64::MAX)), Some(u64::MAX));
    }

    #[test]
    fn whole_number_refuses_everything_else() {
        for v in [
            json!(5.5),
            json!(-1),
            json!(-5.0),
            json!(1e20),
            json!(MAX_SAFE_INTEGER + 2.0),
            json!("5"),
            json!(null),
            json!(true),
            json!([5]),
        ] {
            assert_eq!(whole_number(&v), None, "{v}");
        }
    }

    #[test]
    fn shown_quotes_a_short_value() {
        assert_eq!(shown(&json!("0.5")), "\"0.5\"");
    }

    #[test]
    fn shown_names_a_long_value_by_type_and_size() {
        assert_eq!(shown(&json!("x".repeat(100))), "a string of 102 bytes");
    }
}
