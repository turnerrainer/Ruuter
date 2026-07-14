# WebSocket server DSLs

`DSL/<project>/WS/<path>.yml` — clients connect at `ws://<host>/<project>/<path>`.

The DSL runs **once per inbound frame** (text or binary).

## Context inside a WS DSL

| Binding | Value |
|---------|-------|
| `incoming.body`          | parsed JSON of the frame, or `{"value": "<text>"}` if not JSON |
| `incoming.headers`       | handshake headers (snapshotted at upgrade, identical across frames) |
| `incoming.params`        | handshake URL query (snapshotted at upgrade) |
| `incoming.connection_id` | per-client id like `client:<32-hex>` |

## Reply to originating client

```yaml
# DSL/svc/WS/echo.yml
reply:
  ws_send:
    payload: { type: "echo", got: "${incoming.body}" }
  next: end
```

## Broadcast to every connected client

```yaml
# DSL/svc/WS/broadcast.yml
fanout:
  ws_send:
    broadcast_prefix: "client:"
    payload:
      from: "${incoming.connection_id}"
      msg:  "${incoming.body}"
  next: end
```

## Target a specific connection

```yaml
# DSL/svc/WS/dm.yml
send:
  ws_send:
    to: "${incoming.body.to}"        # e.g. "client:abc..."
    payload: { text: "${incoming.body.text}" }
  next: end
```

Unknown connection id → step error → connection stays open, error logged.

## Per-connection state

Use the [`state` step](../dsl/steps/state.md) with `incoming.connection_id` as the key namespace:

```yaml
increment:
  state:
    get:
      key: "count:${incoming.connection_id}"
      into: prev
  next: bump
bump:
  assign: { n: "${(prev ?? 0) + 1}" }
  next: write
write:
  state: { set: { key: "count:${incoming.connection_id}", value: "${n}" } }
  next: reply
reply:
  ws_send: { payload: { count: "${n}" } }
  next: end
```

## Life cycle

- Handshake is a regular HTTP GET with `Upgrade: websocket`.
- Guards do NOT run on WS handshakes in 0.4.0 — enforce auth in the DSL body via `incoming.headers`.
- On disconnect: writer task aborts; connection unregistered from the WsRegistry.
