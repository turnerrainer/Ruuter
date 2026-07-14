# 024 — Load AppConfig from file/env instead of hard-coded default

## Why

`src/main.rs:29` currently does `let config = AppConfig::default()`.
Every field of the config surface — CSRF allow-list, SSRF allow-list,
CORS origins, Idempotency TTL, method allow-list, Boa runtime limits,
response-size cap, upstream status filter, etc. — is unreachable to
an operator without recompiling. The whole 0.4.0 hardening effort is
inert in a real deploy.

## Acceptance

- Config file path is resolvable via (in priority):
  1. `--config <path>` CLI flag.
  2. `RUUTER_CONFIG` env var.
  3. `./ruuter.yaml` or `./ruuter.yml` in the working directory.
  4. Built-in defaults (current behaviour) if none of the above exist.
- File is YAML; schema mirrors `AppConfig`. Unknown keys are ignored
  (forward-compat) but produce a startup warning naming each.
- Missing keys inherit `Default::default()` for their type.
- A worked sample lives at `DSL/samples/ruuter.yaml.example`
  demonstrating every top-level knob with representative values.
- README's "Configuration" section is updated with the resolution
  order and points at the sample.
- Integration test: spin the router with a tempfile config setting
  `cors.allowed_origins` non-empty; assert `Access-Control-Allow-Origin`
  appears on responses.
