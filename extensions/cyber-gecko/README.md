# cyber-gecko

A GECKO domain extension for detection engineering. It imports STIX 2.1 / MITRE
ATT&CK bundles into typed cyber entities, loads a sensor file describing what you
monitor, and gives playbooks the `cyber-gecko` host imports `claim`, `disposition`,
`coverage_gaps`, `blinded`, `recommend_sources` and `precision`. It compiles in under the `cyber` feature
and is activated with `"cyber"` in `gecko.toml`'s `[extensions] enabled`. The
extension contract it follows is [`extensions/README.md`](../README.md); its caveats
are in [`docs/refactor/KNOWN-LIMITATIONS.md`](../../docs/refactor/KNOWN-LIMITATIONS.md).

## Importing ATT&CK

```bash
gecko sync-stix enterprise-attack.json --database attack
```

`sync-stix` maps every STIX object to a cyber entity and every modelled relationship
to a typed relation. ATT&CK's detection model (Enterprise v18 or later) arrives as
detection strategies, the analytics that carry them out on each platform, and each
analytic's log-source requirements on data components. A re-import under the same
`--bundle-name` (default `stix-sync`) replaces the previous one, including the
strategy, analytic and requirement links, so a link a later release drops does not
linger.

Import each ATT&CK domain into its own database. Enterprise, Mobile and ICS ship some
of the same STIX objects, so a second domain in the same database is refused:

```bash
gecko sync-stix enterprise-attack.json --database attack
gecko sync-stix mobile-attack.json --database attack-mobile
```

## The sensor file

A YAML file naming the platforms you run and the sensors that watch them:

```yaml
platforms: [Windows, Linux]
sensors:
  - name: windows-edr
    platforms: [Windows]
    log_sources: [WinEventLog:Sysmon, WinEventLog:Security]
  - name: linux-audit
    platforms: [Linux]
    log_sources: [auditd:SYSCALL]
    components: [Command Execution]
```

- `platforms` (required, non-empty): the platforms `blinded` assesses. Spell each as
  ATT&CK spells its analytics' platforms (`Windows`, `Linux`, `macOS`, ...).
- `sensors` (optional list; empty means nothing is collected anywhere). Each entry has:
  - `name` (required): unique in the file, of the form `[A-Za-z0-9._-]+`.
  - `platforms` (required, non-empty): the platforms the sensor runs on. A sensor covers
    a requirement only on a platform it runs on.
  - `log_sources`: the ATT&CK log-source names it collects, such as
    `WinEventLog:Security`.
  - `components`: ATT&CK data-component names it covers as a fallback, for a log source
    you collect under a name ATT&CK does not use.

  A sensor needs at least one of `log_sources` and `components`.

Unknown fields are refused. Names are trimmed and matched case-insensitively, and
otherwise exactly: there are no aliases.

## `gecko sync-sensors`

```bash
gecko sync-sensors sensors.yaml --database attack [--bundle-name sensors]
```

Syncs the file into the database that already holds the ATT&CK import; `--database`
is required. Before syncing the file it refuses:

- a database that does not exist, or holds no ATT&CK detection strategies (sync the
  ATT&CK bundle into it first);
- a `--bundle-name` (default `sensors`) whose bundle holds records other than sensors;
- a platform that matches an ATT&CK analytic platform only ignoring case (`windows`
  for `Windows`);
- a fallback component that does not match exactly one live ATT&CK data component.

After the sync it prints a notice for each log source no live analytic reads, each
sensor platform outside the file's `platforms`, and each declared platform no live
analytic runs on. A re-sync under the same bundle name replaces the previous file. Two
sensor files can share a database under two bundle names; `blinded` reads their union.

## `coverage_gaps`

A playbook granted the `cyber-gecko:coverage_gaps` scope calls
`gecko.callExtension("cyber-gecko", "coverage_gaps", {})`. It lists the live techniques that
have no asserted detection claim, and groups them by live tactic:

```json
{
  "undetected_techniques": [
    { "concept_id": "…", "attack_id": "T1059", "tactics": ["TA0002"],
      "sub_techniques": { "claimed": 1, "total": 3 } }
  ],
  "by_tactic": [
    { "attack_id": "TA0002", "concept_id": "…", "shortname": "execution", "name": "Execution",
      "live": 12, "undetected": 2, "techniques": ["T1059", "T1059.001"] }
  ]
}
```

- **`live` and `undetected`.** `live` counts the tactic's live techniques, parents and
  sub-techniques alike. `undetected` counts how many of them are gaps, and `techniques` lists
  those gaps by ATT&CK id.
- **Which tactics appear.** A fully covered tactic stays, with `undetected: 0`. A tactic with no
  live technique is absent.
- **Liveness.** Retired, revoked and deprecated techniques and tactics are left out
  everywhere, though they keep their tactic edges.
- **Ordering.** Every list is sorted by ATT&CK id. In `undetected_techniques`, a technique
  without an ATT&CK id comes last, ordered by concept id.
- **Missing values.** A tactic without a title has `name: null`, and a gap technique with no
  live tactic has `tactics: []`.

## `blinded`

A playbook granted the `cyber-gecko:blinded` scope calls
`gecko.callExtension("cyber-gecko", "blinded", {})`. For every asserted detection claim
and every platform a sensor file declares, it looks at the claimed technique's live
analytics on that platform:

- **covered**: at least one analytic can run, because it has requirements and a live
  sensor on that platform collects each requirement's log source or covers its data
  component. When a platform has several analytics, any one that can run covers it.
- **blind**: analytics exist there but none can run.
- **unassessed**: ATT&CK gives no live analytic there, or the technique is retired,
  revoked or deprecated.

The report lists each claim with a blind or unassessed platform. Each blind platform
has one entry per analytic there, with the log sources it lacks and the data
components each would serve (`no_log_sources` is true for an analytic that names
none). `stale_sensor_components` lists sensors' fallback components that match no live
data component.

```json
{
  "blinded_detections": [
    {
      "concept_id": "playbooks/detect-t1059",
      "technique": "T1059",
      "technique_concept_id": "stix/attack-pattern/...",
      "blind_platforms": [
        {
          "platform": "Linux",
          "analytic": "AN0002",
          "no_log_sources": false,
          "missing": [
            { "log_source": "linux:syslog", "data_components": ["Command Execution"] }
          ]
        }
      ],
      "unassessed_platforms": ["macOS"]
    }
  ],
  "stale_sensor_components": [
    { "sensor": "linux-audit", "component": "Old Component" }
  ]
}
```

`blinded` returns an error, never an empty report, when no sensor file is synced or
the graph holds no current ATT&CK detection data (sync or re-sync the ATT&CK bundle).

## `recommend_sources`

A playbook granted the `cyber-gecko:recommend_sources` scope calls
`gecko.callExtension("cyber-gecko", "recommend_sources", {})`. It turns `blinded`'s
blind platforms into an ordered list of ATT&CK log sources to start collecting. Each
step adds one analytic's remaining missing log sources on one platform: the step that
unblinds the most (claim, platform) pairs per new log source, ties going to the step
that unblinds more, then to the smaller platform and keys. A later step counts only the
sources earlier steps have not already added. `cumulative_unblinded` is the running
count of (claim, platform) pairs that can run once every step so far is in place.

Claims no log source can fix are listed under `unfixable`: `no-requirements` when every
live analytic on that platform names no live log source, and `no-analytic` when the
platform is unassessed (no live analytic there, or the technique is retired, revoked or
deprecated).

```json
{
  "steps": [
    {
      "platform": "Windows",
      "add": [
        { "log_source": "WinEventLog:Security", "key": "wineventlog:security",
          "channels": ["EventCode=4103", "EventCode=4688"] }
      ],
      "unblinds": [
        { "concept_id": "playbooks/detect-t1059", "technique": "T1059",
          "technique_concept_id": "stix/attack-pattern/..." }
      ],
      "cumulative_unblinded": 1
    }
  ],
  "unfixable": [
    { "concept_id": "playbooks/detect-t1059", "technique": "T1059",
      "technique_concept_id": "stix/attack-pattern/...", "platform": "macOS",
      "reason": "no-requirements" }
  ]
}
```

`channels` lists every channel ATT&CK names for that log source on that platform,
across all blind claims, since each source is recommended once, in the first step that
needs it. The list recommends log sources only, never a fallback data component, and
errors on the same graphs as `blinded`.
