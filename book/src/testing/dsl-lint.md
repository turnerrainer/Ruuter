# dsl-lint

Static validator. Reads every DSL, reports errors and warnings. Exits non-zero on any error.

## Usage

```bash
dsl-lint                                       # ./DSL and ./constants.ini
dsl-lint --dsl DSL                             # explicit root
dsl-lint --dsl DSL --constants constants.ini   # explicit constants
dsl-lint --dsl DSL --include-disabled          # also validate *.yml.disabled
dsl-lint --json                                # machine-readable output
dsl-lint --require-guard                       # error on any HTTP route with zero guards (issue #45)
```

## What it checks

| Check | Severity | Notes |
|---|---|---|
| YAML parse | error | The DSL file is valid YAML |
| Step body is a mapping | error | Each top-level key must map to a step config |
| Step kind recognised | error | One of `assign`, `return`, `call`, `switch`, `log`, `template`, `state`, `iterate`, `ws_send`, `declaration` |
| `next:` target resolves | error | Named step must exist in the same DSL (or be `end`) |
| Switch branch `next:` targets resolve | error | Same as above, per branch |
| Template target exists | error | `template:` field must name a DSL key present in the tree |
| Reachable from entry | warning | Every step must be reachable from the first-declared step (following `next:` and switch branches; steps without `next:` fall through in source order) |
| Constants resolve | warning | Every `[#name]` must exist in `constants.ini` after per-file overrides |
| Cron-job shape | error | Files under `cronmanager-jobs/` need `trigger`, `type`, `url` per job |
| Source shape | error | Files under `sources/` need `kind:` (currently only `websocket` supported) |

## Exit codes

- `0` — no errors (warnings do not fail)
- `1` — one or more errors
- `2` — invalid CLI flag

## Output format

Default is human-readable ANSI:

```
error  DSL/samples/POST/broken.yml: step 'validate': next target 'process' does not resolve to any step in this DSL
warn   DSL/samples/GET/http/with-headers.yml: unresolved constant reference '[#API_KEY]' — add to constants.ini or the runner will forward the literal to reqwest

dsl-lint: 54 file(s) scanned, 53 ok, 1 error(s), 1 warning(s)
```

`--json` emits structured output for CI:

```json
{
  "files_scanned": 54,
  "files_ok": 54,
  "errors": 0,
  "warnings": 3,
  "items": [
    { "severity": "warning", "path": "DSL/samples/...", "message": "..." }
  ]
}
```

## `--require-guard` (issue #45)

Opt-in audit mode. Loads the DSL tree via the same loader the runtime uses, walks every HTTP route, resolves its applicable guards through the shared audit helper, and emits **one error per route with zero applicable guards**. Complements the `GET /_/unguarded` admin endpoint (which reports the same data at runtime).

```bash
$ dsl-lint --dsl DSL --require-guard
error  api/POST/is_this_unguarded: no applicable guard — add a project-level, method-scoped, or per-endpoint guard, or drop --require-guard for this route
error  api/GET/health: no applicable guard — add a project-level, method-scoped, or per-endpoint guard, or drop --require-guard for this route

dsl-lint: 15 file(s) scanned, 15 ok, 2 error(s), 0 warning(s)
```

- **Default off** — public endpoints legitimately exist.
- **Path shown** — `<project>/<METHOD>/<route>` (synthetic path, not a filesystem path — matches the format `GET /_/unguarded` emits).
- **Applies to HTTP routes only.** WS/inbound handlers are excluded (the guard chain doesn't fire on the WS path today; see [Guards](../dsl/guards.md)).
- **Ordering matches the runtime**. Uses the same `guard_keys_for_dsl` helper `DslRouter::applicable_guards` uses at request time — no drift between "would this route pass in prod" vs "does the audit see it as guarded".

Combine with `--json` for CI parsing:

```bash
dsl-lint --require-guard --json | jq '.items[] | select(.message | startswith("no applicable guard"))'
```

## When to run

- Pre-commit hook: catches typos in ~100 ms without spinning any harness.
- CI first stage: fails the build before slower runtime tests run.
- After editing `constants.ini`: verifies every reference still resolves.
