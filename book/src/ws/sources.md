# WebSocket sources & triggers

Consume upstream WebSocket feeds and dispatch each frame to a trigger DSL.

## Source config

`DSL/<project>/sources/<name>.yml`:

```yaml
kind: websocket
url: "wss://stream.example.com/v2"

# Upgrade headers — sent on the initial handshake request.
# Values run through [#constant] substitution.
headers:
  X-API-Key: "[#feed_api_key]"

# JSON frames sent right after the handshake completes.
# Replayed after every successful reconnect.
on_connect:
  - send_json:
      action: auth
      key: "[#feed_api_key]"
  - send_json:
      action: subscribe
      symbols:
        - "AAPL"
        - "MSFT"

# How to derive (channel, key) from each inbound JSON message.
dispatch:
  channel: "$.T"          # dot-path in the JSON payload
  key:     "$.S"

# Reconnect / backoff — uniform across connect failures and mid-stream errors.
reconnect:
  initial_backoff_ms: 500
  max_backoff_ms:    60000
  jitter:            true
```

## Trigger DSLs

`DSL/<project>/triggers/<channel>/<key>.yml` — one file per `(channel, key)` pair. `_default.yml` matches any key not otherwise covered.

```yaml
# DSL/svc/triggers/bars/AAPL.yml — per-symbol handler
handle:
  state:
    set:
      key: "last.AAPL"
      value: "${incoming.body.c}"
  next: end
```

```yaml
# DSL/svc/triggers/bars/_default.yml — fallback for every other symbol
handle:
  state:
    set:
      key: "last.${incoming.body.S}"
      value: "${incoming.body.c}"
  next: end
```

## Dispatch algorithm

For each inbound text/binary frame:

1. Parse as JSON. Non-JSON is dropped.
2. If the payload is an array, treat each element as a separate message.
3. Extract `channel` and `key` via the dot-paths in `dispatch:`. Missing channel → drop; missing key → treat as empty string.
4. Look up `triggers/<channel>/<key>.yml`. If absent, try `_default.yml`. If both absent, log at debug and drop.
5. Run the matched DSL through the same step engine as HTTP routes.

## Sending back upstream

The source's own outbound sink is registered as `source:<project>:<name>` — a trigger DSL can push to it via `ws_send`:

```yaml
resubscribe:
  ws_send:
    to: "source:svc:stock-feed"
    payload:
      action: "subscribe"
      symbols:
        - "TSLA"
  next: end
```

## Supervision

Every source runs under a supervisor. On crash / disconnect:

- Exponential backoff (jittered) between `initial_backoff_ms` and `max_backoff_ms`.
- Successful reconnect replays `on_connect` payloads.
- Health visible at `GET /_/sources` (requires `RUUTER_ADMIN_ENABLED=true`).
