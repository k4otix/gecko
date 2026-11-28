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

- **A fence language with surrounding spaces or other casing** (```` ``` JS ````) must still count. `from_lang` already trims and lowercases, and the blank-fence filter must not change that. Pinned in Task 1.
- **A concept with a matching fence but no `engine`** (the existing python/js documentation fixtures) must stay documentation. Pinned in Task 1.
- **YAML that parses `engine: no`, `engine: on` or `engine: 1e3` as a non-string** must be refused, and shown as written. serde_yaml 0.9 reads `no`/`on` as strings, so they're refused as unknown names. Pinned by the `5` and `true` cases.
- **A bundle with one bad playbook among good ones** must fail the whole parse, naming the bad file, not skip it. `parse_bundle` already propagates `?`. Pinned by the Task 2 CLI test.
- **The two error spellings.** "one of js, javascript, quickjs" and "a js, javascript or quickjs fence" must both be built from `SCRIPT_ENGINE_LANGS`, not typed twice. Pinned by reading the Task 1 diff.

---

### Task 1: the parser refuses an unsupported engine or a missing program

**Files:**
- Modify: `core/gecko-extension-api/src/okf.rs:36`, adding `SCRIPT_ENGINE_LANGS` and using it in `from_lang`.
- Modify: `core/gecko-engine/src/okf/linker.rs:83`. `extract_program` drops blank bodies.
- Modify: `core/gecko-engine/src/okf/parser.rs`:
  - `ParseError` (`:18`) gains `InvalidEngine { path, value }` and `MissingProgram { path, engine }`;
  - the `engine` read (`:182`);
  - the program check after `:217`;
  - `fn yaml_as_written(v: &serde_yaml::Value) -> String`, extracted from `:191-194`;
  - the tests.

**Interfaces:**
- **Produces:** `ParseError::InvalidEngine { path: String, value: String }`, `ParseError::MissingProgram { path: String, engine: String }`, and `gecko_extension_api::okf::SCRIPT_ENGINE_LANGS`.

- [ ] **Step 1: Write the failing unit tests** in `parser.rs` `mod tests`, using `parse_concept` the way `parse_timeout` (`:709`) does:
  - `an_unsupported_engine_is_refused_as_written`. For each of `javscript`, `python`, `5`, `true`, `[js]`, `null` and a blank value (`engine:` with nothing after it), the result is `Err(ParseError::InvalidEngine { value, .. })`. `value` is the YAML as written (`javscript`, `5`, `true`, `[js]`, `null`, `null`), and the message contains `one of js, javascript, quickjs` and the path.
  - `engine_spellings_from_lang_accepts_parse`: `JS`, `" javascript "` (quoted, with spaces) and `quickjs`, each with a `js` fence, parse with `engine: Some(QuickJs)`.
  - `a_declared_engine_without_a_program_is_refused`. Each of these is `Err(ParseError::MissingProgram { engine, .. })`, with `engine == "quickjs"`:
    - `engine: quickjs` with no fence;
    - only a ```` ```javscript ```` fence;
    - only a ```` ```python ```` fence;
    - only a blank ```` ```js ```` fence.
  - `a_blank_fence_beside_a_real_one_is_skipped`: a blank ```` ```js ```` fence, then ```` ```js\nconsole.log(1)\n``` ````, gives `program == Some("console.log(1)")`.
  - `a_fence_language_is_matched_like_engine`: ```` ``` JS ```` with code, under `engine: quickjs`, gives a program.
  - In `linker.rs` tests: `extract_program_skips_a_blank_matching_fence`.
- [ ] **Step 2: Run them.**
  - Run: `cargo nextest run -p gecko-engine --no-default-features --lib okf`
  - Expected: a compile failure, because the variants aren't defined. Temporarily comment out the variant-matching tests, run `a_blank_fence_beside_a_real_one_is_skipped` and the linker test, and watch them fail on `Some("")`/the joined blank. Then restore the tests.
- [ ] **Step 3: Implement.**
  - **`SCRIPT_ENGINE_LANGS`,** and `from_lang` matches `lang.trim().to_lowercase()` against it.
  - **`extract_program`:** `.filter(|f| !f.code.trim().is_empty())` before the join.
  - **`yaml_as_written`:** the `InvalidTimeout` arm calls it, so that message is unchanged.
  - **The `engine` read:** `match metadata.remove("engine")`:
    - `None` gives `None`;
    - `Some(Value::String(s))`, where `from_lang(&s)` is `Some(e)`, gives `Some(e)`, keeping `s` for the second error;
    - anything else returns `InvalidEngine` with `yaml_as_written`.
  - **After `extract_program`:** if an engine was declared and `program` is `None`, return `MissingProgram { engine: <the string as written> }`.
  - **Both messages** build the language list from `SCRIPT_ENGINE_LANGS`: `join(", ")` for the first; "js, javascript or quickjs" for the second, through a small `fn or_list(&[&str]) -> String`.
- [ ] **Step 4: Fix the existing fixtures** that now fail:
  - `parse_timeout` (`:709-713`): the body becomes `"```js\n1\n```\n"`;
  - the fixtures at `:644` and `:684`: check each, and add a one-line `js` fence or drop `engine` where the engine isn't under test.
  - Then find every other in-repo concept with an engine and no fence: `grep -rln 'engine:' core gecko-bin extensions sample-bundle --include='*.rs' --include='*.md'`, then run the suites in Step 5. Fix each one by adding a fence. Record any test that only meant to declare an engine as a ruling.
- [ ] **Step 5: Run them.**
  - Run: `cargo nextest run -p gecko-engine -p gecko-extension-api --no-default-features && cargo nextest run -p gecko-bin --no-default-features --features cyber -j 1`
  - Expected: PASS.
- [ ] **Step 6: Commit** with the message `An unsupported engine, or a declared engine with no program, is refused at parse (N10)`.

### Task 2: the CLI test, the README and the docs

**Files:**
- Test: `gecko-bin/tests/run_args_cli_test.rs`.
- Modify: `extensions/README.md:32`, where `build_extensions` is said to be in `gecko-bin/src/main.rs`; it is in `gecko-bin/src/config.rs`.
- Modify: `docs/refactor/KNOWN-LIMITATIONS.md`, adding a bullet in "Playbook `timeout-ms`" (`:136`), renamed to "Playbook frontmatter".
- Modify: `docs/refactor/OWED-WORK-LEDGER.md`, adding a Landed row for N10.

**Interfaces:**
- **Consumes:** `ParseError::InvalidEngine` (Task 1).

- [ ] **Step 1: Write the test** `a_bundle_with_an_unsupported_engine_fails_to_sync`, using the `gecko(dir)` and `text` helpers.
  - The bundle holds a good `playbooks/hello.md` (`engine: quickjs` with a `js` fence) and a bad `playbooks/typo.md` (`engine: javscript`).
  - `gecko --database <db> sync <bundle>` exits non-zero.
  - Its output names `typo.md` and `javscript`.
  - A `gecko --database <db> run playbooks/hello` afterwards fails as unknown, because nothing was synced.

  This passes against Task 1's code. To see it fail first, revert Task 1's `engine` read with `git stash push -- core/gecko-engine/src/okf/parser.rs`, run the test, then `git stash pop`.
- [ ] **Step 2: Run it.**
  - Run: `cargo nextest run -p gecko-bin --no-default-features --test run_args_cli_test -j 1 unsupported_engine`
  - Expected: PASS, and FAIL under the stash.
- [ ] **Step 3: Update the docs.**
  - The README path.
  - **The `KNOWN-LIMITATIONS` bullet:** a bundle an earlier release synced with an unsupported `engine`, or an `engine` with no code in a matching fence, now fails to parse naming the file. The fix is to correct the file.
  - **The ledger row**, naming the tests.
- [ ] **Step 4: Commit** with the message `A bundle with an unsupported engine fails to sync; README and docs`.

## Verification

- `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber -j 1` passes. The baseline is 878; the new tests add to it.
- fmt and both clippy configurations are clean.
- `grep -n 'gecko-bin/src/main.rs' extensions/README.md` no longer names `build_extensions`.
