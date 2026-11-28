# An unrunnable playbook is refused at parse — design

Closes owed-work ledger item **N10**, which an OpenWiki pass surfaced. It's a small change,
so this is the short design.

## Intent

A concept that declares an `engine` means to run. Today two mistakes make it sync silently
as documentation, with nothing to run and nothing pointing at the cause:

- **An unsupported `engine` value.** `parser.rs:182` reads `engine` with `extract_string`,
  which also turns numbers and bools into strings, then maps the result through
  `ScriptEngine::from_lang` (`gecko-extension-api/src/okf.rs:36`). That accepts `js`,
  `javascript` and `quickjs`, case-insensitive and trimmed; everything else becomes `None`.
  So `engine: javscript`, `engine: python`, `engine: 5` and `engine: [js]` are all silently
  "no engine". `engine: null` and a blank value are silently "no key".
- **A declared engine with no program.** `engine: quickjs` with no fence in an accepted
  language (a typo like ```` ```javscript ````, or no fence at all) gives `program: None`.
  A blank fence (```` ```js ```` with an empty body) gives `Some("")`, because
  `extract_program` (`linker.rs:83`) keeps empty bodies.

Done when:
- a concept whose `engine` is present but unsupported fails `parse_bundle`, naming the file
  and the value as written;
- a concept that declares a supported engine but has no non-blank code in a matching fence
  also fails `parse_bundle`, naming the file;
- a concept with no `engine` key still parses as documentation.

This follows the N5 precedent: `timeout-ms` went from "silently ignored" to "refused at
parse".

## Approaches

- **Chosen, with the user: refuse both at parse.** Both are the same silent failure, and
  `parse_concept` catches them before anything reaches TypeDB.
- **Rejected: refuse only the `engine` value.** That would leave a mislabelled fence
  syncing as documentation.
- **Rejected: catch it at `gecko run`.** The playbook would already be in the graph as
  documentation, and the author would learn about it only when someone tries to run it.

## Design

- **`ParseError::InvalidEngine { path, value }`:** "Concept file '{path}' has an invalid
  'engine' ({value}); it must be one of js, javascript, quickjs".
  - It covers any present `engine` that is not a string `from_lang` accepts: an unknown
    name, a number, a bool, a list, a map, `null` and a blank value.
  - The parser matches `serde_yaml::Value::String(s)` directly on `metadata.remove("engine")`,
    instead of `extract_string`, so `5` and `true` are refused, not coerced.
- **`ParseError::MissingProgram { path, engine }`:** "Concept file '{path}' declares engine
  '{engine}' but has no code in a js, javascript or quickjs fence".
  - `{engine}` is the value as written.
  - It covers no matching fence, and matching fences that are all blank.
- **Blank fences.** `extract_program` drops fence bodies that are empty after trimming, so a
  concept whose only matching fences are blank gives `None`. A blank fence next to a real
  one is simply skipped.
- **Quoting as written.** The "shown as YAML" formatter in the `InvalidTimeout` arm
  (`parser.rs:191-194`) becomes a small helper, `fn yaml_as_written(v: &serde_yaml::Value)
  -> String`, used by both errors.
- **The accepted list.** "js, javascript, quickjs" appears in both messages. It comes from
  one constant next to `from_lang` in `gecko-extension-api/src/okf.rs`, so the messages
  can't drift from what is accepted.
- **Ride-along doc fix.** `extensions/README.md:32` says `build_extensions` lives in
  `gecko-bin/src/main.rs`; it lives in `gecko-bin/src/config.rs`.
- **Docs.** `docs/refactor/KNOWN-LIMITATIONS.md` gets a line beside the `timeout-ms` one: a
  bundle an earlier release synced with an unsupported `engine` or a missing program now
  fails to parse, and the fix is to correct the file. The ledger records N10 under Landed.

## Tests

- **Parser unit tests** (`core/gecko-engine/src/okf/parser.rs`):
  - `InvalidEngine` for `javscript`, `python`, `5`, `true`, `[js]`, `null` and a blank value,
    each naming the file and showing the value as written;
  - `JS`, ` javascript ` and `quickjs` are accepted (case and whitespace, as `from_lang`
    allows), and so are ```` ```JS ```` and ```` ```js   ```` fences. A leading space in a fence
    tag (```` ``` JS ````) yields an empty language under `CODE_BLOCK_RE`; widening it is out of
    scope;
  - `MissingProgram` for `engine: quickjs` with no fence, with only a ```` ```javscript ````
    fence, with only a ```` ```python ```` fence, and with only a blank ```` ```js ```` fence;
  - a blank `js` fence beside a real one gives the real program alone;
  - no `engine` key with a `js` fence still parses as documentation (`program: None`).
- **Linker unit test** (`linker.rs`): `extract_program` skips a blank matching fence.
- **Existing fixtures.** Three parser tests (`parser.rs:644`, `:684`, `:713`) declare
  `engine: quickjs` with prose bodies and no fence. Each gets a one-line `js` fence, or
  drops `engine` where the engine isn't under test.
- **End to end:** a `gecko sync` of a bundle holding one `engine: javscript` playbook fails,
  naming the file, and writes nothing (`gecko-bin/tests/run_args_cli_test.rs`, which has the
  CLI helpers).
- **Regression:** every shipped bundle still parses; the walkthrough and sample-bundle
  tests already sync them.

## Non-goals

- Programmatic `OkfConcept` callers. `sync_bundle` doesn't re-check `engine` against
  `program`: the extension converters (`sensors.rs`, `stix/mod.rs`) set `engine: None`, and
  `gecko run` already refuses a concept with no program.
- New engines or new spellings.

## Left to the builder

- The error variants' exact field names, within the messages above.
- Test placement inside the named files.
