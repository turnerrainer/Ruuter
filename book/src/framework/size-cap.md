# Response size cap (outbound)

Bounds the memory an [`http` step](../dsl/steps/http.md) will read from an upstream response.

## Configuration

```yaml
http_response_size_limit: 16777216       # bytes; default 16 MiB
```

## Enforcement

Per response:

1. If the upstream sends `Content-Length` > cap → reject upfront.
2. Otherwise stream and tally bytes; abort if the accumulated body exceeds the cap.

Rejection surfaces as an `http` step error → `500` to the caller with `{"error": "HTTP request rejected: upstream response body ... exceeds http_response_size_limit ..."}`.

## Choosing a value

- Default 16 MiB is generous for JSON APIs.
- Set lower (e.g. 1 MiB) if you know your upstreams should never send more.
- Set to `null` to disable — Ruuter will read whatever the upstream sends. Not recommended for production.

## Not the same as inbound

Inbound request bodies are always capped at **16 MiB** (hardcoded). Requests over that get `400 Bad Request` with a body-read error before the DSL runs.
