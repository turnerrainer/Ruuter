# Guards

Pre-execution DSLs that run before the main route. A guard returning HTTP status ≥ 400 short-circuits — its response becomes the response.

## Two file conventions

### Sibling — `<stem>.guard.yml`

```
DSL/svc/GET/protected.guard.yml     # guards every DSL under protected/
DSL/svc/GET/protected/data.yml      # ← protected
```

### In-folder — `.guard.yml` inside the protected folder

```
DSL/svc/GET/vault/.guard.yml        # guards every DSL under vault/
DSL/svc/GET/vault/secret.yml        # ← protected
```

Both conventions produce the same guard key (`GET/protected` / `GET/vault`). Use whichever fits your tree layout. Bare `.guard` (no extension) is also accepted for strict Java-Ruuter parity.

## Guard DSL

Any regular DSL. Use `return` with `status: 4xx` to reject; return `status: 2xx` (or omit) to let the request through.

```yaml
# DSL/svc/GET/protected.guard.yml
check:
  switch:
    - condition: "${!incoming.headers['x-token']}"
      next: deny
  next: allow

allow:
  return:
    auth: ok
  next: end

deny:
  status: 401
  return:
    error: "missing token"
  next: end
```

## Stacking

Multiple ancestor guards on the same path stack. **All** must pass. Outermost runs first.

```
POST/api.guard.yml            # runs first
POST/api/admin.guard.yml      # runs second
POST/api/admin/users.yml      # main DSL, runs if both guards passed
```

## Passing variables to the main DSL

Guard-side `assign` values reach the main DSL:

```yaml
# guard
parse:
  assign:
    user_id: "${incoming.headers['x-token']}"
  next: allow

allow:
  return:
    ok: true
  next: end
```

```yaml
# main
respond:
  return:
    user: "${user_id}"
  next: end
```

## Override — replace ancestors

Add `declaration.override_ancestors: true` to a guard to make it REPLACE (not stack with) ancestor guards for its subtree. Most-specific override wins if multiple match.

```yaml
# DSL/svc/POST/ops/.guard.yml — folder guard: requires Bearer
check:
  switch:
    - condition: "${!incoming.headers['authorization']}"
      next: deny
  next: ok

ok:
  return:
    auth: true
  next: end

deny:
  status: 401
  return:
    error: "bearer required"
  next: end
```

```yaml
# DSL/svc/POST/ops/inject-fault.guard.yml — override: production-disabled
declaration:
  override_ancestors: true

deny:
  status: 403
  return:
    error: "inject-fault disabled"
  next: end
```

Result:

```
POST /svc/ops/restart              → folder guard runs, needs Bearer
POST /svc/ops/inject-fault/trigger → override guard runs, always 403 (folder guard skipped)
```

## Order of framework checks

Guards run AFTER method-allow-list, CSRF, and If-Match enforcement. See [Request pipeline](../framework/pipeline.md).
