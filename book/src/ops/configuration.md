# Configuration

Ruuter reads one YAML config file at boot. Every field is optional; unset fields inherit safe defaults.

## Resolution priority

1. `--config <path>` CLI flag
2. `RUUTER_CONFIG=<path>` env var
3. `./ruuter.yaml` or `./ruuter.yml` in the working directory
4. Built-in defaults (no file needed)

The startup log tells you which source was chosen:

```
INFO ruuter_rs: Loaded config from ./ruuter.yaml
# or
INFO ruuter_rs: Using built-in default config (no ruuter.yaml found)
```

## Full annotated example

```yaml
port: 8080
config_path: ./DSL                        # where to look for DSL/<project>/... trees
http_request_timeout: 15000               # ms; per outbound http step
max_step_recursions: 10000                # engine step-transition cap
http_response_size_limit: 16777216        # bytes (16 MiB); null = unbounded
http_codes_allow_list: []                 # empty = accept every upstream status

response_default_headers:
  X-Content-Type-Options: nosniff
  X-Frame-Options: DENY

cors:
  allowed_origins: []                     # empty = CORS layer not attached
  allow_credentials: false

incoming_requests:
  allowed_method_types: [GET, POST, PUT, PATCH, DELETE, OPTIONS]

internal_requests:
  disabled:     false
  allowed_urls: []                        # URL prefixes; empty = any
  allowed_ips:  []                        # bare-IP hosts; empty = any

csrf:
  allowed_origins: []                     # empty = check bypassed
  enforce_on_methods: [POST, PUT, PATCH, DELETE]

idempotency:
  enabled: true
  ttl_seconds: 86400
  methods: [POST, PUT, PATCH, DELETE]

optimistic_concurrency:
  require_if_match: false
  enforce_on_methods: [PUT, PATCH, DELETE]

scripting:
  max_loop_iterations: 1000000
  max_stack_size: 400

dsl:
  allowed_filetypes:  [.yml, .yaml]
  processed_filetypes: [.yml, .yaml]
  allow_dsl_reloading: false              # not implemented in 0.4.0

logging:
  display_request_content:  false
  display_response_content: false
  print_stack_trace:        false
  meaningful_errors:        false
```

Ship this file alongside `docker-compose.yml`, mount it as `/app/ruuter.yaml` inside the container.
