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
