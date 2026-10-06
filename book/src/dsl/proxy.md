# Pass-through proxy routes (issue #134)

A route declared with `declaration.proxy:` is a streaming
byte-identical HTTP proxy to a configured upstream. The DSL body is
empty — the router forwards the client's bytes to the upstream
without parsing, without decoding, and without materialising any body
view inside the DSL.

This is the mode you want when:

- You need to forward opaque binary payloads unchanged (AS4 /
  eDelivery, SOAP with MTOM attachments, raw file uploads where the
  upstream owns the content contract).
- The request's digital signature covers the exact bytes and must
  survive forwarding (any re-serialisation would invalidate the
  signature).
- Validation and content-shape enforcement live upstream, not in
  Ruuter.

If your route is JSON-oriented and only needs to forward with a
header tweak or a conditional branch, use a regular DSL with an
`http.*` step instead — the proxy route has no step pipeline.

## Minimal shape

```yaml
declaration:
  description: "AS4 edge proxy to the national eDelivery AP."
  proxy:
    upstream: "[#EDELIVERY_URL]/ws/ap"
    max_body_bytes: 67108864
```

That's it. The DSL file has no `assign:` / `switch:` / `return:` —
only the declaration. Add a `.guard.yml` next to it in the usual way
to run authentication (checking an mTLS-forwarded identity header,
API key, etc.) before anything is forwarded.

## Fields

| Field | Required | Default | Meaning |
|---|---|---|---|
| `upstream` | yes | — | Absolute URL to which every request is forwarded. Constants (`[#NAME]`) are interpolated at parse time. SSRF checks apply on every request. |
| `max_body_bytes` | yes | — | Per-route inbound body cap in bytes. Declared Content-Length over the cap is rejected with 413 before any body is read; chunked bodies are counted mid-stream. No implicit cap for proxy routes — the 16 MiB global cap is bypassed. |
| `preserve_headers` | no | `true` | Forward the client's headers (minus hop-by-hop per RFC 7230 §6.1). Set `false` to forward only `Content-Type` + `Content-Length`. |
| `max_in_flight` | no | `32` | Per-route Semaphore cap. Overflow returns 503 + `Retry-After: 1`. Set `null` to disable (not recommended on public listeners). |
| `inbound_progress_timeout_ms` | no | `10000` | Idle-frame timeout on the inbound body stream, in milliseconds. Mitigates slowloris. |
| `allowed_encodings` | no | `["identity"]` | Permitted values on the inbound `Content-Encoding` header. Operators opt into `gzip`/`br`/`zstd` per route (safe-default pattern). |
| `request_timeout_ms` | no | `60000` | End-to-end deadline for the proxied request (connect + send + first byte). Does not include time spent in the concurrency-cap Semaphore. |

## Hop-by-hop handling

RFC 7230 §6.1 names a fixed set of hop-by-hop headers that MUST NOT
be forwarded by an intermediary. Ruuter strips them on both legs:

- **Request leg**: `Connection`, `Keep-Alive`, `Proxy-Authenticate`,
  `Proxy-Authorization`, `TE`, `Trailer`, `Transfer-Encoding`,
  `Upgrade`, `Host`, `Expect`, plus any header named in the client's
  `Connection:` list (dynamic hop-by-hop).
- **Response leg**: same list minus `Host` and `Expect` (irrelevant
  on responses), plus any header named in the upstream's `Connection:`
  list.

Everything else forwards verbatim, including custom auth tokens,
tracing headers, and the request's `Content-Type` with its multipart
boundary intact.

`traceparent` is end-to-end (not hop-by-hop) and is forwarded on
every request. If the client didn't send one, Ruuter generates a
fresh value for the request-scoped span and forwards that.

## What Ruuter still checks on a proxy route

Pass-through does NOT mean "no checks." The HTTP-layer security
floor still runs:

1. **Method allow-list** (`incoming_requests.allowed_method_types`).
2. **CSRF** when `csrf.allowed_origins` is configured.
3. **Guards** (`.guard.yml` files) — run against a header-only
   `ExecutionContext` (bodies are not parsed). Guards that reference
   `${incoming.body…}` see an empty object on a proxy route; use
   headers / params for authentication.
4. **Content-Length preflight** against the route's `max_body_bytes`.
5. **Content-Encoding allowlist** (`allowed_encodings`).
6. **SSRF** on the upstream URL — same gate as `http.*` steps
   (allowlist, private-network block, DNS-rebinding close).
7. **Per-route concurrency cap** via Semaphore.
8. **Inbound idle-frame timeout**.
9. **Mid-stream body size cap** on both legs. Response leg honours
   `http_response_size_limit`.

What Ruuter does NOT do on a proxy route:

- Parse the request body under `incoming.body` (opaque bytes stay
  opaque).
- Decode the response body under `${result}` (streams back to the
  client unchanged).
- Apply `declaration.allowlist.body` (parse-time error — see
  "Parse-time errors" below).
- Decompress `Content-Encoding`'d bodies. Compressed bytes pass
  through as-is to the upstream; the upstream decides what to do
  with them.

## Parse-time errors

The following combinations are rejected at DSL load time:

- `declaration.proxy:` AND `declaration.allowlist.body:` → error.
  Proxying is byte-identical; a body allowlist would require parsing
  the body (defeats the contract and introduces a parser attack
  surface). Drop the body allowlist; upstream validates content
  shape.
- `declaration.proxy:` AND `declaration.allowed_body:` → error (same
  reason, legacy flat form).
- `declaration.proxy:` AND any action step in the DSL body → error.
  Steps after a proxy declaration are unreachable; the operator
  almost certainly made a mistake.
- `declaration.proxy.upstream:` empty → error.
- `declaration.proxy.max_body_bytes: 0` → error.
- `declaration.proxy.allowed_encodings:` containing an unknown
  encoding → error. Known: `identity`, `gzip`, `deflate`, `br`,
  `zstd`.

Guards declared on a proxy route can declare `allowlist.headers` and
`required_one_of.headers` normally — those are the main authentication
surface on a proxy route.

## Error responses

Ruuter-owned gates return structured JSON bodies with stable `error`
field names so clients can branch:

| Status | `error` field | When it fires |
|---|---|---|
| 413 | `proxy_body_too_large` | Declared Content-Length exceeds `max_body_bytes` (or body exceeds cap mid-stream). |
| 415 | `proxy_unsupported_encoding` | `Content-Encoding` not in `allowed_encodings`. |
| 502 | `proxy_upstream_rejected` | SSRF / allowlist denial / malformed upstream URL. |
| 502 | `proxy_transport_error` | Connect refused, DNS failure, TLS handshake failure, or upstream deadline exceeded (`kind` names the class: `timeout`, `connect`, `request`, `body`, `decode`, `unknown`). |
| 503 | `proxy_capacity_exceeded` | Per-route Semaphore cap reached. Response includes `Retry-After: 1`. |

Upstream status codes are forwarded verbatim — a proxy must not
re-map legitimate 4xx / 5xx responses from the upstream.

## Known caveats (day one)

Two shapes are intentionally not supported in the initial release:

- **`Expect: 100-continue`** is not forward-and-relayed. hyper's
  server-side auto-ack is active on inbound; the upstream's own 100
  (or 417) is handled by reqwest internally but not relayed to the
  client. The AS4 / eDelivery traffic that motivated the feature
  does not use `Expect: 100-continue`.
- **HTTP/1.1 trailers** (headers after the body in chunked
  encoding) are not forwarded. The underlying stream wrappers
  (`axum::body::Body::from_stream`, `reqwest::Body::wrap_stream`)
  carry data frames only.

Both are tracked as follow-up work. If your deployment needs either,
open an issue naming the use case.

## Example: eFTI AS4 edge proxy

```yaml
declaration:
  description: >
    AS4 edge proxy — forwards every request from a peer gate to the
    Klite eDelivery access point, byte-identical. Peer identity is
    verified by the sibling guard against the mTLS-DN header set by
    the envoy terminator.
  proxy:
    upstream: "[#EDELIVERY_URL]/ws/ap"
    preserve_headers: true
    max_body_bytes: 134217728   # 128 MiB — AS4 messages w/ attachments
    max_in_flight: 16
    inbound_progress_timeout_ms: 15000
    allowed_encodings: ["identity", "gzip"]
    request_timeout_ms: 60000
```

The sibling `.guard.yml` would declare the peer-authentication
contract:

```yaml
declaration:
  description: "Peer must present an mTLS DN we recognise."
  allowlist:
    headers:
      - field: x-client-cert-dn
    required_one_of:
      headers:
        - [x-client-cert-dn]
peer_check:
  switch:
    - condition: "${incoming.headers['x-client-cert-dn'] === null}"
      next: deny
  next: end
deny:
  return:
    status: 401
    body: { "error": "peer not authenticated" }
    wrapper: false
end:
  return: {}
```

## Process-wide config

The top-level `pass_through_proxy:` block in `ruuter.yaml` tunes the
dedicated reqwest client pool used by every proxy route:

```yaml
pass_through_proxy:
  pool_max_idle_per_host: 32
  pool_idle_timeout_ms: 90000
  connect_timeout_ms: 10000
```

Defaults as shown. Independent of the `http.*` step's reqwest client
pool so a saturated proxy workload cannot starve normal outbound
traffic.
