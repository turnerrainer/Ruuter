# log

Emit a structured log line at `INFO` level.

```yaml
audit:
  log: "user=${incoming.headers['x-user']} did=${incoming.body.action}"
  next: reply
```

- The value is either a **string** with `${...}` interpolation (single-line form)
  or a **map / array** whose string leaves are interpolated the same way
  and whose evaluated shape is rendered as compact JSON in the log line.
- Output goes to stderr via `tracing`; controlled by `RUST_LOG` env var.
- No return, no state change. Purely a side effect.

## Map form

Use a map when the log line has more than one named field — easier to
read on disk, easier to parse in a log-ingestion pipeline than a single
concatenated string.

```yaml
audit:
  log:
    user: "${incoming.headers['x-user']}"
    action: "${incoming.body.action}"
    request_id: "${incoming.headers['x-request-id']}"
  next: reply
```

Every string leaf runs through the script engine, so `${...}` works the
same as in the scalar form. Non-string leaves (numbers, booleans, nested
maps, arrays) pass through unchanged. The rendered log line contains the
compact-JSON form of the evaluated map, sanitised for CR/LF and
truncated at 256 characters.

Map keys are literal YAML — they are NOT evaluated. Only values are.

## Runnable example

`DSL/samples/GET/advanced/logging-demo.yml` interleaves `log` steps
between compute steps to trace a run:

```yaml
log_start:
  log: "Request processing started for user: ${incoming.params.userId}"
  next: process

process:
  assign:
    user_id: ${incoming.params.userId}
    timestamp: ${Date.now()}
  next: log_processing

log_processing:
  log: "Processing user ID: ${user_id} at ${timestamp}"
  next: complete

complete:
  assign:
    result:
      userId: ${user_id}
      processedAt: ${timestamp}
      status: "completed"
  next: log_complete

log_complete:
  log: "Request processing completed successfully"
  next: respond

respond:
  return: ${result}
  next: end
```

Run the server with `RUST_LOG=info`, then hit it.

Request:

```bash
curl -s 'http://localhost:8080/samples/advanced/logging-demo?userId=42'
```

Response:

```json
{"processedAt":1785079808330.0,"status":"completed","userId":"42"}
```

(Numbers from `Date.now()` serialise as JSON floats — hence the
`.0` suffix.)

The server's stderr shows three log lines from the DSL's three `log:`
steps (INFO-level lines from `tracing`; formatting depends on your
`RUST_LOG` / subscriber config):

```
INFO ruuter_on_rust::steps::log: Request processing started for user: 42
INFO ruuter_on_rust::steps::log: Processing user ID: 42 at 1785079247934
INFO ruuter_on_rust::steps::log: Request processing completed successfully
```

The response body is unaffected by `log:` — it's a pure side effect.
