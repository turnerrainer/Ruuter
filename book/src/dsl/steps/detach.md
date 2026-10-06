# detach

Issue #137. Fire a block of DSL steps in a **background task** so the
parent DSL can respond immediately. The eFTI K4 pattern composes
`detach { parallel_http { ... } }` with a Resql Postgres write
(kemit-ee/efti-gate-ee#252) to answer X-Road callers in milliseconds
while a 60-second fan-out continues in the background.

```yaml
accept:
  detach:
    do:
      - parallel_http:
          peers: "${gates}"
          args:
            url: "${peer.baseUrl}/v1/send"
            body: "${incoming.body}"
          aggregate: collect_all
          max_concurrency: 16
          timeout: 60000
          result: peer_responses
      - call: http.post
        args:
          url: "[#RESQL_URL]/efti/write_search_responses"
          body:
            searchId: "${incoming.body.id}"
            peers: "${peer_responses}"
    timeout_ms: 90000
  next: respond_accepted

respond_accepted:
  return: { id: "${incoming.body.id}" }
  status: 202
```

Caller sees `202 Accepted` in ~20 ms. The `detach.do:` block
continues in a `tokio::spawn` task — `parallel_http` fans out,
waits for all 62 peer gates, then the Resql call writes the
structured array to Postgres. Caller polls a sibling route later
and reads from Postgres.

## Semantics

- **Parent continues immediately** to `next:` as soon as the
  `detach` step accepts the work. Caller's HTTP response is already
  on the wire before `do:` starts.
- **Sub-steps inside `do:` run sequentially**, in source order —
  same contract as `iterate.do:` and `single_flight.do:`. Each
  sub-step's `next:` directive is **ignored**; the block completes
  when the last sub-step finishes, when `timeout_ms` fires, or
  when a sub-step errors.
- **Context is snapshotted** at spawn time via
  `ExecutionContext::snapshot`. The detached task has a fresh
  variables map pre-filled with the parent's bindings; writes made
  inside `do:` **do not propagate** back to the parent. Readonly
  fields (`incoming.body`, `incoming.query`, `incoming.headers`,
  `project`, `traceparent`, `state`) are shared.
- **Errors inside `do:` are logged** with the parent's `traceparent`
  and never affect the parent's already-sent response. Each sub-step
  can still route its own error via its own `error:` handler.
- **Guards still run** when a sub-step is a `template:` call —
  same v0.9.11-rc H1 contract. Guard stack is fresh per detached
  task (the parent's stack is not inherited; the detached task is
  a new execution).

## Fields

### `do:` (required)

Non-empty list of DSL steps. Each entry is a bare step body (no
step-name wrapper — same shape as `iterate.do:`). Mixing step
types is fine. Common pattern:

```yaml
do:
  - parallel_http: { ... }
  - call: http.post
    args: { ... }
```

Empty `do:` is a parse-time error. `return:` inside `do:` is a
parse-time error (unreachable — the parent response is already
sent; see [Parse-time errors](#parse-time-errors) below).

### `timeout_ms:` (optional, milliseconds)

End-to-end deadline for the whole detached block. On timeout the
currently-running sub-step is cancelled and a WARN is logged with
the parent's traceparent. Absent → no timeout (bounded in practice
by the SIGTERM drain window + per-sub-step timeouts).

`timeout_ms: 0` is a parse-time error — zero is never a sensible
deadline; leave the field unset for "no timeout."

## Process-wide bounds (ruuter.yaml)

```yaml
detach:
  max_inflight: 256              # default; null = unbounded
  shutdown_grace_secs: 15        # default; SIGTERM drain window
```

### `detach.max_inflight`

Semaphore permits available across the whole process. Overflow
fails the `detach:` step with
`RuuterError::DslExecution { step: "detach", message: "detach registry full (cap: 256) ..." }`
so the DSL can route to `error:` or fall through. Default `256`.

`null` disables the cap (every `detach:` call spawns unconditionally).
Safe only on locked-down internal deployments. On a non-loopback
listener, Ruuter emits a boot WARN when `null` + public bind so
operators notice the exposure.

### `detach.shutdown_grace_secs`

How long the SIGTERM drain waits for in-flight detached tasks to
finish before `abort_all()`. Default `15` — same posture as
`SHUTDOWN_GRACE_SECS` on the axum / UDS drain paths (T-30). Tasks
still running past the window are **cancelled**; their partial
work is lost. For eFTI-shaped pipelines that write to Postgres on
every sub-step, this is usually fine — the aborted task leaves
whatever rows the Resql call managed to insert.

## Worked examples

### Example 1 — eFTI K4 pattern (compose with parallel_http + Resql)

Caller submits a search. Ruuter answers 202 immediately; the fan-
out to 62 peer gates runs in the background and writes to Postgres
when done. Caller polls a sibling route for the result.

```yaml
# POST /svc/searches — body: { id: "...", query: {...} }

init:
  state:
    set:
      key: "search:${incoming.body.id}"
      value: { status: "in_progress", submitted_at: "${new Date().toISOString()}" }
  next: fanout_async

fanout_async:
  detach:
    do:
      - parallel_http:
          peers: "${gates}"
          call: http.post
          args:
            url: "${peer.baseUrl}/v1/send"
            body: "${incoming.body}"
          aggregate: collect_all
          max_concurrency: 16
          timeout: 60000
          result: peer_responses
      - call: http.post
        args:
          url: "[#RESQL_URL]/efti/write_search_responses"
          body:
            searchId: "${incoming.body.id}"
            peers: "${peer_responses}"
    timeout_ms: 90000
  next: respond_accepted

respond_accepted:
  return:
    id: "${incoming.body.id}"
    status: "accepted"
    poll: "/svc/searches/${incoming.body.id}"
  status: 202
```

Companion poll route (not detached, synchronous):

```yaml
# GET /svc/searches/:id
check:
  call: http.get
  args:
    url: "[#RESQL_URL]/efti/read_search_status"
    query:
      id: "${incoming.params.id}"
  result: r
  next: reply

reply:
  return: "${r.response.body}"
  wrapper: false
  status: 200
```

### Example 2 — fire-and-forget audit event

Record every API call to an audit sink. Caller is not blocked by
the audit write.

```yaml
# POST /svc/payments
process:
  call: http.post
  args:
    url: "[#PAYMENTS_URL]/charge"
    body: "${incoming.body}"
  result: charge
  next: audit_async

audit_async:
  detach:
    do:
      - call: http.post
        args:
          url: "[#AUDIT_URL]/payments"
          body:
            request_id: "${incoming.headers['x-request-id']}"
            caller: "${incoming.headers['authorization']}"
            amount: "${incoming.body.amount}"
            status: "${charge.response.status}"
            charge_id: "${charge.response.body.id}"
  next: reply

reply:
  return: "${charge.response.body}"
  status: "${charge.response.status}"
```

A slow audit sink doesn't degrade the payment API. If the audit
sink is down for 24h, the operator sees it in logs — but the
customer-facing path keeps responding at the payment processor's
latency.

### Example 3 — async webhook fan-out

Receive a webhook; respond 200 to the webhook sender quickly;
notify N downstream services on our side in the background.

```yaml
webhook:
  detach:
    do:
      - parallel_http:
          peers: "${downstream_services}"
          call: http.post
          args:
            url: "${peer.webhook_url}"
            body: "${incoming.body}"
          aggregate: collect_ok
          max_concurrency: 8
          timeout: 5000
          result: _notifications
  next: ack

ack:
  return: { ok: true }
  status: 200
```

The webhook sender gets `200 OK` within a few milliseconds — most
webhook contracts time out their senders at 5-10 seconds, so
hitting them fast matters. Downstream fan-out proceeds under its
own `max_concurrency` cap.

### Example 4 — overflow handling with `error:`

Catch the overflow and fall back to a synchronous slow path.

```yaml
try_async:
  detach:
    do:
      - call: http.post
        args:
          url: "[#BACKGROUND_WORKER_URL]/enqueue"
          body: "${incoming.body}"
  next: respond_fast
  error: fallback_sync

respond_fast:
  return: { accepted: true, mode: "async" }
  status: 202

fallback_sync:
  call: http.post
  args:
    url: "[#WORKER_URL]/process_now"
    body: "${incoming.body}"
    timeout: 30000
  result: result
  next: respond_slow

respond_slow:
  return: "${result.response.body}"
  status: "${result.response.status}"
```

When `detach.max_inflight` is saturated, the step errors; the
DSL's `error: fallback_sync` catches it and processes the work
synchronously (slower but still gets done). Caller never sees a
500 just because the registry filled up.

### Example 5 — nested parallel_http inside detach (full K4)

The result array shape from `parallel_http` composes cleanly as
the body of the Resql write:

```yaml
detach:
  do:
    # Fan out to all registered peer gates.
    - parallel_http:
        peers: "${gates}"
        call: http.post
        args:
          url: "${peer.baseUrl}/v1/send"
          body: "${incoming.body}"
          headers:
            Content-Type: application/json
            X-Internal-Service-Token: "[#INTERNAL_SERVICE_TOKEN]"
        aggregate: collect_all
        max_concurrency: 16
        timeout: 60000
        result: peer_responses

    # Count the successes vs transport-failed vs upstream-failed.
    - assign:
        success_count: "${peer_responses.filter(r => r.response.status >= 200 && r.response.status < 300).length}"
        transport_fails: "${peer_responses.filter(r => r.response.status === 0).length}"
        upstream_fails: "${peer_responses.filter(r => r.response.status >= 400).length}"

    # Write the structured record to Postgres.
    - call: http.post
      args:
        url: "[#RESQL_URL]/efti/write_search_responses"
        body:
          searchId: "${incoming.body.id}"
          success_count: "${success_count}"
          transport_fails: "${transport_fails}"
          upstream_fails: "${upstream_fails}"
          replies: "${peer_responses}"
  timeout_ms: 90000
```

## Known caveats

- **`${peer}` / `${response}` leaks** from a nested `parallel_http`
  still happen (parity with the step's documented posture); they
  stay confined to the detached task's context snapshot and never
  reach the parent.
- **No cross-replica state.** Ruuter restart mid-detach loses the
  in-flight work. The detached task writes to Postgres as it
  progresses, so already-completed sub-steps leave their rows
  behind. Operators who need "survive restart" semantics use a
  work-queue decoupling instead (NATS JetStream, Kafka, Postgres
  LISTEN+NOTIFY) — out of scope for this step.
- **Context clone cost.** Snapshotting the parent's variables is
  cheap on small maps but linear in map size. A DSL that accumulates
  10k keys and then fires detach pays a proportional cost. Keep
  pre-detach context small.
- **Guard stack is reset.** A `template:` sub-step inside `do:`
  starts with an empty guard stack. Semantically correct (the
  detached task is a new execution), but DSL authors who rely on
  parent-stack-based recursion detection should know.

## Parse-time errors

Caught at DSL load:

- Empty `do:` — "detach.do must contain at least one sub-step".
- `return:` inside `do:` — "detach.do may not contain a `return:`
  sub-step — the parent response has already been sent by the time
  the detached task runs." Use a `switch:` that terminates
  naturally if you need early exit.
- `timeout_ms: 0` — "detach.timeout_ms must be > 0 (unset the field
  for no timeout)".

## Runtime errors

- **Registry full** (`detach.max_inflight` exhausted) →
  `RuuterError::DslExecution { step: "detach", ... }`. Route via
  `error:` to a fallback path, or let it fall through to the
  framework's generic 500.
- **Sub-step errors** → logged with parent's `traceparent`;
  parent's response is unaffected. The remaining sub-steps in
  `do:` are **not** run (the detached task bails on the first
  error, same as `iterate.do:`).
- **Timeout** → the currently-running sub-step is cancelled, a
  WARN is logged, the task exits.

## Observability

- Per-step INFO log entry with:
  - `detached=true`
  - `steps=N` (number of sub-steps in `do:`)
- Sub-step logs fire via each sub-step's normal log machinery (the
  `http.*` step's `display_request_content`, the `log:` step's
  message, etc.).
- Errors inside `do:` log with the parent's `traceparent` so
  operators can correlate the detached work back to the inbound
  request.
- SIGTERM drain logs: `detach drain: waiting for inflight tasks`
  (INFO) and `detach drain: grace exceeded, aborting` (WARN) when
  the window closes with tasks still running.

## Cross-links

- [`parallel_http` step](parallel_http.md) — the fan-out primitive
  most commonly composed inside `detach.do:`.
- [`iterate` step](iterate.md) — sequential looping; `do:` sub-step
  semantics are the same.
- [`single_flight` step](single_flight.md) — synchronous
  coalescing; also uses the "sub-steps run sequentially inside a
  `do:` block" contract.
- [Request pipeline](../../framework/pipeline.md) — where `detach`
  fits in the execution model.
- [Idempotency pattern](../idempotency-pattern.md) — writing
  caller-identity + body-hash state records from inside a detached
  task.
