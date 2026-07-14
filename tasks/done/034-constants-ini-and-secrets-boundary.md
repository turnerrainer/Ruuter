# 034 — Prove constants.ini works end-to-end; carve out the secrets boundary

## Why

`[#KEY]` interpolation is claimed in the README and used in samples
(e.g. `DSL/samples/sources/stock-feed.yml.disabled` referencing
`[#alpaca_api_key]`), but nothing in the test suite covers the
happy path or the missing-key error path. If constants loading breaks
silently, every deploy that relies on it does too.

Separately: secrets management (Vault, KMS, Docker secrets) is a
different story — Ruuter's job is to consume a mounted constants
file, not to fetch secrets. This task locks in that boundary so it
doesn't creep.

## Acceptance

**constants.ini path (implement now):**

- `tests/constants.rs` covering:
  - Load a temp `.ini` with `[SECT]` header + `KEY=VALUE` lines →
    the returned `HashMap` contains `KEY` with the correct value.
  - Comments (`#`) and blank lines are skipped.
  - Missing key referenced from a WS source config (`[#unknown]`) →
    `resolve_constants` returns
    `Err(RuuterError::Config("undefined constant [#unknown]"))`.
  - A YAML DSL that references `[#KNOWN]` gets the value substituted
    at parse time (`DslParser::replace_constants`).
- README's "Configuration" section documents the file format and
  substitution rules explicitly (currently only mentioned in passing).

**Secrets boundary (document, don't implement):**

- New section in README titled "Secrets" that says:
  > Ruuter reads constants from a file. It does NOT fetch secrets
  > from Vault, KMS, or any other secret store. Mount your resolved
  > secrets as `constants.ini` (or bind a Vault-agent-rendered file
  > over it). Rotation is the deploy pipeline's job, not the
  > framework's.
- The Docker sample `docker-compose.yml` shows `constants.ini`
  mounted read-only — that stays as the shape.
