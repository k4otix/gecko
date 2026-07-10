use boa_engine::{
    Context, JsNativeError, JsResult, JsValue, NativeFunction, Source, js_string,
    object::ObjectInitializer, property::Attribute,
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
        out_buf_ptr: *mut u8,
        out_buf_capacity: usize,
    ) -> i32;
}

/// Out-buffer capacities tried, in order, for a single host call. On the special
/// overflow sentinel (`-5` with an all-zero buffer) we grow to the next size; if
/// the largest still overflows we surface a clear error.
const HOST_CALL_CAPACITIES: [usize; 3] = [1024 * 1024, 4 * 1024 * 1024, 16 * 1024 * 1024];

/// The shared host-call marshalling seam.
///
/// Serializes `args_json`, invokes the `host_call_extension` FFI import with a
/// grow-and-retry buffer strategy, and interprets the return-value ABI:
///
/// - `ret >= 0` → success; the first `ret` bytes of the out-buffer are the UTF-8
///   JSON reply, parsed and converted to a `JsValue`.
/// - `ret == -5` with an all-zero `buf[..5]` → overflow sentinel: the payload was
///   larger than the buffer and the host wrote nothing. Retry at the next capacity;
///   if the 16 MiB attempt still overflows, throw.
/// - any other `ret < 0` (including a `-5` whose buffer is non-zero, i.e. a genuine
///   5-byte error message) → error; `(-ret)` bytes hold the UTF-8 message.
fn host_call(ext: &str, func: &str, args_json: &Value, context: &mut Context) -> JsResult<JsValue> {
    let args_str = serde_json::to_string(args_json).unwrap_or_else(|_| "{}".to_string());

    for (i, &cap) in HOST_CALL_CAPACITIES.iter().enumerate() {
        // A freshly zeroed buffer each attempt so the overflow sentinel is
        // unambiguous: after a `-5`, an all-zero `buf[..5]` means "host wrote
        // nothing" (overflow), while a non-zero `buf[..5]` is a real 5-byte error.
        let mut out_buf = vec![0u8; cap];

        let ret = unsafe {
            host_call_extension(
                ext.as_ptr(),
                ext.len(),
                func.as_ptr(),
                func.len(),
                args_str.as_ptr(),
                args_str.len(),
                out_buf.as_mut_ptr(),
                out_buf.len(),
            )
        };

        if ret >= 0 {
            let len = ret as usize;
            let value: Value = serde_json::from_slice(&out_buf[..len]).map_err(|e| {
                JsNativeError::error().with_message(format!("host reply was not valid JSON: {e}"))
            })?;
            return JsValue::from_json(&value, context);
        }

        // Overflow sentinel: retry with a bigger buffer, or give up at the top size.
        if ret == -5 && out_buf[..5].iter().all(|&b| b == 0) {
            if i + 1 < HOST_CALL_CAPACITIES.len() {
                continue;
            }
            return Err(JsNativeError::error()
                .with_message("host response exceeded 16 MiB")
                .into());
        }

        // Genuine error: magnitude is the message length (host-truncated to cap).
        let err_len = ((-ret) as usize).min(out_buf.len());
        let err = String::from_utf8_lossy(&out_buf[..err_len]).into_owned();
        return Err(JsNativeError::error()
            .with_message(format!("Host call failed: {err}"))
            .into());
    }

    // Unreachable: the loop returns on every branch.
    Err(JsNativeError::error()
        .with_message("host call exhausted all buffer capacities")
        .into())
}

/// Reads a required string positional arg, throwing a JS error naming it if the
/// arg is absent or not a string — a fail-fast on the guest side (the host also
/// validates).
fn require_str_arg(args: &[JsValue], idx: usize, name: &str) -> JsResult<String> {
    args.get(idx)
        .and_then(JsValue::as_string)
        .map(|s| s.to_std_string_escaped())
        .ok_or_else(|| {
            JsNativeError::typ()
                .with_message(format!("mem: missing required string argument '{name}'"))
                .into()
        })
}

/// Converts an optional positional arg to a `serde_json::Value`, treating
/// `undefined`/`null`/absent (and any conversion failure) as "not supplied".
fn opt_arg_json(args: &[JsValue], idx: usize, context: &mut Context) -> Option<Value> {
    match args.get(idx) {
        Some(v) if !v.is_undefined() && !v.is_null() => v.to_json(context).ok().flatten(),
        _ => None,
    }
}

// ── mem.* namespace ──────────────────────────────────────────────────────────
//
// Each guest method builds the exact snake_case payload the host parses (see
// `dispatch_mem_call` / `dispatch_mem_recall` in the engine's `mem_host.rs`),
// omitting a key when its arg is absent, and delegates to `host_call`. The host
// strips and stamps provenance (`run_id`/`actor`/`source`/`occurred_at`) itself,
// so the guest never sends those.

/// `mem.recall(query, budget?)` → `{ text, max_chunks?, max_tokens?, as_of? }`.
/// Unwraps the host's `{ chunks }` envelope, returning the array directly.
fn mem_recall(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let query = require_str_arg(args, 0, "query")?;
    let mut payload = serde_json::Map::new();
    payload.insert("text".to_string(), json!(query));

    if let Some(budget) = opt_arg_json(args, 1, context) {
        if let Some(n) = budget.get("maxChunks").and_then(Value::as_f64) {
            payload.insert("max_chunks".to_string(), json!(n as u64));
        }
        if let Some(n) = budget.get("maxTokens").and_then(Value::as_f64) {
            payload.insert("max_tokens".to_string(), json!(n as u64));
        }
        if let Some(s) = budget.get("asOf").and_then(Value::as_str) {
            payload.insert("as_of".to_string(), json!(s));
        }
    }

    let reply = host_call("mem", "recall", &Value::Object(payload), context)?;

    // Unwrap `{ chunks: [...] }` → the array (empty array if the key is absent).
    if let Some(obj) = reply.as_object() {
        let chunks = obj.get(js_string!("chunks"), context)?;
        if !chunks.is_undefined() && !chunks.is_null() {
            return Ok(chunks);
        }
    }
    JsValue::from_json(&json!([]), context)
}

/// `mem.remember(text, opts?)` → `{ text, event_time? }`.
fn mem_remember(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let text = require_str_arg(args, 0, "text")?;
    let mut payload = serde_json::Map::new();
    payload.insert("text".to_string(), json!(text));

    if let Some(opts) = opt_arg_json(args, 1, context)
        && let Some(s) = opts.get("eventTime").and_then(Value::as_str)
    {
        payload.insert("event_time".to_string(), json!(s));
    }

    host_call("mem", "remember", &Value::Object(payload), context)
}

/// `mem.derive(text, evidence?, opts?)` → `{ text, evidence?, method?, confidence? }`.
fn mem_derive(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let text = require_str_arg(args, 0, "text")?;
    let mut payload = serde_json::Map::new();
    payload.insert("text".to_string(), json!(text));

    if let Some(evidence) = opt_arg_json(args, 1, context)
        && evidence.is_array()
    {
        payload.insert("evidence".to_string(), evidence);
    }

    if let Some(opts) = opt_arg_json(args, 2, context) {
        if let Some(s) = opts.get("method").and_then(Value::as_str) {
            payload.insert("method".to_string(), json!(s));
        }
        if let Some(n) = opts.get("confidence").and_then(Value::as_f64) {
            payload.insert("confidence".to_string(), json!(n));
        }
    }

    host_call("mem", "derive", &Value::Object(payload), context)
}

/// `mem.supersede(old, text, reason?, opts?)` → `{ old, text, reason?, confidence? }`.
fn mem_supersede(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let old = require_str_arg(args, 0, "old")?;
    let text = require_str_arg(args, 1, "text")?;
    let mut payload = serde_json::Map::new();
    payload.insert("old".to_string(), json!(old));
    payload.insert("text".to_string(), json!(text));

    if let Some(reason) = args.get(2).and_then(JsValue::as_string) {
        payload.insert("reason".to_string(), json!(reason.to_std_string_escaped()));
    }

    // Optional `{ confidence }` — the host's supersede arm reads it, mirroring derive.
    if let Some(opts) = opt_arg_json(args, 3, context)
        && let Some(n) = opts.get("confidence").and_then(Value::as_f64)
    {
        payload.insert("confidence".to_string(), json!(n));
    }

    host_call("mem", "supersede", &Value::Object(payload), context)
}

/// `mem.contest(claims)` → `{ claims }` (array of id strings; empty if absent).
fn mem_contest(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let claims = match opt_arg_json(args, 0, context) {
        Some(v) if v.is_array() => v,
        _ => json!([]),
    };
    let mut payload = serde_json::Map::new();
    payload.insert("claims".to_string(), claims);

    host_call("mem", "contest", &Value::Object(payload), context)
}

/// `gecko.callExtension(ext, func, argsObj)` — the generic host-call surface,
/// delegating to the shared `host_call` helper.
fn gecko_call_extension(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let ext_name = args
        .first()
        .and_then(JsValue::as_string)
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_default();
    let func_name = args
        .get(1)
        .and_then(JsValue::as_string)
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_default();
    let args_json = args
        .get(2)
        .and_then(|v| v.to_json(context).ok().flatten())
        .unwrap_or_else(|| json!({}));

    host_call(&ext_name, &func_name, &args_json, context)
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

    // Register the `gecko` global (generic host-call surface).
    let gecko = ObjectInitializer::new(&mut context)
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
                    // Print JSON directly, gecko engine parses this as the output object
                    // but we should wrap it so it matches ExecutionResult format if needed,
                    // or just return the raw evaluation
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
