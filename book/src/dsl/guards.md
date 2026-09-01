# Guards

Pre-execution DSLs that run before the main route. A guard returning HTTP status ≥ 400 short-circuits — its response becomes the response.

## Three file conventions

### Sibling — `<stem>.guard.yml`

Protects the same-name DSL AND every DSL under a same-name folder. Both `protected.yml` (same directory) and `protected/*` (child folder) are covered.

```
DSL/svc/GET/protected.guard.yml     # guards protected.yml (if any) + everything under protected/
DSL/svc/GET/protected.yml           # ← protected (same-key DSL, issue #41)
DSL/svc/GET/protected/data.yml      # ← protected (child)
```

Special case — **per-endpoint guard**: if no `<stem>/` folder exists alongside, the sibling guard protects just the one same-name DSL:

```
DSL/svc/POST/v1/consignments.guard.yml   # guards ONLY consignments.yml
DSL/svc/POST/v1/consignments.yml         # ← protected (no consignments/ folder)
```

### In-folder — `.guard.yml` inside the protected folder

```
DSL/svc/GET/vault/.guard.yml        # guards every DSL under vault/
DSL/svc/GET/vault/secret.yml        # ← protected
```

Both file conventions produce the same guard key (`GET/protected` / `GET/vault`). Use whichever fits your tree layout.

Three filename variants of the in-folder / project-level guard are accepted at runtime:

| Name | When to use |
|---|---|
| `.guard.yml` | **Preferred.** Modern default. Editors give YAML syntax highlighting + LSP validation; `dsl-lint` and glob tooling match `*.yml`. |
| `.guard.yaml` | Same as above but with the `.yaml` extension if that's your project convention. |
| `.guard` | Bare, no extension. Accepted for strict Java-Ruuter parity. Editors lose YAML syntax highlighting. Use only when porting a Java tree unchanged. |

Same-name variants in the same folder (e.g. both `.guard` and `.guard.yml`, or a sibling `foo.guard.yml` and an in-folder `foo/.guard.yml`) currently produce identical keys and one silently overwrites the other — pick one variant per key. A load-time collision error mirroring the project-level check is on the roadmap.

### Sibling guards are name-scoped, not directory-scoped

Common trap: a `.guard.yml` file protects a **name**, not "everything in this directory". Consider:

```
DSL/api/POST/foo.guard.yml          # protects foo (same-key + children of foo/)
DSL/api/POST/foo.yml
DSL/api/POST/another.guard.yml      # protects another (same-key + children of another/)
DSL/api/POST/another.yml
DSL/api/POST/is_this_unguarded.yml  # ← unguarded! No matching guard.
```

`is_this_unguarded.yml` is genuinely **unguarded** — neither `foo.guard.yml` nor `another.guard.yml` covers it (their keys are `POST/foo` and `POST/another`; the peer's key is `POST/is_this_unguarded`, which matches neither). To guard the peer:

- **Add an in-folder guard**: `DSL/api/POST/.guard.yml` protects every DSL in the folder. `foo.guard.yml` and `another.guard.yml` then stack on top.
- **Add a per-endpoint sibling**: `DSL/api/POST/is_this_unguarded.guard.yml` protects just that route.
- **Add a project-level guard** (issue #39): `DSL/api/.guard.yml` protects every route in the project.

### Project-level — `.guard.yml` at the project root (issue #39)

```
DSL/svc/.guard.yml                  # guards every HTTP DSL in svc
DSL/svc/GET/ping.yml                # ← protected
DSL/svc/POST/orders.yml             # ← protected
DSL/svc/DELETE/things.yml           # ← protected
```

Applies to every HTTP method in the project. Use this for cross-method authorisation logic that would otherwise duplicate across `GET/.guard.yml`, `POST/.guard.yml`, etc.

WebSocket inbound handlers (`WS/inbound/...`) are **not** currently gated by guards — the guard chain fires on the HTTP `execute_dsl` path only. Guarding a WS upgrade is a separate concern and lives on its future roadmap; do not rely on `<project>/.guard.yml` to authorise WS connections.

Runs as the outermost guard: project → method-root → path-ancestor → target. A nested guard with `declaration.override_ancestors: true` still bypasses the project-level guard for its subtree (see [Override](#override--replace-ancestors) below) — that's your escape hatch for public endpoints under an otherwise-protected project.

Only one project-level guard per project. Having both `.guard.yml` and `.guard.yaml` (or `.guard` and `.guard.yml`) at the project root is a load-time error; the message names both files so you can remove one.

`override_ancestors: true` on a project-level guard has no meaning (nothing outside it to override) — the loader WARNs and ignores the flag.

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

## Runnable example — project-level guard (issue #39)

Three files ship under `DSL/guarded-demo/`. One `.guard.yml` at the
project root protects both a GET and a POST endpoint — no per-method
duplication.

`DSL/guarded-demo/.guard.yml`:

```yaml
check:
  switch:
    - condition: "${incoming.headers['x-api-key'] !== 'demo-secret'}"
      next: deny
  next: allow

allow:
  return:
    passed: true
  next: end

deny:
  status: 401
  return:
    error: "guarded-demo: missing or invalid X-Api-Key header"
  next: end
```

`DSL/guarded-demo/GET/status.yml`:

```yaml
respond:
  return:
    status: ok
    method: GET
  next: end
```

`DSL/guarded-demo/POST/echo.yml`:

```yaml
respond:
  return:
    method: POST
    echoed: "${incoming.body}"
  next: end
```

Request — GET without the header, project guard rejects:

```bash
curl -si http://localhost:8080/guarded-demo/status
```

Response:

```
HTTP/1.1 401 Unauthorized
```

```json
{"error":"guarded-demo: missing or invalid X-Api-Key header"}
```

Request — POST without the header, same guard fires (no duplicate
per-method file):

```bash
curl -si -X POST http://localhost:8080/guarded-demo/echo \
     -H 'Content-Type: application/json' -d '{"n":1}'
```

Response:

```
HTTP/1.1 401 Unauthorized
```

```json
{"error":"guarded-demo: missing or invalid X-Api-Key header"}
```

Request — GET with the header, guard passes:

```bash
curl -si -H 'X-Api-Key: demo-secret' http://localhost:8080/guarded-demo/status
```

Response:

```
HTTP/1.1 200 OK
```

```json
{"method":"GET","status":"ok"}
```

## Runnable example — in-folder guard

Both files ship under `DSL/samples/GET/vault/`.

`DSL/samples/GET/vault/.guard.yml`:

```yaml
check:
  switch:
    - condition: "${!incoming.headers['x-vault-token']}"
      next: deny
  next: ok

ok:
  return:
    passed: true
  next: end

deny:
  status: 401
  return:
    error: "vault: missing x-vault-token header"
  next: end
```

`DSL/samples/GET/vault/secret.yml`:

```yaml
respond:
  return:
    secret: "42"
  next: end
```

Request — missing header, guard rejects:

```bash
curl -si http://localhost:8080/samples/vault/secret
```

Response:

```
HTTP/1.1 401 Unauthorized
```

```json
{"error":"vault: missing x-vault-token header"}
```

Request — header present, guard passes, main DSL runs:

```bash
curl -si -H 'X-Vault-Token: hunter2' http://localhost:8080/samples/vault/secret
```

Response:

```
HTTP/1.1 200 OK
```

```json
{"secret":"42"}
```

## Stacking

Multiple ancestor guards on the same path stack. **All** must pass. Outermost runs first.

```
.guard.yml                    # project-level, runs first (issue #39)
POST/api.guard.yml            # runs second
POST/api/admin.guard.yml      # runs third
POST/api/admin/users.yml      # main DSL, runs if every guard passed
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

## Runnable example — folder guard + override

Three files under `DSL/samples/POST/ops/`.

`DSL/samples/POST/ops/.guard.yml` — folder guard, requires Bearer:

```yaml
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
    error: "ops: bearer token required"
  next: end
```

`DSL/samples/POST/ops/restart.yml` — protected by the folder guard:

```yaml
respond:
  return:
    restarted: true
  next: end
```

`DSL/samples/POST/ops/inject-fault.guard.yml` — override guard that
replaces the folder guard for its subtree:

```yaml
declaration:
  override_ancestors: true

deny:
  status: 403
  return:
    error: "inject-fault is disabled in production"
  next: end
```

`DSL/samples/POST/ops/inject-fault/trigger.yml` — main DSL that is
now unreachable, because the override always denies:

```yaml
respond:
  return:
    fired: true
  next: end
```

Request — restart without Authorization: folder guard rejects:

```bash
curl -si -X POST http://localhost:8080/samples/ops/restart \
     -H 'Content-Type: application/json' -d '{}'
```

Response:

```
HTTP/1.1 401 Unauthorized
```

```json
{"error":"ops: bearer token required"}
```

Request — restart WITH Authorization: folder guard passes, main
DSL runs:

```bash
curl -si -X POST http://localhost:8080/samples/ops/restart \
     -H 'Authorization: Bearer abc' \
     -H 'Content-Type: application/json' -d '{}'
```

Response:

```
HTTP/1.1 200 OK
```

```json
{"restarted":true}
```

Request — inject-fault WITH Authorization: override guard runs
instead of the folder one and still denies:

```bash
curl -si -X POST http://localhost:8080/samples/ops/inject-fault/trigger \
     -H 'Authorization: Bearer abc' \
     -H 'Content-Type: application/json' -d '{}'
```

Response:

```
HTTP/1.1 403 Forbidden
```

```json
{"error":"inject-fault is disabled in production"}
```

## Order of framework checks

Guards run AFTER method-allow-list, CSRF, and If-Match enforcement. See [Request pipeline](../framework/pipeline.md).
