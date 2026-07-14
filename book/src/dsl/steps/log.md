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
