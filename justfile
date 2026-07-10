# GECKO — task interface.  Run `just` with no arguments to list recipes.
#
# Philosophy: fast & hermetic by DEFAULT; opt IN to the live 1.3GB model.
#   just test        → Tier 1: stub model, no candle, no network        (every-commit speed)
#   just test-it     → Tier 2: real TypeDB, STILL stub model
#   just test-model  → Tier 3: real 1.3GB model  (OPT-IN; first run fetches it)
#
# All `gecko` subcommands run via `cargo run` so the repo is self-sufficient —
# no install step needed for development.

set shell := ["bash", "-uc"]

# gecko invoked from source (stub build is fine for up/sync/doctor/model-fetch — none load the model)
gecko := "cargo run --quiet --no-default-features --bin gecko --"

# default: show the recipe list
default:
    @just --list --unsorted

# ---------- build ----------

# fast dev build: stub embedder, candle NOT compiled
build:
    cargo build --no-default-features

# shipped binary: working (candle) embedder
build-release:
    cargo build --release --features real-embedder

# ---------- test tiers ----------

# Tier 1 — fast, serviceless, stub embedder. No candle, no model, no network.
test:
    cargo test --lib --bins --no-default-features

# Tier 2 — integration against a real TypeDB, but STILL the stub embedder (no 1.3GB).
test-it: up
    cargo test --test '*' --no-default-features -- --test-threads=1

# Tier 3 — OPT-IN. Real bge-large model (first run downloads ~1.3GB) + real TypeDB.
test-model: up
    cargo test --features integration-model -- --test-threads=1

# everything (slow; pulls the model)
test-all: test test-it test-model

# ---------- run / orchestrate ----------

# start the orchestrated TypeDB (idempotent: no-op if already running)
up:
    {{gecko}} up

# stop the orchestrated TypeDB
down:
    {{gecko}} down

# ingest a bundle (defaults to the sample)
sync bundle="sample-bundle":
    {{gecko}} sync {{bundle}}

# ---------- setup / health ----------

# first-run bootstrap (prereq checks + TypeDB-mode choice + delegate to subcommands)
setup:
    ./scripts/setup.sh

# write a default gecko.toml if absent (never clobbers)
init:
    {{gecko}} init

# report every precondition ✓/⚠/✗ with the fix; non-zero exit iff a hard check fails
doctor:
    {{gecko}} doctor

# pre-stage the embedding model into the cache (one-time ~1.3GB)
fetch-model:
    {{gecko}} model fetch

# ---------- hygiene ----------

fmt:
    cargo fmt

lint:
    cargo clippy --no-default-features -- -D warnings

# pre-push gate: format, lint, fast tests
check: fmt lint test
