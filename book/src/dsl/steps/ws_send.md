# ws_send

Push a JSON frame to one or more WebSocket connections.

Three addressing modes, checked in this priority order:

## 1. Broadcast

```yaml
fanout:
  ws_send:
    broadcast_prefix: "client:"     # every connection id starting with this
    payload: { note: "server closing" }
```

Server-side connections are automatically registered as `client:<32-hex>`. Sources are `source:<project>:<name>`.

## 2. Explicit target(s)

```yaml
direct:
  ws_send:
    to: "${target_cid}"             # string OR array of strings
    payload: { dm: "${incoming.body.text}" }
```

Unknown connection id → step error.

## 3. Implicit (reply to caller)

```yaml
reply:
  ws_send:
    payload: { type: "echo", got: "${incoming.body}" }
```

Uses `context.connection_id` — the connection whose frame triggered this DSL. If the DSL was invoked outside a WebSocket context (i.e. HTTP), the step errors.

## Payload

Any JSON value. Serialized once and sent as a single Text frame. `${...}` expressions inside the payload are evaluated before send.
