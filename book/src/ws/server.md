# WebSocket server DSLs

`DSL/<project>/WS/inbound/<path>.yml` — clients connect at
`ws://<host>/<project>/<path>`. The DSL runs **once per inbound
frame** (text or binary).

Legacy layout — files placed directly under `DSL/<project>/WS/<path>.yml`
— still loads with a WARN. Rename to `WS/inbound/…` for the canonical
layout; see [Reserved subdirectories](../reference/reserved-subdirs.md)
for the full transition table.

## Context inside a WS DSL

| Binding | Value |
|---------|-------|
| `incoming.body`          | parsed JSON of the frame, or `{"value": "<text>"}` if not JSON |
| `incoming.headers`       | handshake headers (snapshotted at upgrade, identical across frames) |
| `incoming.params`        | handshake URL query (snapshotted at upgrade) |
| `incoming.connection_id` | per-client id like `client:<32-hex>` |

## Reply to originating client

`DSL/samples/WS/inbound/echo.yml`:

```yaml
reply:
  ws_send:
    payload:
      type: "echo"
      received: "${incoming.body}"
      connection_id: "${incoming.connection_id}"
  next: end
```

Request — one frame in:

```bash
wscat -c ws://localhost:8080/samples/echo -x '{"greet":"hello"}' -w 1
```

Frame received back:

```json
{"connection_id":"client:ed5b374ce44a5acd474839644e27de85","received":{"greet":"hello"},"type":"echo"}
```

## Broadcast to every connected client

`DSL/samples/WS/inbound/broadcast.yml`:

```yaml
fanout:
  ws_send:
    broadcast_prefix: "client:"
    payload:
      type: "broadcast"
      from: "${incoming.connection_id}"
      msg: "${incoming.body}"
  next: end
```

Request:

```bash
wscat -c ws://localhost:8080/samples/broadcast -x '{"hello":"world"}' -w 1
```

Frame received back (every connected client sees the same frame,
including the sender):

```json
{"from":"client:402bebd3493e1eb923c92171f7d9d27e","msg":{"hello":"world"},"type":"broadcast"}
```

## Target a specific connection

```yaml
# DSL/svc/WS/inbound/dm.yml
send:
  ws_send:
    to: "${incoming.body.to}"        # e.g. "client:abc..."
    payload:
      text: "${incoming.body.text}"
  next: end
```

Unknown connection id → step error → connection stays open, error logged.

## Per-connection state

Use the [`state` step](../dsl/steps/state.md) with
`incoming.connection_id` as the key namespace.
`DSL/samples/WS/inbound/chat.yml` is a worked example — each client's
nickname is remembered until it disconnects:

```yaml
route:
  switch:
    - condition: "${incoming.body.type === 'set_name'}"
      next: save_name
    - condition: "${incoming.body.type === 'msg'}"
      next: load_name
  next: end

save_name:
  state:
    set:
      key: "chat:${incoming.connection_id}:name"
      value: "${incoming.body.name}"
  next: ack_name

ack_name:
  ws_send:
    payload:
      type: "name_set"
      name: "${incoming.body.name}"
  next: end

load_name:
  state:
    get:
      key: "chat:${incoming.connection_id}:name"
      into: "sender_name"
  next: fanout

fanout:
  ws_send:
    broadcast_prefix: "client:"
    payload:
      type: "msg"
      from: "${sender_name || 'anonymous'}"
      text: "${incoming.body.text}"
  next: end
```

Request — set the nickname first:

```bash
wscat -c ws://localhost:8080/samples/chat \
      -x '{"type":"set_name","name":"alice"}' -w 1
```

Frame received back (server acknowledges to the same connection only):

```json
{"name":"alice","type":"name_set"}
```

Request — send a message from the same connection:

```bash
wscat -c ws://localhost:8080/samples/chat \
      -x '{"type":"msg","text":"hey"}' -w 1
```

Frame received back (broadcast to every connected client). Note the
`from` value falls back to `"anonymous"` when this run uses a fresh
connection — because the previous `wscat` invocation dropped its
socket, and per-connection state is bound to that specific
`connection_id`:

```json
{"from":"anonymous","text":"hey","type":"msg"}
```

To see the intended `from: "alice"`, hold one connection open and
send both frames on it (feed multiple lines via stdin instead of
`-x`).

## Life cycle

- Handshake is a regular HTTP GET with `Upgrade: websocket`.
- Guards do NOT run on WS handshakes in 0.4.0 — enforce auth in the DSL body via `incoming.headers`.
- On disconnect: writer task aborts; connection unregistered from the WsRegistry.
