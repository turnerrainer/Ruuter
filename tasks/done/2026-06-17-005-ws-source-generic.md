# 005 — Generic WebSocket source

**Status**: BACKLOG.
**Severity**: HIGH (unlocks all push-driven services).
**Effort**: 1-1.5 days.
**Filed**: 2026-06-17.
**Blocked by**: #003 (state), #004 (trigger directory + dispatcher).

## What's wrong

No WebSocket support at all. Any service whose authoritative data
arrives over WS (market data, presence, device telemetry, chat) cannot
be implemented in Ruuter today.

## Fix

Add a generic `WsSource` driven entirely by YAML config:

```
DSL/<project>/sources/<source_name>.yml
```

```yaml
kind: websocket
url: "${constants.ws_url}"
# Optional opening payload sent immediately after connect.
on_connect:
  - send_json:
      action: auth
      key: "${constants.api_key}"
      secret: "${constants.api_secret}"
  - send_json:
      action: subscribe
      bars: ["AAPL","MSFT"]
# How to derive (channel, key) from each inbound message.
# Both are JSONPath-ish expressions evaluated against the parsed JSON.
dispatch:
  channel: "$.T"     # e.g. "b" for bar, "t" for trade
  key:     "$.S"     # e.g. "AAPL"
# Reconnect policy.
reconnect:
  initial_backoff_ms: 500
  max_backoff_ms: 60000
  jitter: true
```

Runtime:

1. On startup, after DSL load, scan each project's `sources/` dir.
   For every `kind: websocket` config, spawn a tokio task running a
   `WsSource`.
2. `WsSource` connects via `tokio-tungstenite`, sends the `on_connect`
   payloads, then loops on inbound text/binary frames.
3. For each inbound message: parse JSON, evaluate the `channel`/`key`
   expressions, call `TriggerDispatcher::dispatch(project, channel,
   key, payload)` (from #004).
4. On disconnect or auth error: exponential backoff with jitter,
   reconnect, replay `on_connect`. Never crash the process.

`constants.ini` substitution (`${constants.foo}`) is honoured in
source configs — secrets stay out of the YAML.

## Verification

- Spin up a local echo WS server in an integration test; verify a
  message routed to the matching trigger DSL fires its steps.
- Kill the test server, observe reconnect attempts with backoff,
  bring server back, confirm subscriptions are replayed.
- Confirm bad `channel` / `key` evaluation logs and drops the message
  rather than panicking the source task.

## Why this is generic

Nothing in `WsSource` knows about Alpaca, market data, or any
specific protocol. The `on_connect`, `dispatch`, and `reconnect`
configs are arbitrary. Service-specific behaviour lives entirely in
the YAML files of the `<project>/`.
