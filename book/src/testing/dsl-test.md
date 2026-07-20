# dsl-test

Scenario runner. Walks `DSL-tests/`, loads each `*.test.yml`, executes its scenarios against the DSL tree, reports pass/fail per scenario. Exits non-zero on any failure.

## Usage

```bash
dsl-test                                              # ./DSL, ./DSL-tests, ./constants.ini
dsl-test --dsl DSL --tests DSL-tests                  # explicit roots
dsl-test --constants constants.ini
dsl-test --filter GET/basic                           # substring on test-file path
dsl-test --json                                       # machine-readable summary
```

## How it runs

For each `.test.yml` file:

1. Parse the file into a [`TestFile`](./schema.md).
2. Merge `constants:` (from the file) over the base `constants.ini`.
3. For `mock-http` / `trigger-inject`: spawn a mock upstream on `127.0.0.1:0`, expand `{MOCK}` in constants, apply `http_rewrite` via the `RUUTER_HTTP_REWRITE` env var.
4. Build one `Harness` — fresh `DslLoader` + `StepEngine` + `DslRouter` + `TriggerDispatcher` + `StateStore`.
5. Run scenarios in declaration order. State persists across scenarios within one file; nothing leaks between files.
6. For each scenario: run request → assert response → assert state → assert mock calls.

## Output

```
pass  DSL-tests/samples/GET/ping.test.yml::returns pong with 202
pass  DSL-tests/samples/GET/basic/hello.test.yml::returns greeting
fail  DSL-tests/samples/POST/state/inc.test.yml::first call returns 1
      body_matches: subset failed
        expected: {"counter":1}
        actual:   {"counter":0}

dsl-test: 98 scenario(s) — 97 passed, 1 failed
```

`--json` emits:

```json
{
  "total": 98,
  "passed": 98,
  "failed": 0,
  "items": [
    {"path": "DSL-tests/...", "scenario": "returns pong", "ok": true, "error": null},
    ...
  ]
}
```

## Filtering

`--filter <substring>` selects tests whose file path contains the substring. Useful for focused re-runs:

```bash
dsl-test --filter GET/http     # only external-HTTP tests
dsl-test --filter WS/          # only WebSocket tests
```

## Exit codes

- `0` — every scenario passed
- `1` — at least one scenario failed OR no test files were found under `--tests`
- `2` — invalid CLI flag

## What runs where

`dsl-test` routes HTTP scenarios through `tower::ServiceExt::oneshot` on the built axum router. This means:

- CSRF, traceparent, method allow-list, response-default-headers all run.
- No TCP socket bound (except for the mock upstream and the ws-client mode's ephemeral server).
- Scenario execution is single-process, deterministic, and fast: the 98-scenario corpus runs in ~2 s.

For WebSocket mode (`ws-client`), a real axum server is bound on `127.0.0.1:0` and driven by a `tokio-tungstenite` client — the DSL's `ws_send` step operates on real frames through a real socket.
