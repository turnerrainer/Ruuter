# 021 — WS source must support custom upgrade headers

**Filed:** 2026-06-25 by desk team during replay-stack provisioning.

## Problem

`src/sources/ws.rs::connect_and_drain` connects via
`tokio_tungstenite::connect_async(&cfg.url)`. The upgrade is bare:
no custom headers are sent. This breaks integrations where the
upstream WS service authenticates on the upgrade itself, e.g.
Andmela's `ws-fanout` and `andmela-replay` services:

```
ws://replay.andmela.local:8082/replay
  Headers on upgrade:
    X-Andmela-Token: <token>
```

Without a way to set the header on connect, the upgrade fails 401
and the source enters a tight reconnect loop.

## Implementation

This task is implemented in the same commit as the patch.

1. `WsSourceConfig` gets an optional `headers: HashMap<String, String>`
   with `[#constant]` substitution running over the values.
2. `connect_and_drain` builds a `tungstenite::handshake::client::Request`
   with the headers attached, then passes it to
   `tokio_tungstenite::connect_async`.
3. The existing reconnect / backoff logic is unchanged.

## DSL surface

```yaml
kind: websocket
url: "ws://replay.andmela.local:8082/replay"
headers:
  X-Andmela-Token: "[#andmela_token]"
on_connect:
  - send_json:
      action: start
      date: "[#replay_date]"
```

## Cross-references

- Closes part of architecture O2 (the bot's view): once headers ship,
  the runner-direct WS fallback is no longer needed.
- Replay-stack PR in `stocktrading.dev/Alpaca/desk/services/`
  consumes this immediately.
