# Status codes emitted by the framework

Statuses Ruuter itself can emit BEFORE reaching your DSL, or on DSL-execution failure:

| Status | When |
|--------|------|
| `400` | Malformed JSON body OR body over 16 MiB |
| `403` | CSRF Origin/Referer check failed |
| `404` | Route doesn't exist, or DSL threw `FileNotFound` |
| `405` | Method not in `incoming_requests.allowed_method_types` |
| `428` | `optimistic_concurrency.require_if_match: true` and `If-Match` missing |
| `500` | Any DSL step error (script eval, SSRF rejection, size cap, upstream status filter, template not found, ws_send target missing, etc.) |

## Statuses your DSL can emit

Anything, via `return.status`. The framework validates only that the value fits in a `u16`; invalid values fall back to `200`.

## Status inference for OpenAPI

The generator scans `return.status` literals across all steps in a DSL. Non-literal (JS-expression) statuses can't be resolved statically, so the operation defaults to `200`.

Every operation also carries `400` (framework can emit for malformed body) and `500` (catch-all) in its documented responses.
