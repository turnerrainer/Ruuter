# Built-in endpoints

| Method + path        | Purpose | Auth |
|----------------------|---------|------|
| `GET /health`        | Liveness probe | none |
| `GET /_/openapi.json`| OpenAPI 3.1 spec generated from every DSL | none |
| `GET /_/sources`     | Source supervisor health | `RUUTER_ADMIN_ENABLED=true` |

## `/health`

```
$ curl http://localhost:8080/health
{"service":"ruuter-rs","status":"ok","version":"0.4.0"}
```

Always returns `200` when the process is up. Used by Docker's `HEALTHCHECK`.

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
