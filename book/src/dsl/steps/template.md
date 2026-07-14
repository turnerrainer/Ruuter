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

## Verified example

```yaml
# DSL/svc/GET/templates/pong.yml
respond: { return: { pong: true }, status: 200, next: end }

# DSL/svc/GET/ping.yml
call:  { template: templates/pong, result: r, next: shape }
shape: { return: { echoed: "${r.response.body.pong}" }, next: end }
```

`GET /svc/ping` → `{"echoed": true}`.
