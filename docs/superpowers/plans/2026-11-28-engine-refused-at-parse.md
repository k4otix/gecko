# An unrunnable playbook is refused at parse — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A concept whose `engine` is present but unsupported, or which declares a supported engine but has no non-blank code in a matching fence, fails `parse_bundle`, naming the file (N10).

**Architecture:** The parser matches `engine` as a YAML string and refuses anything `ScriptEngine::from_lang` rejects (`ParseError::InvalidEngine`). After `extract_program`, which drops blank fence bodies, a declared engine with no program is refused (`ParseError::MissingProgram`). Both errors quote values through one shared helper, and name the accepted spellings from one constant.

**Tech Stack:** Rust 2024, serde_yaml, thiserror, cargo nextest; TypeDB 3.15 for the CLI test only.

**Spec:** `docs/superpowers/specs/2026-11-28-engine-refused-at-parse-design.md`

## Global Constraints

- **Exact messages:**
  - `Concept file '{path}' has an invalid 'engine' ({value}); it must be one of js, javascript, quickjs`;
  - `Concept file '{path}' declares engine '{engine}' but has no code in a js, javascript or quickjs fence`.
- **The accepted spellings** come from one constant beside `from_lang`, in `core/gecko-extension-api/src/okf.rs`: `pub const SCRIPT_ENGINE_LANGS: [&str; 3] = ["js", "javascript", "quickjs"];`. `from_lang` matches against it.
- **An absent `engine` key** still parses as documentation, with `program: None`.
- **Comments are greenfield.** No `#[allow(dead_code)]`. Frozen fixtures under `*/tests/fixtures/` keep their text.
- **Integration tests** run with `-j 1`. Never touch the `gecko` database.
- **Before each commit:**
  - `cargo fmt --all -- --check`;
  - `cargo clippy --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`;
  - the same clippy run with `--features cyber`.

  Read clippy's own exit status. Don't pipe it through `tail`.
- **Commit trailer:** `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

- **A fence language in other casing or with trailing spaces** (```` ```JS ````, ```` ```js   ````) must still count. `from_lang` lowercases, and the blank-fence filter must not change that. A *leading* space (```` ``` JS ````) gives an empty language under `CODE_BLOCK_RE` (`linker.rs:21`); widening the regex is out of scope. Pinned in Task 1.
- **A concept with a matching fence but no `engine`** (the existing python/js documentation fixtures) must stay documentation. Pinned in Task 1.
- **YAML that parses `engine: no`, `engine: on` or `engine: 1e3` as a non-string** must be refused, and shown as written. serde_yaml 0.9 reads `no`/`on` as strings, so they're refused as unknown names. Pinned by the `5` and `true` cases.
- **A bundle with one bad playbook among good ones** must fail the whole parse, naming the bad file, not skip it. `parse_bundle` already propagates `?`. Pinned by the Task 1 CLI test.
- **The two error spellings.** "one of js, javascript, quickjs" and "a js, javascript or quickjs fence" must both be built from `SCRIPT_ENGINE_LANGS`, not typed twice. Pinned by reading the Task 1 diff.

---

### Task 1: the parser refuses an unsupported engine or a missing program

**Files:**
- Modify: `core/gecko-extension-api/src/okf.rs:36`, adding `SCRIPT_ENGINE_LANGS` and using it in `from_lang`.
- Modify: `core/gecko-engine/src/okf/linker.rs:83`. `extract_program` drops blank bodies. Its doc comment (`:76-82`) says so.
- Modify: the doc comments on `from_lang` (`okf.rs:32-35`), on `OkfConcept::program` (`okf.rs:66-69`) and at `core/gecko-engine/src/pipeline.rs:43`, so they describe the refusal.
- Test: `gecko-bin/tests/run_args_cli_test.rs`. The CLI test is written here, before the implementation, so it is seen to fail.
- Modify: `core/gecko-engine/src/okf/parser.rs`:
  - `ParseError` (`:18`) gains `InvalidEngine { path, value }` and `MissingProgram { path, engine }`;
  - the `engine` read (`:182`);
  - the program check after `:217`;
  - `fn yaml_as_written(v: &serde_yaml::Value) -> String`, extracted from `:191-194`;
  - the tests.

**Interfaces:**
- **Produces:**
  - `ParseError::InvalidEngine { path: String, value: String }`;
  - `ParseError::MissingProgram { path: String, engine: String }`;
  - `gecko_extension_api::okf::SCRIPT_ENGINE_LANGS`, and `pub fn or_list(items: &[&str]) -> String` beside it.

- [ ] **Step 1: Write the failing unit tests** in `parser.rs` `mod tests`, using `parse_concept` the way `parse_timeout` (`:709`) does:
  - `an_unsupported_engine_is_refused_as_written`:
    - **Scalars:** `javscript`, `python`, `no`, `5`, `true`, `null` and a blank value (`engine:` with nothing after it). Each gives `Err(ParseError::InvalidEngine { value, .. })`, with `value` equal to `javscript`, `python`, `no`, `5`, `true`, `null` and `null`. serde_yaml 0.9 reads `no` as a string; a blank renders `null`.
    - **Collections:** `[js]` and `{a: 1}` are refused with `InvalidEngine`. Only the path and the accepted list are asserted, because the YAML formatter renders block form (`- js`).
    - Every message contains `one of js, javascript, quickjs` and the path.
  - `engine_spellings_from_lang_accepts_parse`: `JS`, `" javascript "` (quoted, with spaces) and `quickjs`, each with a `js` fence, parse with `engine: Some(QuickJs)`.
  - `a_declared_engine_without_a_program_is_refused`. Each of these is `Err(ParseError::MissingProgram { engine, .. })`, with `engine == "quickjs"`:
    - `engine: quickjs` with no fence;
    - only a ```` ```javscript ```` fence;
    - only a ```` ```python ```` fence;
    - only a blank fence, with exactly the body `"```js\n\n```\n"` (a blank line inside).
  - `a_blank_fence_beside_a_real_one_is_skipped`: the body `"```js\n\n```\n\n```js\nconsole.log(1)\n```\n"` gives `program == Some("console.log(1)")`.
  - `a_fence_language_is_matched_like_engine`: ```` ```JS ```` and ```` ```js   ```` with code, under `engine: quickjs`, each give a program.
  - `a_fence_without_an_engine_stays_documentation`: no `engine` key, with a `js` fence, gives `engine: None` and `program: None`.
  - In `linker.rs` tests: `extract_program_skips_a_blank_matching_fence`, using the same two-fence body.
  - **The CLI test** `a_bundle_with_an_unsupported_engine_fails_to_sync`, in `run_args_cli_test.rs`, using `gecko(dir)` and `text`:
    - the bundle holds a good `playbooks/hello.md` (`engine: quickjs` with a `js` fence) and a bad `playbooks/typo.md` (`engine: javscript`);
    - `gecko --database <db> sync <bundle>` exits non-zero, and its output names `typo.md` and `javscript`;
    - `parse_bundle` runs before any database is created (`main.rs:391`), so the test asserts that `db.name` is not in `db.router().list_databases()`, as `run_args_cli_test.rs:89` does.
- [ ] **Step 2: Run them.**
  - Run: `cargo nextest run -p gecko-bin --no-default-features --test run_args_cli_test -j 1 unsupported_engine`
    - Expected: FAIL. The sync succeeds and creates the database.
  - Run: `cargo nextest run -p gecko-engine --no-default-features --lib okf`
    - Expected: a compile failure, because the variants aren't defined.
    - Temporarily comment out the variant-matching tests, run `a_blank_fence_beside_a_real_one_is_skipped` and the linker test, and watch them fail on the joined blank body. Then restore the tests.
- [ ] **Step 3: Implement.**
  - **`SCRIPT_ENGINE_LANGS`,** and `from_lang` matches `lang.trim().to_lowercase()` against it.
  - **`extract_program`:** `.filter(|f| !f.code.trim().is_empty())` before the join.
  - **`yaml_as_written`:** the `InvalidTimeout` arm calls it, so that message is unchanged.
  - **The `engine` read:** `match metadata.remove("engine")`:
    - `None` gives `None`;
    - `Some(Value::String(s))`, where `from_lang(&s)` is `Some(e)`, gives `Some(e)`, keeping the raw `s` for the second error;
    - anything else returns `InvalidEngine` with `yaml_as_written`.
  - **After `extract_program`:** if an engine was declared and `program` is `None`, return `MissingProgram { engine: s }`, the raw YAML string, not run through `yaml_as_written`.
  - **Both messages** build the language list from `SCRIPT_ENGINE_LANGS`: `join(", ")` for the first; "js, javascript or quickjs" for the second, through `or_list`, which lives beside the constant in `gecko-extension-api/src/okf.rs`.
- [ ] **Step 4: Fix the existing fixtures** that now fail:
  - `parse_timeout` (`:709-713`): the body becomes `"```js\n1\n```\n"`;
  - `:684`, which asserts `engine == Some(QuickJs)`, gains a one-line `js` fence;
  - `:644` already has fences and needs no change.
  - Then find every other in-repo concept with an engine and no fence: `grep -rln 'engine:' core gecko-bin extensions sample-bundle --include='*.rs' --include='*.md'`, then run the suites in Step 5. Fix each one by adding a fence. Record any test that only meant to declare an engine as a ruling.
- [ ] **Step 5: Run them.**
  - Run: `cargo nextest run -p gecko-engine -p gecko-extension-api --no-default-features && cargo nextest run -p gecko-bin --no-default-features --features cyber -j 1`
  - Expected: PASS.
- [ ] **Step 6: Commit** with the message `An unsupported engine, or a declared engine with no program, is refused at parse (N10)`.

### Task 2: the README and the docs

**Files:**
- Modify: `extensions/README.md:32`, where `build_extensions` is said to be in `gecko-bin/src/main.rs`; it is in `gecko-bin/src/config.rs`.
- Modify: `docs/refactor/KNOWN-LIMITATIONS.md`, adding a bullet in "Playbook `timeout-ms`" (`:136`), renamed to "Playbook frontmatter".
- Modify: `docs/refactor/OWED-WORK-LEDGER.md`, adding a Landed row for N10.

**Interfaces:**
- **Consumes:** nothing new; it documents Task 1.

- [ ] **Step 1: Update the docs.**
  - The README path.
  - **The `KNOWN-LIMITATIONS` bullet:** a bundle an earlier release synced with an unsupported `engine`, or an `engine` with no code in a matching fence, now fails to parse naming the file. The fix is to correct the file.
  - **The ledger row**, naming the tests. It is add-only: N10 has no open row.
  - **The ledger note:** a fence with no line inside (```` ```js\n``` ````) is matched across the next fence by `CODE_BLOCK_RE`. That is a pre-existing regex quirk, recorded and not fixed here.
- [ ] **Step 2: Commit** with the message `README and docs for the engine refusal`.

## Verification

- `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber -j 1` passes. The baseline is 878; the new tests add to it.
- fmt and both clippy configurations are clean.
- `grep -n 'gecko-bin/src/main.rs' extensions/README.md` no longer names `build_extensions`.
