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

## Fan-out — `parallel_http:` (issues #135 + #136)

Reviewed if any DSL uses the [`parallel_http` step](../dsl/steps/parallel_http.md).

- [ ] **`max_concurrency` set** on every proxy fan-out that serves
  public traffic. The default (unbounded) is fine for internal
  admin DSLs but a public route that fires `parallel_http` to a 60-
  peer registry without a cap turns one inbound request into 60
  outbound sockets. Pick a modest value (8 — 32) and raise under
  load.
- [ ] **`timeout` set per peer.** Default inherits
  `http_request_timeout` (15 s). For peer-gate workloads where
  stragglers are expected, choose a tighter value — the step's
  tail latency is bounded by the slowest peer under `collect_ok` /
  `collect_all`.
- [ ] **Peer URLs validated.** Peers often come from a Postgres
  table or config file; if the DSL doesn't own the source, treat
  `${peer.url}` as user-controlled and keep `block_private_networks:
  true` + an SSRF allowlist on at least one of the peer origins.
- [ ] **`first_n.body_predicate` is defence-in-depth, not a
  security control.** A compromised peer can return any body it
  wants; the predicate just picks "which 2xx counts." Don't rely
  on it to authenticate peers.

## Background execution — `detach:` (issue #137)

Reviewed if any DSL uses the [`detach:` step](../dsl/steps/detach.md).

- [ ] **`detach.max_inflight` set** to a numeric value on any
  non-loopback deployment. `null` disables the per-process cap on
  concurrent detached tasks — one inbound request can spawn an
  unbounded number of them. Default `256`; raise under load but set
  a real number.
- [ ] **`detach.shutdown_grace_secs` matches your deploy pattern.**
  Default `15` s. On a rolling deploy, detached tasks that take
  longer than this get `abort_all()`'d — any Postgres writes they
  were about to make are lost. If your eFTI-shaped fan-out routinely
  takes 60 s, raise this (and accept the longer drain window).
- [ ] **No `incoming.*` dereference in a detached task assumes
  per-request state writes.** The detached task has a snapshot of
  the parent's context — writes made inside `do:` do NOT propagate
  back to the parent. If you need the parent to see a detached
  task's result, write to the state store or Postgres, not to a
  DSL variable.
- [ ] **No long-held upstream requests inside `detach.do:` that
  would survive the SIGTERM grace window.** Those are the ones that
  get aborted mid-flight on redeploy. If "survive restart" is a
  requirement, use a work-queue decoupling (NATS JetStream, Kafka,
  Postgres LISTEN+NOTIFY) instead of `detach:`.
