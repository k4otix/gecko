# Rebuilding the sandbox guest wasm

The exec-doc sandbox runs a **Boa** JavaScript engine compiled to
`wasm32-wasip1`. The binary is **checked in prebuilt** at
`core/gecko-engine/src/sandbox/quickjs.wasm` and pulled into the engine with a
single `include_bytes!` (`core/gecko-engine/src/sandbox/wasm_executor.rs`). Its
source is `core/js-sandbox/src/main.rs`.

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
