# Built-in endpoints

| Method + path        | Purpose | Auth |
|----------------------|---------|------|
| `GET /health`        | Liveness probe | none |
| `GET /_/openapi.json`| OpenAPI 3.1 spec generated from every DSL | none |
| `GET /_/sources`     | Source supervisor health | `RUUTER_ADMIN_ENABLED=true` |
| `GET /_/unguarded`   | Guarded vs unguarded HTTP route audit (issue #45) | `RUUTER_ADMIN_ENABLED=true` |

## `/health`

Request:

```bash
curl http://localhost:8080/health
```

Response:

```json
{"status":"ok"}
```

Always returns `200` when the process is up. Used by Docker's
`HEALTHCHECK`. Deliberately slim — no framework name, no version — so
a probe against `/health` can't be used to fingerprint the Ruuter
build for advisory-matching (h2ck.me S7 hardening).

## `/_/openapi.json`

See [OpenAPI generation](./openapi.md).

## `/_/sources`

Off by default. Enable:

```yaml
# docker-compose.yml
environment:
  - RUUTER_ADMIN_ENABLED=true
```

Returns a supervisor report:

```json
{
  "total": 1,
  "running": 1,
  "restarting": 0,
  "dead": 0,
  "sources": [
    {
      "id": { "kind": "websocket", "name": "stock-feed" },
      "status": { "status": "running" },
      "last_status_change_unix_ms": 1784032269184,
      "restart_count": 0
    }
  ]
}
```

`status.status` ∈ `{"starting", "running", "restarting", "dead"}`. Restarting/dead statuses include additional fields (`restart_count`, `last_error`, `next_attempt_in_ms` / `reason`).

## `/_/unguarded`

Off by default. Enable the same way as `/_/sources` (`RUUTER_ADMIN_ENABLED=true`).

Full runtime audit of guarded vs unguarded HTTP routes across every loaded project. Complements `dsl-lint --require-guard` — same underlying helper (`crate::dsl::guard_audit::audit_all_routes`), so a route that shows up as `unguarded` here would also fail the lint, and vice versa. Use the endpoint for post-deploy smoke checks and dashboard panels; use the lint in CI for pre-deploy audits.

```json
{
  "totals": {
    "routes": 53,
    "guarded": 7,
    "unguarded": 46
  },
  "projects": {
    "guarded-demo": {
      "guarded": [
        { "method": "GET", "path": "status", "guards": ["*"] },
        { "method": "POST", "path": "echo", "guards": ["*"] }
      ],
      "unguarded": []
    },
    "samples": {
      "guarded": [
        { "method": "GET", "path": "protected/data", "guards": ["GET/protected"] },
        { "method": "POST", "path": "ops/inject-fault/trigger", "guards": ["POST/ops/inject-fault"] }
      ],
      "unguarded": [
        { "method": "GET", "path": "ping" },
        { "method": "POST", "path": "state/inc" }
      ]
    }
  }
}
```

- **`guards` field** — array of guard keys in outer-first execution order. `*` is the reserved [project-level guard](../dsl/guards.md#project-level--guardyml-at-the-project-root-issue-39) key (issue #39); everything else is a `<METHOD>/<path>` method-scoped key.
- **`unguarded` list** — routes with zero applicable guards. This is what @angryziber's discussion around issue #41 flagged as the "silent unguarded peer" trap — the endpoint enumerates them so a reviewer doesn't have to eyeball the DSL tree.
- **HTTP routes only** — WS/inbound handlers are excluded from the audit for now; the guard chain doesn't fire on the WS path (see [Guards](../dsl/guards.md)).
- **Deterministic order** — sorted by `(project, method, path)` so diffs across deploys are meaningful.

### Consuming from a dashboard

```bash
# Unguarded route count as a Prometheus-shape gauge
curl -s http://ruuter:8080/_/unguarded | jq '.totals.unguarded'

# Per-project unguarded lists
curl -s http://ruuter:8080/_/unguarded \
  | jq '.projects | to_entries[] | "\(.key): \(.value.unguarded | length) unguarded"'
```
