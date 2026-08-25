# Environment variables

| Variable                        | Purpose | Default |
|---------------------------------|---------|---------|
| `RUST_LOG`                      | Log filter (see `tracing_subscriber::EnvFilter` syntax) | `info` |
| `RUUTER_LOG_FORMAT`             | Log output format: `text` or `json`. Overrides `logging.format`. Full ref: [Logging](../logging/index.md) | value of `logging.format` in config (default `text`) |
| `RUUTER_CONFIG`                 | Path to a YAML config file | unset (falls through to `./ruuter.yaml`) |
| `RUUTER_ADMIN_ENABLED`          | `true` to expose `GET /_/sources` | unset (endpoint returns 404) |
| `OTEL_EXPORTER_OTLP_ENDPOINT`   | OTLP gRPC endpoint (e.g. `http://otel-collector:4317`). Setting this enables span export. | unset |
| `OTEL_SERVICE_NAME`             | Service name attached to emitted spans | `ruuter-on-rust` |

## RUST_LOG examples

```bash
RUST_LOG=info                             # everything at info+
RUST_LOG=debug                            # everything at debug+
RUST_LOG=ruuter_on_rust=debug,reqwest=warn     # per-module control
```
