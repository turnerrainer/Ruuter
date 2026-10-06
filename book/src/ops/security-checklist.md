# Security hardening checklist

Review before every partner-facing deploy.

## Framework

- [ ] `csrf.allowed_origins` set to the exact set of browser origins that can POST/PUT/PATCH/DELETE — even if you rely on `SameSite=Strict` cookies.
- [ ] `cors.allowed_origins` set to the same list (if you have a browser UI).
- [ ] `internal_requests.allowed_urls` set. Default is unrestricted outbound — that's SSRF territory. At minimum, prefix-lock to your trusted upstream domains.
- [ ] `http_response_size_limit` set to a value < your process memory. Default 16 MiB is fine unless upstreams are known bounded.
- [ ] `http_codes_allow_list` set if you want strict outcome control (e.g. `[200, 201, 202, 204]`).
- [ ] `incoming_requests.allowed_method_types` narrowed if some verbs are never expected (e.g. drop OPTIONS if not doing CORS).
- [ ] `optimistic_concurrency.require_if_match: true` if your DSLs implement ETag validation and you want to reject naive clients at the door.
- [ ] `response_default_headers` includes at minimum: `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Strict-Transport-Security` (if behind HTTPS).

## Container

- [ ] `read_only: true` (default in shipped compose)
- [ ] `no-new-privileges:true` (default)
- [ ] `cap_drop: [ALL]` (default)
- [ ] Memory + CPU limits (default 512 M / 2 CPU)
- [ ] `tini` as PID 1 (default)
- [ ] Non-root user (default uid 1000)
- [ ] Constants and DSLs mounted read-only

## Secrets

- [ ] `constants.ini` contains no plaintext secrets that shouldn't be on disk. Vault-agent-rendered, Docker secret, or KMS-decrypted at deploy time.
- [ ] No secret values in `ruuter.yaml` (config is checked into git).
- [ ] `constants.ini` file mode `0400` (owner-read only).

## Network

- [ ] Only port 8080 published, behind a TLS-terminating reverse proxy.
- [ ] `X-Forwarded-For` handling is at the reverse proxy — Ruuter uses this header only for the `request_origin` context string (informational, not for auth).
- [ ] Outbound egress firewalled to the domains in `internal_requests.allowed_urls`.

## Observability

- [ ] `OTEL_EXPORTER_OTLP_ENDPOINT` configured to point at your collector.
- [ ] Log aggregation captures stderr (JSON via `tracing`).
- [ ] `traceparent` propagation verified end-to-end (curl a route, check the response's `x-trace-id` matches what your collector received).

## DSLs

- [ ] Every guard returns explicit 4xx status on reject (not a bare `return: { error: ... }` that would 200).
- [ ] No DSL uses `${incoming.body.url}` (or similar) as an `http` step URL without an SSRF allow-list.
- [ ] Idempotency-Key semantics understood by clients writing to POST/PUT/PATCH/DELETE routes.

## Pass-through proxy routes (issue #134)

Reviewed if any route in the loaded tree uses `declaration.proxy:`.
See [Pass-through proxy routes](../dsl/proxy.md) for the full
contract.

- [ ] **16 MiB global cap bypassed on proxy routes.** The framework-wide
  Content-Length preflight does not apply to proxy routes — the
  per-route `declaration.proxy.max_body_bytes` is authoritative.
  Set it explicitly to the maximum message size you expect
  (typically 64–128 MiB for AS4 / eDelivery). An unset value is a
  parse-time error, so this is enforced at boot.
- [ ] **`max_in_flight` set** on every proxy route exposed on a
  non-loopback listener. Default 32; raise or lower per deployment.
  `null` disables the cap and risks resource exhaustion under
  flood.
- [ ] **`allowed_encodings`** reviewed. Default `["identity"]` —
  compressed bodies get 415. Operators who need to pass gzip
  bodies to the upstream opt in explicitly per route (compressed
  bytes pass through unchanged — Ruuter never decompresses).
- [ ] **`inbound_progress_timeout_ms`** set. Default 10 000 (10 s)
  idle-frame timeout. Mitigates slowloris on long bodies.
- [ ] **`request_timeout_ms`** set. Default 60 000 (matches AS4
  budgets).
- [ ] **Guard authenticates on headers, not body.** `incoming.body`
  is always empty on proxy routes. The guard must check an mTLS
  DN header set by the TLS terminator, an API key, or similar.
- [ ] **TLS termination is in front of Ruuter.** Ruuter is HTTP-
  only; mTLS peer identity is forwarded as a header set by the
  terminator (envoy / nginx / k8s ingress). The upstream service
  does AS4 content validation; Ruuter does HTTP-layer sanity
  only.
- [ ] **Pool tuning reviewed in `pass_through_proxy:`.** Separate
  from the `http.*` client pool so a saturated proxy workload
  cannot starve normal outbound traffic. See
  [Pass-through proxy config](../config/pass-through-proxy.md).
