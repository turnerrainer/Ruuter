# Method allow-list

Restrict which HTTP methods the framework will accept.

## Configuration

```yaml
incoming_requests:
  allowed_method_types: [GET, POST, PUT, PATCH, DELETE, OPTIONS]
```

## Enforcement

Runs BEFORE routing. Method not in the list → `405 Method Not Allowed` with `{"error": "Method Not Allowed"}`.

Case-insensitive match.

## Use cases

- Lock a read-only mirror to `[GET]`.
- Block `OPTIONS` if you're not doing CORS.
- Emergency read-only mode without DSL edits — set `[GET]` at runtime and restart.

## Verified

```yaml
incoming_requests:
  allowed_method_types: [GET]
```

Request:

```bash
curl -sSD - -X POST http://localhost:8080/svc/anything | head -3
```

Response:

```http
HTTP/1.1 405 Method Not Allowed
content-type: application/json
```
