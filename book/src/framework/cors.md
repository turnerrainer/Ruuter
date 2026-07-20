# CORS

Cross-Origin Resource Sharing headers. Added by a `tower-http` CORS layer when configured.

## Configuration

```yaml
cors:
  allowed_origins: []              # empty = no CORS layer attached
  allow_credentials: false
```

- `allowed_origins` **empty** → no CORS layer at all. Same-origin requests work; cross-origin browser requests are rejected by the browser (no `Access-Control-Allow-Origin` header sent).
- `allowed_origins` **non-empty** → each listed origin exactly matched against the request's `Origin` header.

## What the layer permits

When configured:

- Methods: `GET, POST, PUT, PATCH, DELETE, OPTIONS`
- Headers: `content-type, authorization, if-match, traceparent`
- Credentials: per `allow_credentials`

## Verification

```yaml
cors:
  allowed_origins: ["https://ui.example.com"]
```

```
$ curl -sSD - -H 'Origin: https://ui.example.com' http://localhost:8080/svc/data | grep -i access-control
access-control-allow-origin: https://ui.example.com
```

## Not on the /_/ endpoints

`/health`, `/_/openapi.json`, `/_/sources` are same-origin-only in practice — the CORS layer applies uniformly, but these endpoints are typically consumed via same-origin tooling.
