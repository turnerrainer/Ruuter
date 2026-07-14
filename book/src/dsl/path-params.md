# Path parameters

One DSL file serves multiple URL shapes. Trailing URL segments that don't match on their own are stripped and prepended to `incoming.params.pathParams`.

## Resolution

```
DSL/svc/GET/things.yml    served by GET /svc/things
                                    GET /svc/things/anything
                                    GET /svc/things/anything/deeper
```

| Request                             | pathParams              |
|-------------------------------------|-------------------------|
| `GET /svc/things`                   | `[]`                    |
| `GET /svc/things/abc-123`           | `["abc-123"]`           |
| `GET /svc/things/abc-123/legs`      | `["abc-123", "legs"]`   |

Order matches the URL (leftmost stripped segment first).

## Specificity

Exact-match wins over path-param fallback. If both `things.yml` and `things/legs.yml` exist, `GET /svc/things/legs` hits `things/legs.yml` and `pathParams` is `[]`.

## Verified example

```yaml
# DSL/samples/GET/things.yml
route:
  switch:
    - condition: "${incoming.params.pathParams.length === 0}"
      next: list
    - condition: "${incoming.params.pathParams.length === 1}"
      next: detail
  next: sub
list:   { return: { mode: "list", items: ["a","b","c"] }, next: end }
detail: { return: { mode: "detail", id: "${incoming.params.pathParams[0]}" }, next: end }
sub:    { return: { mode: "sub", id: "${incoming.params.pathParams[0]}", subresource: "${incoming.params.pathParams[1]}" }, next: end }
```

```
$ curl http://localhost:8080/samples/things
{"items":["a","b","c"],"mode":"list"}

$ curl http://localhost:8080/samples/things/abc-123
{"id":"abc-123","mode":"detail"}

$ curl http://localhost:8080/samples/things/abc-123/legs
{"id":"abc-123","mode":"sub","subresource":"legs"}
```

## Cross-project isolation

Path-param lookup never crosses project boundaries. A missing route in project `b` does not fall back to a matching prefix in project `a`.
