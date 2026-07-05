use boa_engine::{
    js_string, object::ObjectInitializer, property::Attribute, Context, JsValue, NativeFunction,
    Source,
};
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

    // Register gecko.callExtension
    let call_extension = NativeFunction::from_fn_ptr(|_this, args, context| {
        let ext_name = args
            .first()
            .and_then(boa_engine::JsValue::as_string)
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_default();
        let func_name = args
            .get(1)
            .and_then(boa_engine::JsValue::as_string)
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_default();
        let args_obj = args.get(2).unwrap_or(&JsValue::undefined()).clone();

        let args_json = match args_obj.to_json(context) {
            Ok(json) => serde_json::to_string(&json).unwrap_or_default(),
            Err(_) => "{}".to_string(),
        };

        // Output buffer for host result (1MB limit for now)
        let mut out_buf = vec![0u8; 1024 * 1024];

        let result_len = unsafe {
            host_call_extension(
                ext_name.as_ptr(),
                ext_name.len(),
                func_name.as_ptr(),
                func_name.len(),
                args_json.as_ptr(),
                args_json.len(),
                out_buf.as_mut_ptr(),
                out_buf.len(),
            )
        };

        if result_len < 0 {
            // Read error message from buffer (magnitude is the length)
            let err_len = (-result_len) as usize;
            let err_str = String::from_utf8_lossy(&out_buf[..err_len]).into_owned();
            Err(boa_engine::JsNativeError::error()
                .with_message(format!("Host call failed: {err_str}"))
                .into())
        } else {
            let res_str = std::str::from_utf8(&out_buf[..result_len as usize]).unwrap_or("{}");

            // Parse JSON back into JsValue using Boa's JSON parser
            // Since Boa doesn't have a direct JSON.parse native helper that takes a string easily,
            // we can evaluate it wrapped in parentheses.
            let eval_str = format!("({res_str})");
            let eval_src = Source::from_bytes(eval_str.as_bytes());
            match context.eval(eval_src) {
                Ok(val) => Ok(val),
                Err(_) => Ok(JsValue::undefined()),
            }
        }
    });

    let gecko = ObjectInitializer::new(&mut context)
        .function(call_extension, js_string!("callExtension"), 3)
        .build();

    context
        .register_global_property(js_string!("gecko"), gecko, Attribute::all())
        .expect("Failed to register gecko");

    // Evaluate the code
    let source = Source::from_bytes(code.as_bytes());
    match context.eval(source) {
        Ok(value) => {
            // Attempt to convert the resulting value to JSON string
            // We use JS `JSON.stringify` equivalent via Boa
            match value.to_json(&mut context) {
                Ok(json_val) => {
                    // It returns a serde_json::Value
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
