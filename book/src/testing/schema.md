# Test file schema

A `.test.yml` file describes one or more scenarios against one DSL (or WS handler, or trigger, or framework invariant).

## Top-level

```yaml
mode: inprocess          # default; inprocess | mock-http | ws-client | trigger-inject
constants:               # optional; overrides constants.ini for this file
  API_KEY: "test-key"
http_rewrite:            # optional; only meaningful in mock-http / trigger-inject
  "https://api.example.com": "{MOCK}"
tests:                   # required
  - name: ...
    ...
```

`{MOCK}` in either `constants:` values or `http_rewrite:` values expands to the mock upstream's base URL at runtime.

## Scenario

```yaml
- name: string                    # required; shown in output
  description: string             # optional
  request: HttpRequest            # inprocess / mock-http
  ws: WsScenario                  # ws-client
  trigger: TriggerScenario        # trigger-inject
  expect: ExpectHttp              # inprocess / mock-http
  setup: Setup                    # optional; seed state or register mocks
  verify_state: [StateAssertion]  # optional; state-store assertions
  verify_mocks: [MockAssertion]   # optional; mock-upstream call assertions
```

## `request:` (HttpRequest)

```yaml
request:
  method: GET | POST | PUT | PATCH | DELETE | OPTIONS
  path: /samples/basic/hello       # full URL path (project = first segment)
  query: { k: v, ... }             # optional query params (values coerced to string)
  headers: { k: v, ... }           # optional headers
  body: { ... } | "string" | [...] # optional JSON body
```

## `ws:` (WsScenario)

```yaml
ws:
  path: /samples/echo
  query: { k: v }                  # handshake query (optional)
  headers: { k: v }                # handshake headers (optional)
  send:                            # frames to send, each becomes one Text frame
    - { type: "hello" }
  expect_frames:                   # order-sensitive; each subset-matches the received frame
    - { type: "echo", received: { type: "hello" } }
  timeout_ms: 2000                 # optional; how long to wait for expected frames
```

## `trigger:` (TriggerScenario)

```yaml
trigger:
  project: samples                 # first URL segment of the DSL file's project
  channel: bars                    # matches DSL/<project>/triggers/<channel>/
  key: AAPL                        # matches <key>.yml, falls back to _default.yml
  payload: { S: "AAPL", c: 100 }   # becomes the DSL's incoming.body
  expect_dispatched: true          # optional; false = the DSL should NOT match
```

## `expect:` (ExpectHttp)

Every field is optional. Absent fields are not checked.

```yaml
expect:
  status: 200                      # HTTP status
  body: { ... }                    # exact match on response body
  body_matches: { ... }            # subset match; see Matchers
  headers: { X-Foo: bar }          # header equality (case-insensitive on the name)
  header_present: [X-Trace-Id]     # header must exist
  header_absent: [X-Something]     # header must not exist
```

## `setup:` (Setup)

```yaml
setup:
  state:                            # state-store rows inserted before the scenario runs
    - { project: samples, key: counter, value: 41 }
  mocks:                            # mock-http / trigger-inject only
    - url_matches: "/users/1"       # URL substring
      method: GET                   # default GET
      status: 200                   # default 200
      body: { ... }                 # optional response body (JSON)
      headers: { k: v }             # optional response headers
```

Setup runs at the start of each scenario. State from setup + state written during the scenario carries into subsequent scenarios in the same file.

## `verify_state:` (StateAssertion[])

```yaml
verify_state:
  - project: samples
    key: counter
    value: 42          # subset-match on objects, deep-equal on scalars, null = must be missing
```

## `verify_mocks:` (MockAssertion[])

```yaml
verify_mocks:
  - url_matches: "/users/1"
    count: 1                        # exact number of matching calls
    body_matches: { ... }           # subset-match on the request body sent to the mock
```

Any request the DSL makes that does NOT match a registered mock returns HTTP 599 — the DSL fails loudly rather than silently reaching the real internet.

## Full worked example

```yaml
mode: mock-http

http_rewrite:
  "https://jsonplaceholder.typicode.com": "{MOCK}"

tests:
  - name: fetches user then posts, combines
    setup:
      mocks:
        - url_matches: "/users/7"
          method: GET
          status: 200
          body: { id: 7, name: "Kurtis Weissnat" }
        - url_matches: "/posts"
          method: GET
          status: 200
          body: [{ id: 60, title: "hello" }]
    request:
      method: POST
      path: /samples/http/chained-requests
      headers: { content-type: "application/json" }
      body: { userId: 7 }
    expect:
      status: 200
      body_matches:
        user: { id: 7, name: "Kurtis Weissnat" }
        posts: "$type:array"
    verify_mocks:
      - { url_matches: "/users/7", count: 1 }
      - { url_matches: "/posts", count: 1 }
```
