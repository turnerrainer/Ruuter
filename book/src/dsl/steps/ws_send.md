# ws_send

Push a JSON frame to one or more WebSocket connections.

Three addressing modes, checked in this priority order:

## 1. Broadcast

```yaml
fanout:
  ws_send:
    broadcast_prefix: "client:"     # every connection id starting with this
    payload:
      note: "server closing"
```

Server-side connections are automatically registered as `client:<32-hex>`. Sources are `source:<project>:<name>`.

## 2. Explicit target(s)

```yaml
direct:
  ws_send:
    to: "${target_cid}"             # string OR array of strings
    payload:
      dm: "${incoming.body.text}"
```

Unknown connection id → step error.

## 3. Implicit (reply to caller)

```yaml
reply:
  ws_send:
    payload:
      type: "echo"
      got: "${incoming.body}"
```

Uses `context.connection_id` — the connection whose frame triggered this DSL. If the DSL was invoked outside a WebSocket context (i.e. HTTP), the step errors.

## Payload

Any JSON value. Serialized once and sent as a single Text frame. `${...}` expressions inside the payload are evaluated before send.

## Runnable example

`DSL/samples/WS/echo.yml` — a minimal WebSocket endpoint that echoes
each inbound frame back to its sender via the implicit-reply form:

```yaml
reply:
  ws_send:
    payload:
      type: "echo"
      received: "${incoming.body}"
      connection_id: "${incoming.connection_id}"
  next: end
```

Drive it with `wscat` (or any WS client).

Request:

```bash
wscat -c ws://localhost:8080/samples/echo -x '{"greet":"hello"}' -w 1
```

Response frame:

```json
{"connection_id":"client:ed5b374ce44a5acd474839644e27de85","received":{"greet":"hello"},"type":"echo"}
```

The `connection_id` is assigned at upgrade time and stays stable for
the lifetime of the socket — every subsequent frame from the same
client arrives with the same id. Use it as the key for a per-client
`state` entry if you need session-like behaviour.
