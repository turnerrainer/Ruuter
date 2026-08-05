# Configuration

Ruuter reads one YAML config file at boot. Every field is optional; unset fields inherit safe defaults.

## Resolution priority

1. `--config <path>` CLI flag
2. `RUUTER_CONFIG=<path>` env var
3. `./ruuter.yaml` or `./ruuter.yml` in the working directory
4. Built-in defaults (no file needed)

The startup log tells you which source was chosen:

```
INFO ruuter_on_rust: Loaded config from ./ruuter.yaml
# or
INFO ruuter_on_rust: Using built-in default config (no ruuter.yaml found)
```

The boot sequence also emits a WARN line for every accepted-but-inert
config field it detects (Java-parity nouns that the loader accepts
but that don't influence runtime behaviour). See the [inert-field
reference](../config/inert-fields.md) for the full list.

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

response:
  default_wrapper: true                   # Java parity — wrap body in {"response": ...} envelope
  dsl_with_response_status: null          # status when DSL returns a value AND didn't set status (null = 200)
  dsl_without_response_status: null       # status when DSL never reached a return (null = 200)

guards:
  mode: stack                             # or `closest_only` for Java-parity single-guard behaviour

default_dsl_in_case_of_exception:
  dsl: default-dsl                        # fallback DSL name (Java `defaultDslInCaseOfException` parity)
  request_type: POST
  project: framework
  body: {}
  query: {}
  headers: {}

cors:
  allowed_origins: []                     # empty = CORS layer not attached
  allow_credentials: false

incoming_requests:
  allowed_method_types: [GET, POST, PUT, PATCH, DELETE, OPTIONS]
  headers: {}                             # merged into every incoming request

internal_requests:
  disabled: false                         # kill-switch — honoured by every transport (TCP, UDS, self-call)
  allowed_urls: []                        # URL prefixes; empty = any
  allowed_ips: []                         # bare-IP hosts; empty = any
  block_private_networks: true            # default-deny RFC-1918 / loopback / link-local / ULA

proxy:
  trusted: []                             # peer IPs allowed to set X-Forwarded-For / X-Real-IP

csrf:
  allowed_origins: []                     # empty = check bypassed
  enforce_on_methods: [POST, PUT, PATCH, DELETE]

optimistic_concurrency:
  require_if_match: false
  enforce_on_methods: [PUT, PATCH, DELETE]

scripting:
  max_loop_iterations: 1000000
  max_stack_size: 400

unix_socket_map:                          # DSL-transparent UDS aliases — DSL keeps writing http://<alias>/...
  resql: /var/run/ruuter/resql.sock
  tim:   /var/run/ruuter/tim.sock

uds_http_version: http1                   # or `http2` (h2c on UDS)

listeners: []                             # empty = single TCP listener on `port`. See below.

dsl:
  allowed_filetypes: [.yml, .yaml]        # accepted-but-inert; see config/inert-fields.md
  processed_filetypes: [.yml, .yaml]
  allow_dsl_reloading: false              # dev only — see book/src/ops/hot-reload.md

logging:                                  # accepted-but-inert; see config/inert-fields.md
  display_request_content:  false
  display_response_content: false
  print_stack_trace:        false
  meaningful_errors:        false

stop_in_case_of_exception: true           # accepted-but-inert; the engine always halts on step error
```

## Multiple listeners

When `listeners:` is non-empty it **replaces** the single-`port`
default. Each entry names one bind spec (TCP or UDS). Example — TCP
externally, UDS for a co-located sidecar:

```yaml
listeners:
  - name: public
    bind: 0.0.0.0:8080
  - name: internal
    unix: /var/run/ruuter/internal.sock
    http2: true
```

Exactly one of `bind` or `unix` must be set per entry. See
[Listeners](../config/listeners.md).

## Deep-dive tutorials

The "Configuration deep dive" section documents each knob in
isolation with the default, why it exists, and how to migrate from
Java Ruuter's `application.yml`. Start at the
[overview](../config/index.md).

Ship this file alongside `docker-compose.yml`, mount it as
`/app/ruuter.yaml` inside the container.
