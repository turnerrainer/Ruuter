# template

Call another DSL in the same project as if it were an HTTP endpoint.

```yaml
fetch:
  template: templates/user-profile   # project-relative path, no extension
  request_type: GET                  # default: GET
  body:    { name: "alice" }         # sets callee's incoming.body
  query:   { verbose: "1" }          # sets callee's incoming.query
  headers: { X-Trace: "yes" }        # sets callee's incoming.headers
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
- **Guards**: NOT re-applied. The template call bypasses guards that would fire on a real HTTP request to the same path.
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

```console
# Happy path
$ curl -sX POST http://localhost:8080/samples/templates/call-create-template \
    -H 'Content-Type: application/json' \
    -d '{"name":"widget","type":"gadget"}' | jq .
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

# Guard branch fires — template never runs
$ curl -sX POST http://localhost:8080/samples/templates/call-create-template \
    -H 'Content-Type: application/json' -d '{}'
{"error":"Name is required"}
```

Notice the callee's response is wrapped as
`${result_name}.response.{status,body,headers}` — same shape as the
[`http` step](./http.md).
