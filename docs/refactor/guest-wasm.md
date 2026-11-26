# Rebuilding the sandbox guest wasm

The exec-doc sandbox runs a **Boa** JavaScript engine compiled to
`wasm32-wasip1`. The binary is **checked in prebuilt** at
`core/gecko-engine/src/sandbox/quickjs.wasm` and pulled into the engine with a
single `include_bytes!` (`core/gecko-engine/src/sandbox/wasm_executor.rs`). Its
source is `core/js-sandbox/src/main.rs`.

## The host-call ABI

The guest imports two functions from module `env`. Only the guest calls them.

- **`host_call_extension(ext_ptr, ext_len, func_ptr, func_len, args_ptr, args_len) -> i32`**
  clears any reply left from an earlier call, then works in this order:
  1. It reads the extension and function names (at most 256 bytes each).
  2. It checks the S3 scope `"ext:func"`.
  3. It reads and parses the args JSON (at most 8 MiB).
  4. It runs the extension callback **at most once**.

  An ungranted call is refused as "not granted" whatever its args are. The outcome is held
  host-side, and the return value is its length: `+len` for a JSON reply, `-len` for a UTF-8
  error message. It is never 0, because an empty error gets a message. A reply over 16 MiB
  becomes the error `host response exceeded 16 MiB`, and an error over 16 MiB is truncated.
- **`host_read_reply(out_ptr, out_cap) -> i32`** takes the held outcome, so it can be read once.
  It writes the outcome into the guest buffer and returns its length, or returns:
  - `-1`: nothing is held;
  - `-2`: `out_cap` is too small;
  - `-3`: the write is out of bounds.

  The outcome is cleared in every case.

Both imports trap if the guest exports no linear memory. The guest allocates exactly
`|len|` bytes for each call.

## Run arguments

The guest's program arrives on stdin. Its run arguments arrive in its environment, which
is otherwise empty, as **`GECKO_ARGS`**:

- **Always set** to a JSON object. It is `{}` when `gecko run` is given no `--args`.
- **At most 1 MiB** serialized. The executor refuses a larger value before the guest
  starts.
- **Read once at startup.** A missing, invalid or non-object value fails the run before
  any program code runs.
- **Exposed as `gecko.args`.** It is deep-frozen: every nested object and array is frozen
  too. The property is non-writable and non-configurable.
- **Not gated by an S3 scope.** The arguments come from the operator, not from a host
  capability.

## Why it is prebuilt and out of CI

CI never rebuilds the guest (`--exclude js-sandbox` everywhere) — it consumes the
committed binary as-is. This keeps the per-push pipeline free of a wasm target and
a multi-minute Boa compile. The tradeoff: **after editing the guest source you
must rebuild and re-commit the binary by hand**, or the change never reaches a run.

## How to rebuild

```
just build-guest
```

This runs `cargo build -p js-sandbox --target wasm32-wasip1 --release` and copies
`target/wasm32-wasip1/release/js-sandbox.wasm` over the checked-in
`quickjs.wasm`. The `wasm32-wasip1` target is pinned in `rust-toolchain.toml`
(`just build-guest` also runs `rustup target add wasm32-wasip1` defensively).

Then run the guest-executing tests — they must stay green — and commit the source
change and the regenerated binary **together**:

```
cargo test -p gecko-engine --test wasm_tests --test pipeline_tests --no-default-features
cargo test -p gecko-bin --test sandbox_test --no-default-features   # live TypeDB on :1729
```

## Reproducibility scope

The goal is a **working** guest (the tests above green), not a bit-identical
rebuild. Release wasm is not byte-reproducible without extra toolchain pinning we
deliberately don't carry; a rebuild from unchanged source differs by a few bytes
of build metadata, which is expected and harmless.

## Naming note

The artifact is historically named `quickjs.wasm` but the engine inside is Boa,
not QuickJS. The name is kept to avoid binary/path churn; treat it as "the guest
wasm".
