# declaration

DSL metadata. Runtime no-op except for the `override_ancestors` flag on guards.

```yaml
declaration:
  description: "Cancel an order and archive its audit trail."
  version: "1"
  method: "POST"
  accepts: "application/json"
  returns: "application/json"
  allowed_body:   [order_id, reason]
  allowed_header: [Authorization, Idempotency-Key]
  allowed_params: [correlation_id]
  override_ancestors: false        # only meaningful in guard DSLs
```

## Effects

- **OpenAPI generation** reads `description`, `allowed_body`, `allowed_header`, `allowed_params` to shape the operation entry.
- **`override_ancestors: true`** on a guard DSL makes it REPLACE ancestor guards for its subtree (see [Guards](../guards.md)).

Any declaration field is optional. All are metadata only — nothing enforces `allowed_body` etc. at request time.

## Runnable example — what the OpenAPI output looks like

A DSL without a `declaration:` block still appears in the spec — the
generator falls back to a synthesised description:

```console
$ curl -s http://localhost:8080/_/openapi.json | \
    jq '.paths["/samples/ping"].get | {description, operationId}'
{
  "description": "Auto-generated from DSL `GET/ping` in project `samples`. Add a `declaration.description` to override.",
  "operationId": "get_samples_ping"
}
```

Add a `declaration.description` at the top of the DSL and the fallback
sentence is replaced with your own text. The `operationId`, request /
response schemas, and status-code entries are generated the same way
either way.

## Runnable example — guard override in action

`DSL/samples/POST/ops/inject-fault.guard.yml` uses `override_ancestors`
to *replace* the folder-wide guard rather than stacking on it — so
the `inject-fault` endpoint is always 403 in production even though
its sibling endpoints under `POST/ops/` may be permitted:

```yaml
declaration:
  override_ancestors: true

deny:
  status: 403
  return:
    error: "inject-fault is disabled in production"
  next: end
```

See [Guards](../guards.md) for the full override-vs-stacking
resolution rules.
