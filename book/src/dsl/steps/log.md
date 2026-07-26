# log

Emit a structured log line at `INFO` level.

```yaml
audit:
  log: "user=${incoming.headers['x-user']} did=${incoming.body.action}"
  next: reply
```

- The value is a string with `${...}` interpolation.
- Output goes to stderr via `tracing`; controlled by `RUST_LOG` env var.
- No return, no state change. Purely a side effect.

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

Run the server with `RUST_LOG=info` and hit it:

```console
$ curl -s 'http://localhost:8080/samples/advanced/logging-demo?userId=42'
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
