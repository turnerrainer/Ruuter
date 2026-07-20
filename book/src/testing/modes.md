# Modes

Four modes, selected by the `mode:` field at the top of the test file.

## `inprocess` (default)

Route through the full axum stack via `tower::ServiceExt::oneshot`. No TCP socket. All framework middleware runs.

Fields used: `request:`, `expect:`, `setup:` (state only, mocks ignored), `verify_state:`.

```yaml
mode: inprocess
tests:
  - name: happy path
    request: { method: GET, path: /samples/ping }
    expect: { status: 202, body: pong }
```

Use for anything that doesn't need an upstream HTTP call, a live WS client, or synthetic event injection. That's ~70% of the corpus.

## `mock-http`

Same as `inprocess`, plus:

- Boots a mock upstream on `127.0.0.1:0`.
- Expands `{MOCK}` in `constants:` values.
- Sets `RUUTER_HTTP_REWRITE` from `http_rewrite:`, so outbound HTTP calls whose origin matches a rewrite `from:` are redirected to the mock.
- Enables `setup.mocks:` (register response canned responses) and `verify_mocks:` (assert on captured calls).

```yaml
mode: mock-http

http_rewrite:
  "https://jsonplaceholder.typicode.com": "{MOCK}"

tests:
  - name: fetches user
    setup:
      mocks:
        - url_matches: "/users/1"
          method: GET
          status: 200
          body: { id: 1, name: "Leanne Graham" }
    request: { method: GET, path: /samples/http/simple-get }
    expect:
      status: 200
      body_matches: { data: { id: 1, name: "Leanne Graham" } }
    verify_mocks:
      - { url_matches: "/users/1", count: 1 }
```

Any DSL HTTP call that doesn't match a registered mock returns 599, so tests fail loudly instead of silently reaching the real internet.

## `ws-client`

Binds a real axum server on `127.0.0.1:0`, opens a `tokio-tungstenite` client, sends `ws.send[]` frames, collects up to `expect_frames.len()` frames or times out.

Fields used: `ws:`, `setup:` (state only), `verify_state:`.

```yaml
mode: ws-client
tests:
  - name: echo bounces every frame
    ws:
      path: /samples/echo
      send:
        - { hello: "world" }
      expect_frames:
        - { type: "echo", received: { hello: "world" }, connection_id: "$type:string" }
```

Each frame in `expect_frames` subset-matches against the received frame at the same index. Order matters. Extra received frames beyond `expect_frames.len()` are not checked. Timeout defaults to 2000 ms; override with `timeout_ms:`.

## `trigger-inject`

Bypasses the WS source layer entirely — calls `TriggerDispatcher::dispatch` directly with the payload the source would have emitted.

Fields used: `trigger:`, `setup:` (state and mocks), `verify_state:`, `verify_mocks:`.

```yaml
mode: trigger-inject

constants:
  aapl_alert_webhook: "{MOCK}/alerts"

tests:
  - name: big move alerts
    setup:
      state:
        - { project: samples, key: "stock:AAPL:last_close", value: 100.0 }
      mocks:
        - url_matches: "/alerts"
          method: POST
          status: 200
          body: { ok: true }
    trigger:
      project: samples
      channel: stock-bars
      key: AAPL
      payload: { S: "AAPL", c: 105.0, t: "2026-06-25T13:00:00Z" }
    verify_state:
      - { project: samples, key: "stock:AAPL:last_close", value: 105.0 }
    verify_mocks:
      - url_matches: "/alerts"
        count: 1
        body_matches: { symbol: "AAPL", close: 105.0 }
```

Why bypass the source? Sources connect to real upstreams (WebSocket, MQTT, Kafka in future). Testing the trigger DSL itself doesn't need that plumbing — the dispatcher's contract is `(project, channel, key, payload) → DSL`. Test that contract, not the socket.

## Choosing a mode

| DSL type | Mode |
|---|---|
| Pure engine (assign, switch, return, JS) | `inprocess` |
| State store (`state:` step, `verify_state:`) | `inprocess` |
| Guards | `inprocess` |
| Templates (calls another DSL by name) | `inprocess` if callee is engine-only; `mock-http` if callee makes HTTP calls |
| External HTTP (`call: http.get/post/...`) | `mock-http` |
| WS server DSL (`WS/<path>.yml`) | `ws-client` |
| Trigger DSL (`triggers/<channel>/...`) | `trigger-inject` |
| Framework invariant (CSRF, 404, traceparent) | `inprocess` — the full stack runs |
