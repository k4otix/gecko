# Installing GECKO

`gecko` ships as a small (tens-of-MB) per-platform binary via [GitHub
Releases](https://github.com/k4otix/gecko/releases). **Neither the ~1.3GB
embedding model nor TypeDB is ever bundled in that download** — both are
runtime assets that get staged into a local cache on first use. This keeps the
install fast; the two heavy pieces are opt-in and explicit.

## 1. Install the binary

```bash
curl -fsSL https://raw.githubusercontent.com/k4otix/gecko/main/scripts/install.sh | sh
```

This detects your OS/arch, downloads the matching archive from the latest
GitHub Release (`aarch64-apple-darwin`, `x86_64-apple-darwin`, or
`x86_64-unknown-linux-gnu`), and installs the `gecko` binary to `~/.local/bin`
(falling back to `/usr/local/bin` if that isn't writable/creatable).

Alternatives:

- **Homebrew**: a formula template lives at `HomebrewFormula/gecko.rb` in this
  repo. A real tap (`k4otix/homebrew-gecko`) is a separate repository that
  publishes a filled-in copy of that template per release — see the
  instructions at the top of the file. Once a tap exists: `brew install
  k4otix/gecko/gecko`.
- **From source**: `cargo install --path gecko-bin` (see the root
  [README.md](../README.md#building--installation)). This compiles candle
  (`real-embedder` is default-on) but, like the release binary, never
  downloads the model at build time.
- **Manual**: download the archive for your platform directly from the
  [Releases page](https://github.com/k4otix/gecko/releases), extract it, and
  place the `gecko` binary on your `PATH`.

## 2. First run (local / online)

Once `gecko` is on your `PATH`:

```bash
# 1. (optional) write a default gecko.toml in the current directory
gecko init

# 2. One-time download of the embedding model (~1.3GB). Explicit and
#    opt-in — nothing above silently fetched it.
gecko model fetch

# 3. Bring up TypeDB. In the default `orchestrated` mode this downloads the
#    PINNED TypeDB server into the local cache (once) and runs it as a
#    managed child process — no separate install, no Docker required.
gecko up

# 4. Sync a knowledge bundle and confirm everything works end to end.
gecko sync sample-bundle
```

`gecko down` stops the managed TypeDB child when you're done. Both the model
and the TypeDB server live under the GECKO cache root (`~/.cache/gecko` by
default; override with `GECKO_CACHE_DIR` or `[cache_dir]` in `gecko.toml`) —
never inside the binary or the repo.

## 3. Air-gapped / enterprise install (no runtime downloads)

For environments without outbound network access, stage both heavy assets
ahead of time on a machine that does have access, copy them over, and point
`gecko.toml` at the staged copies instead of letting it fetch anything:

```toml
[semantic_index]
enabled    = true
embedder   = "bge-large-en-v1.5"
model_id   = "bge-large-en-v1.5@<pinned-revision>"
auto_fetch = false                              # default — never a silent pull
model_path = "/opt/gecko/models/bge-large-en-v1.5"  # pre-provisioned model dir
                                                     # (config.json, tokenizer.json,
                                                     # model.safetensors)

[typedb]
mode     = "external"       # gecko connects only; it never manages the process
endpoint = "typedb.internal.example:1729"  # a TypeDB server you run/manage yourself
```

Steps:

1. On a connected machine, run `gecko model fetch` (or `gecko model fetch
   --path <dir>`) to stage the model, then copy that directory to the
   air-gapped host (or to shared storage it can read) and set `model_path`
   above to point at it.
2. Stand up TypeDB yourself on/near the air-gapped host (a manually installed
   TypeDB server, or `docker compose up -d` with the image pre-pulled/mirrored
   internally) and set `[typedb] mode = "external"` with `endpoint` pointing
   at it. `gecko` will connect to it but will never try to download or manage
   a TypeDB process in this mode.
3. Run `gecko schema-init` and `gecko sync <bundle>` as usual — no network
   access is required at this point, since both `model_path` and `endpoint`
   are pre-provisioned.

`auto_fetch = false` is the config default everywhere (not just air-gapped
setups): GECKO never silently pulls the 1.3GB model. It is only ever fetched
via the explicit `gecko model fetch` command, or (if you set `auto_fetch =
true`) lazily on first embed — a choice you opt into, not a default.

## Reference: what's bundled vs. what's runtime

| Artifact | In the release binary? | How it gets there |
|---|---|---|
| `gecko` executable (incl. compiled-in candle embedder) | Yes | Built by `.github/workflows/release.yml` (`cargo build --release --features real-embedder --bin gecko`) |
| Embedding model weights (~1.3GB) | **Never** | `gecko model fetch`, or lazily on first embed if `auto_fetch = true` |
| TypeDB server | **Never** | `gecko up` in `orchestrated` mode (downloads the pinned version once); or run your own and use `external`/`compose` mode |
