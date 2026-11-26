use boa_engine::{
    Context, JsNativeError, JsObject, JsResult, JsValue, NativeFunction, Source, js_string,
    object::{IntegrityLevel, ObjectInitializer},
    property::{Attribute, PropertyKey},
};
use serde_json::{Value, json};
use std::io::{self, Read};

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn host_call_extension(
        ext_name_ptr: *const u8,
        ext_name_len: usize,
        func_name_ptr: *const u8,
        func_name_len: usize,
        args_json_ptr: *const u8,
        args_json_len: usize,
    ) -> i32;

    fn host_read_reply(out_buf_ptr: *mut u8, out_buf_capacity: usize) -> i32;
}

fn type_error(message: String) -> boa_engine::JsError {
    JsNativeError::typ().with_message(message).into()
}

/// The shared host-call marshalling seam.
///
/// Serializes `args_json` and calls `host_call_extension`, which runs the call once
/// and returns the length of its outcome: `+len` for a JSON reply, `-len` for an
/// error message. The guest then allocates exactly that many bytes and reads the
/// outcome with `host_read_reply`.
fn host_call(ext: &str, func: &str, args_json: &Value) -> JsResult<Value> {
    let args_str = serde_json::to_string(args_json).map_err(|e| {
        JsNativeError::error().with_message(format!("failed to serialize host call args: {e}"))
    })?;

    let ret = unsafe {
        host_call_extension(
            ext.as_ptr(),
            ext.len(),
            func.as_ptr(),
            func.len(),
            args_str.as_ptr(),
            args_str.len(),
        )
    };
    if ret == 0 {
        return Err(JsNativeError::error()
            .with_message("host call returned no reply")
            .into());
    }

    let mut out_buf = vec![0u8; ret.unsigned_abs() as usize];
    let read = unsafe { host_read_reply(out_buf.as_mut_ptr(), out_buf.len()) };
    if read < 0 {
        return Err(JsNativeError::error()
            .with_message(format!("host reply could not be read (code {read})"))
            .into());
    }
    out_buf.truncate(read as usize);

    if ret < 0 {
        let err = String::from_utf8_lossy(&out_buf).into_owned();
        return Err(JsNativeError::error()
            .with_message(format!("Host call failed: {err}"))
            .into());
    }
    serde_json::from_slice(&out_buf).map_err(|e| {
        JsNativeError::error()
            .with_message(format!("host reply was not valid JSON: {e}"))
            .into()
    })
}

/// Reads a required string positional arg, throwing a JS error naming it if the
/// arg is absent or not a string — a fail-fast on the guest side (the host also
/// validates).
fn require_str_arg(args: &[JsValue], idx: usize, name: &str) -> JsResult<String> {
    args.get(idx)
        .and_then(JsValue::as_string)
        .map(|s| s.to_std_string_escaped())
        .ok_or_else(|| type_error(format!("mem: missing required string argument '{name}'")))
}

/// Converts a guest value to its JSON wire form: `undefined` is absent (`None`),
/// and a non-finite number, a function, or a value `to_json` cannot convert or
/// gives no JSON for (a `Symbol`, a cyclic object, a `BigInt`) is a `TypeError`
/// naming `name`. Anything else is forwarded as JSON unchanged; the host judges
/// its type.
fn to_wire(v: &JsValue, name: &str, context: &mut Context) -> JsResult<Option<Value>> {
    if v.is_undefined() {
        return Ok(None);
    }
    if v.as_number().is_some_and(|n| !n.is_finite()) {
        return Err(type_error(format!("'{name}' must be a finite number")));
    }
    if v.is_callable() {
        return Err(type_error(format!("'{name}' must not be a function")));
    }
    match v.to_json(context) {
        Ok(Some(json)) => Ok(Some(json)),
        Ok(None) => Err(type_error(format!("'{name}' has no JSON form"))),
        Err(e) => Err(type_error(format!(
            "'{name}' cannot be converted to JSON: {e}"
        ))),
    }
}

/// An optional positional arg in wire form: absent and `undefined` are `None`.
fn opt_arg(
    args: &[JsValue],
    idx: usize,
    name: &str,
    context: &mut Context,
) -> JsResult<Option<Value>> {
    match args.get(idx) {
        Some(v) => to_wire(v, name, context),
        None => Ok(None),
    }
}

/// Reads the options bag at `args[idx]` into `payload`. The bag must be absent,
/// `undefined`, `null` or an object; its own keys must each appear in `allowed`
/// (`(guestKey, wire_key)` pairs), checked before any value is read; and each
/// value goes through [`to_wire`], an `undefined` value being omitted.
fn read_bag(
    args: &[JsValue],
    idx: usize,
    param: &str,
    allowed: &[(&str, &str)],
    payload: &mut serde_json::Map<String, Value>,
    context: &mut Context,
) -> JsResult<()> {
    let Some(bag) = args.get(idx).filter(|v| !v.is_undefined() && !v.is_null()) else {
        return Ok(());
    };
    let obj = match bag.as_object() {
        Some(obj) if !bag.is_callable() && !obj.is_array() => obj,
        _ => return Err(type_error(format!("'{param}' must be an object"))),
    };
    let accepted = || {
        allowed
            .iter()
            .map(|(k, _)| *k)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut keys = Vec::new();
    for key in obj.own_property_keys(context)? {
        let name = match &key {
            PropertyKey::String(s) => s.to_std_string_escaped(),
            PropertyKey::Index(i) => i.get().to_string(),
            PropertyKey::Symbol(_) => continue,
        };
        let Some(&(_, wire)) = allowed.iter().find(|(k, _)| *k == name) else {
            return Err(type_error(format!(
                "'{param}' does not accept '{name}'; it accepts {}",
                accepted()
            )));
        };
        keys.push((key, name, wire));
    }
    for (key, name, wire) in keys {
        let value = obj.get(key, context)?;
        if let Some(json) = to_wire(&value, &name, context)? {
            payload.insert(wire.to_string(), json);
        }
    }
    Ok(())
}

// ── mem.* namespace ──────────────────────────────────────────────────────────
//
// Each guest method checks argument shape, renames its options to the snake_case
// wire keys `dispatch_mem_call` / `dispatch_mem_recall` parse in the engine's
// `mem_host.rs`, and forwards every value unconverted: the host is the single
// authority on types and ranges. A key is omitted when its value is absent. The
// host strips and stamps provenance (`run_id`/`actor`/`source`/`occurred_at`)
// itself, so the guest never sends those.

/// `mem.recall(query, budget?)` → `{ text, max_chunks?, as_of? }`.
/// Unwraps the host's `{ chunks }` envelope, returning the array directly.
fn mem_recall(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let query = require_str_arg(args, 0, "query")?;
    let mut payload = serde_json::Map::new();
    payload.insert("text".to_string(), json!(query));
    read_bag(
        args,
        1,
        "budget",
        &[("maxChunks", "max_chunks"), ("asOf", "as_of")],
        &mut payload,
        context,
    )?;

    let reply = host_call("mem", "recall", &Value::Object(payload))?;
    match reply.get("chunks") {
        Some(chunks @ Value::Array(_)) => JsValue::from_json(chunks, context),
        _ => Err(JsNativeError::error()
            .with_message("mem.recall reply carries no chunks array")
            .into()),
    }
}

/// `mem.remember(text, opts?)` → `{ text, event_time? }`.
fn mem_remember(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let text = require_str_arg(args, 0, "text")?;
    let mut payload = serde_json::Map::new();
    payload.insert("text".to_string(), json!(text));
    read_bag(
        args,
        1,
        "opts",
        &[("eventTime", "event_time")],
        &mut payload,
        context,
    )?;

    let reply = host_call("mem", "remember", &Value::Object(payload))?;
    JsValue::from_json(&reply, context)
}

/// `mem.derive(text, evidence?, opts?)` → `{ text, evidence?, method?, confidence? }`.
fn mem_derive(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let text = require_str_arg(args, 0, "text")?;
    let mut payload = serde_json::Map::new();
    payload.insert("text".to_string(), json!(text));
    if let Some(evidence) = opt_arg(args, 1, "evidence", context)? {
        payload.insert("evidence".to_string(), evidence);
    }
    read_bag(
        args,
        2,
        "opts",
        &[("method", "method"), ("confidence", "confidence")],
        &mut payload,
        context,
    )?;

    let reply = host_call("mem", "derive", &Value::Object(payload))?;
    JsValue::from_json(&reply, context)
}

/// `mem.supersede(old, text, reason?, opts?)` → `{ old, text, reason?, confidence? }`.
fn mem_supersede(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let old = require_str_arg(args, 0, "old")?;
    let text = require_str_arg(args, 1, "text")?;
    let mut payload = serde_json::Map::new();
    payload.insert("old".to_string(), json!(old));
    payload.insert("text".to_string(), json!(text));
    if let Some(reason) = opt_arg(args, 2, "reason", context)? {
        payload.insert("reason".to_string(), reason);
    }
    read_bag(
        args,
        3,
        "opts",
        &[("confidence", "confidence")],
        &mut payload,
        context,
    )?;

    let reply = host_call("mem", "supersede", &Value::Object(payload))?;
    JsValue::from_json(&reply, context)
}

/// `mem.retract(id, reason?)` → `{ id, reason? }`. The reply is
/// `{ id, blast_radius }`.
fn mem_retract(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = require_str_arg(args, 0, "id")?;
    let mut payload = serde_json::Map::new();
    payload.insert("id".to_string(), json!(id));
    if let Some(reason) = opt_arg(args, 1, "reason", context)? {
        payload.insert("reason".to_string(), reason);
    }
    let reply = host_call("mem", "retract", &Value::Object(payload))?;
    JsValue::from_json(&reply, context)
}

/// `mem.contest(claims)` → `{ claims }`. A missing `claims` is sent as missing, and
/// the host refuses it.
fn mem_contest(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let mut payload = serde_json::Map::new();
    if let Some(claims) = opt_arg(args, 0, "claims", context)? {
        payload.insert("claims".to_string(), claims);
    }

    let reply = host_call("mem", "contest", &Value::Object(payload))?;
    JsValue::from_json(&reply, context)
}

/// `gecko.callExtension(ext, func, args?)` — the generic host-call surface. `ext`
/// and `func` must be strings; an absent, `undefined` or `null` `args` is sent as
/// `{}`, and any other `args` goes through [`to_wire`].
fn gecko_call_extension(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let name = |idx: usize, what: &str| {
        args.get(idx)
            .and_then(JsValue::as_string)
            .map(|s| s.to_std_string_escaped())
            .ok_or_else(|| type_error(format!("gecko.callExtension: '{what}' must be a string")))
    };
    let ext_name = name(0, "ext")?;
    let func_name = name(1, "func")?;
    let args_json = match opt_arg(args, 2, "args", context)? {
        None | Some(Value::Null) => json!({}),
        Some(v) => v,
    };

    let reply = host_call(&ext_name, &func_name, &args_json)?;
    JsValue::from_json(&reply, context)
}

/// Reads the run arguments from `GECKO_ARGS`, which the host always sets to a JSON
/// object.
fn read_run_args() -> Result<Value, String> {
    let raw = std::env::var("GECKO_ARGS").map_err(|e| format!("GECKO_ARGS is unreadable: {e}"))?;
    match serde_json::from_str::<Value>(&raw) {
        Ok(args @ Value::Object(_)) => Ok(args),
        Ok(_) => Err("GECKO_ARGS is not a JSON object".to_string()),
        Err(e) => Err(format!("GECKO_ARGS is not valid JSON: {e}")),
    }
}

/// Freezes `obj` and every object reachable through its own property values,
/// innermost first.
fn deep_freeze(obj: &JsObject, context: &mut Context) -> JsResult<()> {
    for key in obj.own_property_keys(context)? {
        if let Some(child) = obj.get(key, context)?.as_object() {
            deep_freeze(&child, context)?;
        }
    }
    if obj.set_integrity_level(IntegrityLevel::Frozen, context)? {
        Ok(())
    } else {
        Err(type_error("gecko.args could not be frozen".to_string()))
    }
}

/// The run arguments as a deep-frozen guest value.
fn frozen_run_args(args: &Value, context: &mut Context) -> JsResult<JsValue> {
    let value = JsValue::from_json(args, context)?;
    if let Some(obj) = value.as_object() {
        deep_freeze(&obj, context)?;
    }
    Ok(value)
}

#[allow(clippy::too_many_lines)]
fn main() {
    // Read the JavaScript code from standard input
    let mut code = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut code) {
        print_json_result(false, format!("Failed to read stdin: {e}"));
        return;
    }

    if code.is_empty() {
        print_json_result(false, "No code provided".to_string());
        return;
    }

    let run_args = match read_run_args() {
        Ok(args) => args,
        Err(e) => {
            print_json_result(false, e);
            return;
        }
    };

    // Initialize the Boa JavaScript context
    let mut context = Context::default();

    // Register a simple console.log polyfill
    let console_log = NativeFunction::from_fn_ptr(|_this, args, context| {
        let mut msg = String::new();
        for arg in args {
            if !msg.is_empty() {
                msg.push(' ');
            }
            if let Ok(s) = arg.to_string(context) {
                msg.push_str(&s.to_std_string_escaped());
            }
        }
        // Print to stderr so we don't corrupt JSON output on stdout
        eprintln!("{msg}");
        Ok(JsValue::undefined())
    });

    let console = ObjectInitializer::new(&mut context)
        .function(console_log, js_string!("log"), 1)
        .build();

    context
        .register_global_property(js_string!("console"), console, Attribute::all())
        .expect("Failed to register console");

    // Register the `gecko` global: the generic host-call surface and the run's
    // arguments, which a program can neither replace nor change.
    let args = match frozen_run_args(&run_args, &mut context) {
        Ok(args) => args,
        Err(e) => {
            print_json_result(false, format!("Failed to expose gecko.args: {e}"));
            return;
        }
    };
    let gecko = ObjectInitializer::new(&mut context)
        .property(
            js_string!("args"),
            args,
            Attribute::READONLY | Attribute::ENUMERABLE,
        )
        .function(
            NativeFunction::from_fn_ptr(gecko_call_extension),
            js_string!("callExtension"),
            3,
        )
        .build();

    context
        .register_global_property(js_string!("gecko"), gecko, Attribute::all())
        .expect("Failed to register gecko");

    // Register the `mem` global (ergonomic epistemic-memory surface).
    let mem = ObjectInitializer::new(&mut context)
        .function(
            NativeFunction::from_fn_ptr(mem_recall),
            js_string!("recall"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(mem_remember),
            js_string!("remember"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(mem_derive),
            js_string!("derive"),
            3,
        )
        .function(
            NativeFunction::from_fn_ptr(mem_supersede),
            js_string!("supersede"),
            4,
        )
        .function(
            NativeFunction::from_fn_ptr(mem_contest),
            js_string!("contest"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(mem_retract),
            js_string!("retract"),
            2,
        )
        .build();

    context
        .register_global_property(js_string!("mem"), mem, Attribute::all())
        .expect("Failed to register mem");

    // Evaluate the code
    let source = Source::from_bytes(code.as_bytes());
    match context.eval(source) {
        Ok(value) => {
            // Attempt to convert the resulting value to JSON string
            // We use JS `JSON.stringify` equivalent via Boa
            match value.to_json(&mut context) {
                Ok(json_val) => {
                    // It returns an Option<serde_json::Value>
                    let result_str =
                        serde_json::to_string(&json_val).unwrap_or_else(|_| "null".to_string());
                    // The host parses this line as the run's output.
                    println!("{result_str}");
                }
                Err(e) => {
                    print_json_result(false, format!("Failed to stringify result: {e}"));
                }
            }
        }
        Err(e) => {
            let err_str = e.to_string();
            print_json_result(false, format!("JS Execution Error: {err_str}"));
        }
    }
}

fn print_json_result(success: bool, message: String) {
    let result = serde_json::json!({
        "success": success,
        "error": message,
    });
    println!("{result}");
}
