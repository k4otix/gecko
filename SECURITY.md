# Security Policy

## Reporting a vulnerability

Please report suspected vulnerabilities **privately** through GitHub's
[private vulnerability reporting](https://github.com/k4otix/gecko/security/advisories/new)
(the **Report a vulnerability** button under the repository's **Security** tab).
Do not open a public issue for a security problem.

Please include enough detail to reproduce: affected version/commit, the component
(engine, sandbox, an extension, CI/release tooling), and a proof of concept if you
have one.

## Scope and expectations

GECKO is a solo hobby / research project, maintained on a **best-effort** basis
(see `CONTRIBUTING.md`). There is no SLA. Reports are triaged as time allows; a
realistic first-response target is a few weeks. Fixes ship on the same best-effort
cadence, prioritized by severity.

Especially in scope:

- Sandbox escape or the untrusted WASM guest reaching host capabilities it was not
  granted (the S1–S5 tiers, the S3 scope gate).
- TypeQL injection (values must always flow through parameterized `given` stages).
- Supply-chain integrity of the build and release pipeline.

## Supported versions

Only the latest `main` is supported. There are no backports.
