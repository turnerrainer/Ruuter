# parallel_http

Issues #135 + #136. Bounded concurrent fan-out to N peer services
with three structured-aggregation modes. The DSL-level primitive that
replaces the Klite-multiplexer pattern retired by K4
(kemit-ee/efti-gate-ee#252) and the broader "ask every peer gate,
aggregate structured results" shape — eFTI, broadcast fan-out,
cache warming, etc.

```yaml
fan:
  parallel_http:
    peers: "${gates}"             # expression → array
    call: http.post               # default: http.get
    args:
      url: "${peer.baseUrl}/v1/send"
      headers:
        Authorization: "Bearer [#tok]"
      body: "${incoming.body}"
    timeout: 2000                 # per-peer deadline (ms)
    max_concurrency: 8            # bounded fan-out
    aggregate: first_n            # collect_ok | collect_all | first_n
    first_n: 1                    # required with first_n
    early_exit_on:                # only with first_n
      status_range: [200, 299]
      body_predicate: "${response.body.found === true}"
    remaining_peers_after: cancel # cancel | drain_bg (only with first_n)
    result: peer_responses
  next: assemble
```

## What it does

Fires one HTTP call per element of the `peers:` array, concurrently
up to `max_concurrency`. Each call goes through the same
`HttpClient` the `http.*` step uses — so SSRF checks, allowlists,
pinned-DNS resolution, and the issue #89 transport-error contract
all apply per peer. Aggregates results into a structured array
bound to the variable named by `result:`.

Composes naturally with the [`detach` step (issue #137)](detach.md)
for "fire fan-out, return 202 Accepted, let the peers reply in the
background to a Postgres state store the caller polls" — the eFTI
K4 pattern.

## Fields

### `peers:` (required)

A JS expression evaluating to a `Value::Array`. Each element is
bound to `${peer}` while the per-peer `args:` are evaluated — same
model as `iterate.as`. The binding leaks to subsequent steps
(parity with iterate's item_var); if you need a fresh name, assign
first:

```yaml
assign_peers:
  assign:
    gates: [{id: A, url: ...}, {id: B, url: ...}]
  next: fan

fan:
  parallel_http:
    peers: "${gates}"
    ...
```

An empty array is legal: `result:` binds to `[]` and the step
advances to `next:`. A `null` is treated as empty. Anything else
(scalar, object) is a runtime error.

### `call:` (optional, default `http.get`)

The HTTP method dispatch key. Same vocabulary as the `http.*`
step: `http.get`, `http.post`, `http.put`, `http.patch`,
`http.delete`. Parse-time error on anything else.

### `args:` (required)

Per-peer HTTP args. Same shape as [`http.args`](http.md): `url`
(required), `body`, `headers`, `query`, `content_type`. All string
leaves run through the script engine with `${peer}` bound, so a
typical shape is:

```yaml
args:
  url: "${peer.baseUrl}/v1/lookup/${incoming.body.id}"
  headers:
    Authorization: "Bearer ${peer.token}"
  body: "${incoming.body}"
```

`traceparent` is auto-forwarded per peer unless the DSL set one
explicitly (same rule as the http step).

### `timeout:` (optional, milliseconds)

Per-peer round-trip deadline. Default falls back to
`AppConfig.http_request_timeout` (15 s). Each peer times out
independently — a slow peer does not block the others.

### `max_concurrency:` (optional)

Maximum in-flight peer calls at any moment. `None` means
"len(peers)" (no explicit cap). Operators serving public traffic
should always set a modest value — unbounded fan-out turns one
inbound request into N outbound connections.

### `aggregate:` (required)

One of three modes. See [Aggregation modes](#aggregation-modes)
below.

### `first_n:`, `early_exit_on:`, `remaining_peers_after:` (first_n only)

See [Aggregation modes — `first_n`](#first_n--return-when-quota-is-met)
below. Parse-time error if any of these appear under `collect_ok`
or `collect_all`.

### `result:` (required)

Name of the variable the aggregated array is bound to in the parent
context.

## Aggregation modes

### `collect_ok` — "wait for all; drop errored peers"

Wait until every peer either responds or times out. Peers whose
response is a transport error (`status: 0`, `error:` populated) are
dropped. Successes bind in input order (array index aligned to the
input `peers:` list).

Shape: `[{peer, response: {status, body, headers, error: null}}, ...]`
— `response.error` is always null in this mode.

Use when: "do the broadcast, give me what succeeded, I'll log the
rest out-of-band."

### `collect_all` — "wait for all; keep everything"

Same wait semantics; errors kept. Transport failures surface as
`{peer, response: {status: 0, body: null, headers: {}, error: "connect"}}`
— stable shape, no special-casing needed in the DSL.

Shape: `[{peer, response: {status, body, headers, error}}, ...]`.

Use when: you need the full picture (audit, reconciliation,
"which peer missed this?" dashboards). This is the mode K4 names
as the replacement for Klite's string-join aggregation.

### `first_n` — "return when quota is met"

Wait for `first_n:` peers to satisfy `early_exit_on:`; the step
unblocks immediately and the remaining peers are disposed per
`remaining_peers_after:`.

The result array contains only the N matching peers — not every
peer in the input list. If fewer than `first_n` peers match before
every task completes, the step returns the matches it has (so
`${result.length}` is the signal for "quota met vs timed out").

#### `early_exit_on:` (optional under first_n)

Defines "match." Absent → the default is 2xx responses with no body
predicate. Fields:

- `status_range: [low, high]` — inclusive HTTP status range (default `[200, 299]`).
- `body_predicate: "${expression}"` — JS expression evaluated with
  `response` bound to the peer's response (`{status, body, headers, error}`).
  Must return truthy to count. Transport errors never match.

```yaml
early_exit_on:
  status_range: [200, 299]
  body_predicate: "${response.body.found === true}"
```

Like `${peer}`, the `${response}` binding leaks to subsequent steps
(you'll see the last peer's response).

#### `remaining_peers_after:` (optional under first_n, default `cancel`)

What happens to the peers still in flight when `first_n` is met.

- **`cancel`** — hyper closes the sockets, tokio aborts the tasks.
  Lowest tail latency, caller sees only the N matches. Use when
  the already-collected N answers are authoritative.
- **`drain_bg`** — the step unblocks immediately; a detached
  background task consumes the remaining responses and logs each
  via `tracing::info!` (status + peer identity) or `tracing::warn!`
  (transport failure). The caller gets just the first_n matches.
  Use for audit-while-serving: answer the user fast, keep the full
  picture in the log stream.

## Result shape (stable across all three modes)

```json
[
  {
    "peer":     { ... },           // input array element, verbatim
    "response": {
      "status":  200,              // 0 = transport failure
      "body":    { ... } | null,
      "headers": { ... },
      "error":   null | "timeout" | "connect" | "request" | "body" | "decode" | "unknown"
    }
  },
  ...
]
```

- `status: 0` with `error:` populated = transport failure (same
  shape as the [#89 http step contract](http.md#transport-errors)).
- `status: 2xx` with `error: null` = success.
- `status: 4xx / 5xx` with `error: null` = upstream rejected us
  (reachable, replied, not OK).

DSL authors key on `response.status === 0` for transport failures
and `response.status >= 400` for upstream rejections — the two are
semantically different and the shape distinguishes them.

## Composition with the detach step (eFTI K4 pattern)

```yaml
accept:
  state:
    set:
      key: "search:${incoming.body.id}"
      value: { status: "in_progress", submitted_at: "${now()}" }
  next: fanout_async

fanout_async:
  detach:
    do:
      - fanout:
          parallel_http:
            peers: "${gates}"
            args:
              url: "${peer.baseUrl}/v1/send"
              body: "${incoming.body}"
            aggregate: collect_all
            max_concurrency: 16
            timeout: 60000
            result: peer_responses
      - persist:
          call: http.post
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

The DSL returns `202 Accepted` immediately; the fan-out + Resql
write continues in the background. Caller polls a sibling route
that reads from Postgres. See [`detach`](detach.md) for the
background-execution contract.

## Error handling and partial failures

- A peer task that panics (shouldn't happen under normal operation)
  is logged via `tracing::warn!` and skipped — it does not fail the
  step.
- A transport error on a peer surfaces as `{response: {status: 0,
  error: "..."}}` in the result array (or is dropped under
  `collect_ok`). The step itself returns success.
- An invalid URL expression (fails to evaluate to a string) is a
  step-level error — aborts the fan-out and routes to the step's
  `error:` handler if set.
- Semaphore closure (should not happen; defensive) surfaces as a
  per-peer transport-error-shaped record.

## Parse-time errors

- `aggregate: first_n` without `first_n: N` (N >= 1) → load error.
- `first_n:` / `early_exit_on:` / `remaining_peers_after:` under
  `collect_ok` / `collect_all` → load error (undefined fields).
- `call:` outside the five known methods → load error.
- `early_exit_on.status_range: [hi, lo]` where `hi > lo` → load error.

## Observability

Each step execution emits a per-step INFO log entry with:
- `peers=N` — input peer count
- `aggregate=collect_ok|collect_all|first_n` — mode
- `yielded=M` — number of entries in the result array

Each peer's outbound HTTP call is logged via the usual `http.*`
step log machinery (`display_request_content` /
`display_response_content` config). `drain_bg` peers log their
eventual completion via `tracing::info!` on the parent request's
span.

## Known caveats

- **`${peer}` binding leaks** to subsequent steps (parity with
  `iterate.as`). If you need isolation, assign a fresh variable.
- **`${response}` binding leaks** when `first_n.early_exit_on.body_predicate`
  is set — subsequent steps that reference `response` see the last
  peer's response.
- **Order within the result array** is input order (sorted after
  collection). Completion order is lost intentionally — the DSL
  author's mental model is "give me results in the order I asked."

## Cross-links

- [`http` step](http.md) — the per-peer invocation shape.
- [`iterate` step](iterate.md) — the sequential alternative; use
  when peers can't run concurrently.
- [`detach` step](detach.md) — background execution; composes with
  `parallel_http` for the eFTI K4 pattern.
- [SSRF allow-list](../../framework/ssrf.md) — applies per peer.
- [Request pipeline](../../framework/pipeline.md) — where
  `parallel_http` fits in the execution model.
- [`declaration` step](declaration.md) — the OpenAPI shape for
  routes that use `parallel_http`.
