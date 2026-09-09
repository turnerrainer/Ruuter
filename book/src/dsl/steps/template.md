# template

Call another DSL in the same project as if it were an HTTP endpoint.

```yaml
fetch:
  template: templates/user-profile   # project-relative path, no extension
  request_type: GET                  # default: GET
  body:                              # sets callee's incoming.body
    name: "alice"
  query:                             # sets callee's incoming.query
    verbose: "1"
  headers:                           # sets callee's incoming.headers
    X-Trace: "yes"
  result: profile                    # binds .response.{status,body,headers}
  next: reply
```

## Resolution

Target = `DSL/<current-project>/<request_type>/<template>.yml`.

Missing target → step error. Wrong `request_type` (target doesn't exist under that verb) → step error.

## Result shape

Identical to the [`http` step](./http.md):

```json
{
  "response": {
    "status":  200,
    "body":    <whatever the template returned>,
    "headers": { ... }
  }
}
```

## Shared vs isolated state

- **State store**: shared with caller (same project, same DashMap).
- **Traceparent**: forwarded from caller.
- **Guards**: **re-applied** against the child context (since v0.9.11-rc, h2ck.me H1 / PR #72). A `template:` step to a guarded route runs every applicable guard on the callee's `<METHOD>/<path>` key before dispatching the body; a guard returning `>= 400` short-circuits and the caller's `${result}` binds the guard's response. Forward auth headers explicitly via `template.headers:` if the target route depends on them.
- **Guard recursion** (issue #79): guards that themselves contain a `template:` step no longer loop. The router tracks guard keys currently mid-execution on the `ExecutionContext` and filters the target's applicable-guard list against that stack, so `guard → template → same-guard` cycles break at the second step. A hard cap at `MAX_GUARD_DEPTH = 32` surfaces exotic mutual-recursion patterns as a clean `DslExecution` error instead of a stack overflow.
- **Local variables**: NOT shared. The callee starts with a fresh variable context; only the values you pass via `body:`/`query:`/`headers:` reach it.

## Runnable example

Two files: the reusable template plus the caller.

`DSL/samples/POST/templates/create-entity.yml` — reusable template
that wraps `incoming.body` in metadata:

```yaml
prepare_entity:
  assign:
    entity:
      data: ${incoming.body}
      metadata:
        created_at: ${Date.now()}
        created_by: "system"
        version: 1
  next: respond

respond:
  status: 201
  return: ${entity}
  next: end
```

`DSL/samples/POST/templates/call-create-template.yml` — validates
input then delegates:

```yaml
validate_input:
  switch:
    - condition: ${!incoming.body.name}
      next: missing_name
  next: call_template

call_template:
  template: "templates/create-entity"
  request_type: "POST"
  body:
    name: ${incoming.body.name}
    type: ${incoming.body.type || "default"}
  result: created_entity
  next: respond

respond:
  return:
    success: true
    entity: ${created_entity}
  next: end

missing_name:
  status: 400
  return:
    error: "Name is required"
  next: end
```

Request — happy path:

```bash
curl -sX POST http://localhost:8080/samples/templates/call-create-template \
     -H 'Content-Type: application/json' \
     -d '{"name":"widget","type":"gadget"}' | jq .
```

Response:

```json
{
  "entity": {
    "response": {
      "body": {
        "data": { "name": "widget", "type": "gadget" },
        "metadata": {
          "created_at": 1785079271978.0,
          "created_by": "system",
          "version": 1
        }
      },
      "headers": {},
      "status": 201
    }
  },
  "success": true
}
```

Request — validation branch fires; the template never runs:

```bash
curl -sX POST http://localhost:8080/samples/templates/call-create-template \
     -H 'Content-Type: application/json' -d '{}'
```

Response:

```json
{"error":"Name is required"}
```

Notice the callee's response is wrapped as
`${result_name}.response.{status,body,headers}` — same shape as the
[`http` step](./http.md).
