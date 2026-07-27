# state

Project-scoped, in-process key-value store. Three operations:

| YAML op | What it does | Result binding |
|---|---|---|
| `state.get`    | Read `key` into a context variable | `into` var; missing key → `null` |
| `state.set`    | Write a JSON value under `key` | (no binding) |
| `state.delete` | Remove `key` | (no binding; no-op if absent) |

`remove:` is accepted as an alias for `delete:` so DSLs can spell it either way.

## Quick reference

```yaml
read:
  state:
    get:
      key: "counter"
      into: n
  next: bump

bump:
  assign:
    n2: "${(n ?? 0) + 1}"
  next: write

write:
  state:
    set:
      key: "counter"
      value: "${n2}"
  next: reply

wipe:
  state:
    delete:
      key: "counter"
  next: end

gone:
  state:
    remove:                # alias for delete
      key: "counter"
  next: end
```

## Semantics

- **Scope**: per-project. The project is the first path segment of the inbound request (`/orders/...` → project `orders`). Two projects never see each other's keys.
- **Type**: any JSON value. Numbers, strings, arrays, objects — stored as-is.
- **Missing key on `get`**: binds `null`. Guard with `${n ?? default}`.
- **Persistence**: in-process only. **Container restart wipes everything.** No disk snapshot, no WAL.
- **Concurrency**: `DashMap`-backed; concurrent writes to the same key are last-write-wins within one process.
- **No TTL, no size cap, no eviction.** If the workload grows keys unboundedly, delete them explicitly.
- **No cross-replica coordination.** Each Ruuter pod keeps its own copy. Two replicas WILL diverge — see [Multi-instance caveat](#multi-instance-caveat).

## Runnable example — increment a counter

`DSL/samples/POST/state/inc.yml`:

```yaml
read_counter:
  state:
    get:
      key: "counter"
      into: current
  next: bump

bump:
  assign:
    next_value: "${(current == null ? 0 : current) + 1}"
  next: write_counter

write_counter:
  state:
    set:
      key: "counter"
      value: "${next_value}"
  next: respond

respond:
  return:
    counter: "${next_value}"
  next: end
```

`DSL/samples/POST/state/get.yml` reads it back without mutating:

```yaml
read_counter:
  state:
    get:
      key: "counter"
      into: current
  next: respond

respond:
  return:
    counter: "${current}"
  next: end
```

Hit the increment endpoint twice, then read.

First bump:

```bash
curl -sX POST http://localhost:8080/samples/state/inc
```

```json
{"counter":1}
```

Second bump:

```bash
curl -sX POST http://localhost:8080/samples/state/inc
```

```json
{"counter":2}
```

Read without mutating:

```bash
curl -sX POST http://localhost:8080/samples/state/get
```

```json
{"counter":2}
```

Restart the server → the value is gone (`get` returns `{"counter":null}`).
`state` is process-local and non-durable by design; see the multi-instance
caveat at the bottom of this page.

## Sample — cache an upstream REST response

Cache each unique upstream response and short-circuit subsequent calls
with the same key. The key mixes URL + body-hash so distinct inputs
get distinct cache slots.

```yaml
# GET /samples/state/rates?symbol=EUR
cache_key:
  assign:
    key: "rates:${incoming.query.symbol}"
  next: lookup

lookup:
  state:
    get:
      key: "${key}"
      into: cached
  next: decide

decide:
  switch:
    - condition: "${cached != null}"
      next: reply_from_cache
  next: fetch_upstream

fetch_upstream:
  call: http.get
  args:
    url: "https://rates.example/${incoming.query.symbol}"
  result: r
  next: store

store:
  state:
    set:
      key: "${key}"
      value: "${r.response.body}"
  next: reply_fresh

reply_from_cache:
  return:
    source: "cache"
    data: "${cached}"
  next: end

reply_fresh:
  return:
    source: "upstream"
    data: "${r.response.body}"
  next: end
```

Adjacent value-under-hash pattern (collapse "same request" for a
POST body):

```yaml
key:
  assign:
    k: "quote:${sha256(JSON.stringify(incoming.body))}"
  next: lookup
# ...same lookup / decide / fetch / store as above
```

## Sample — invalidate on write

A write endpoint updates the source-of-truth AND drops the cached read.
Next `GET` misses cache, re-fetches, and re-populates.

```yaml
# PUT /samples/state/rates
persist:
  call: http.put
  args:
    url: "https://rates.example/${incoming.body.symbol}"
    body: "${incoming.body}"
  result: r
  next: invalidate

invalidate:
  state:
    delete:
      key: "rates:${incoming.body.symbol}"
  next: reply

reply:
  return:
    updated: true
  next: end
```

## Sample — TTL via timestamp

Ruuter's store has no built-in TTL. Bake the expiry into the stored
value and check it on read.

```yaml
lookup:
  state:
    get:
      key: "session:${incoming.headers.authorization}"
      into: s
  next: check

check:
  switch:
    - condition: "${s != null && s.expires_at > Date.now()}"
      next: use_session
  next: refresh_session

use_session:
  return:
    user: "${s.user}"
  next: end

refresh_session:
  # ... hit the auth service, then store a fresh entry with a new expiry.
  call: http.post
  args:
    url: "https://auth.example/introspect"
    body:
      token: "${incoming.headers.authorization}"
  result: t
  next: store_session

store_session:
  state:
    set:
      key: "session:${incoming.headers.authorization}"
      value:
        user: "${t.response.body.sub}"
        expires_at: "${Date.now() + 300000}"    # 5-minute lease
  next: use_session_fresh

use_session_fresh:
  return:
    user: "${t.response.body.sub}"
  next: end
```

## Sample — one-shot dedup marker

`state.set` under a per-request key acts as a first-write-wins marker
within one process. Combined with `single_flight`, it collapses
duplicate concurrent submissions.

```yaml
mark:
  state:
    get:
      key: "submit:${incoming.body.request_id}"
      into: seen
  next: gate

gate:
  switch:
    - condition: "${seen != null}"
      next: replay_result
  next: reserve

reserve:
  state:
    set:
      key: "submit:${incoming.body.request_id}"
      value:
        status: "in_flight"
  next: do_work

do_work:
  # ... real side-effect steps here.
  next: record_done

record_done:
  state:
    set:
      key: "submit:${incoming.body.request_id}"
      value:
        status: "done"
        body: "${result}"
  next: reply_done

replay_result:
  return:
    replayed: true
    prior: "${seen}"
  next: end

reply_done:
  return:
    status: "ok"
  next: end
```

For the "N concurrent identical POSTs collapse to one execution" case,
front this with [`single_flight`](./single_flight.md) — that step
handles the concurrent-arrival race; `state.set` handles the
retry-after-completion race.

## Multi-instance caveat

Two Ruuter replicas do **not** share state. Container restarts, rolling
updates, and horizontal scaling all diverge the store. Options for
cross-replica consistency:

- **Front the cache with Resql / Postgres.** DSL reads through
  `http.get resql/...` first, writes through both Resql and `state.set`.
  Correctness lives in the DB; `state` is just a local perf boost.
- **Skip caching, hit the upstream every request.** Fine for
  low-frequency reads.
- **Bus-based invalidation.** Ruuter already ships WS-subscriber
  plumbing (see [Sources & triggers](../../ws/sources.md)); a trigger
  DSL can call `state.delete` on receipt of an invalidation message
  from an external broker.

See [What Ruuter does NOT do](../../reference/non-goals.md).
