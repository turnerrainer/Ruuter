# CSRF (Origin / Referer allow-list)

Framework-level protection against cross-site state-changing requests.

## Configuration

```yaml
csrf:
  allowed_origins: []                        # empty = check BYPASSED
  enforce_on_methods: [POST, PUT, PATCH, DELETE]
```

## Semantics

- `allowed_origins` **empty** → check is skipped entirely. Same-origin admin surfaces behind a reverse proxy that already enforces same-origin (or relies on `SameSite=Strict` cookies) don't need this.
- `allowed_origins` **non-empty** AND method ∈ `enforce_on_methods` → the request must present an `Origin` header (or fall back to a `Referer` origin) matching one of the allow-listed strings.
- No matching header → `403 Forbidden`.

## Verification

```yaml
csrf:
  allowed_origins: ["https://admin.example.com"]
```

Request — disallowed origin:

```bash
curl -sS -w "\nHTTP %{http_code}\n" -X POST \
     -H 'Origin: https://evil.example.com' \
     http://localhost:8080/svc/action
```

Response:

```
{"error":"CSRF: origin not allowed"}
HTTP 403
```

Request — allowlisted origin:

```bash
curl -sS -w "\nHTTP %{http_code}\n" -X POST \
     -H 'Origin: https://admin.example.com' \
     http://localhost:8080/svc/action
```

Response:

```
{"ok":true}
HTTP 200
```

## Compared to CORS

CORS controls which cross-origin **browsers** are allowed to READ responses. CSRF controls which cross-origin browsers are allowed to WRITE. Configure both — [CORS](./cors.md) and CSRF — for a browser-facing admin UI.
