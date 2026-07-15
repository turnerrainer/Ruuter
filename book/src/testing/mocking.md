# Mocking upstream HTTP

DSLs that call `http.get / post / put / patch / delete` normally reach out to real upstreams. Tests intercept those calls without modifying the DSL by combining two mechanisms: the mock server and URL rewriting.

## The mock server

`mock-http` and `trigger-inject` modes each boot a tiny axum-based mock upstream on `127.0.0.1:0`. Its behaviour:

- Matches on **URL substring + HTTP method**. First registered mock whose `url_matches:` occurs in the request URL and whose `method:` matches wins.
- Records every call — URL, method, body — for later `verify_mocks:` assertions.
- Returns the mock's declared `status:`, `body:`, `headers:`.
- **No mock registered → HTTP 599**. Loud failure; the DSL step sees a 5xx and typically errors out, which fails the test with a clear message rather than silently silent-passing.

## URL rewriting

DSLs often hardcode external URLs (`https://jsonplaceholder.typicode.com/users/1`). To point them at the mock without touching the DSL, use `http_rewrite:` at the top of the test file:

```yaml
mode: mock-http

http_rewrite:
  "https://jsonplaceholder.typicode.com": "{MOCK}"
```

Under the hood: the runner expands `{MOCK}` to the mock server's base URL (`http://127.0.0.1:<port>`) and sets the env var:

```
RUUTER_HTTP_REWRITE=https://jsonplaceholder.typicode.com=http://127.0.0.1:57431
```

`HttpClient::request` reads that env var on every outbound call and rewrites any URL whose origin matches a rewrite `from:` (path, query, headers preserved).

Multiple rewrites: comma-separate:

```yaml
http_rewrite:
  "https://api.example.com": "{MOCK}"
  "https://httpbin.org": "{MOCK}"
```

## Constants-based redirection

When a DSL points at `[#some_webhook_url]` instead of a hardcoded URL, override the constant in the test file directly — no `http_rewrite` needed:

```yaml
mode: trigger-inject

constants:
  stock_alert_webhook: "{MOCK}/alerts"
  aapl_alert_webhook:  "{MOCK}/aapl-alerts"

tests:
  - name: ...
    setup:
      mocks:
        - { url_matches: "/alerts",      method: POST, status: 200, body: {} }
        - { url_matches: "/aapl-alerts", method: POST, status: 200, body: {} }
```

`{MOCK}` is expanded to the mock's base URL before the DSL loader reads the constant.

## Verify mock calls

```yaml
verify_mocks:
  - url_matches: "/alerts"
    count: 1
    body_matches:               # subset-match on the JSON body the DSL sent
      symbol: "MSFT"
      close: 405.0
```

If the DSL didn't call the mock (or called it a different number of times), the assertion produces:

```
mock assertion failed: expected 1 call(s) matching '/alerts', got 0
  (all calls: [("POST", "/some/other/url")])
```

## Isolation between scenarios

The mock server's registered mocks are cleared between scenarios in the same file. Each scenario re-registers its `setup.mocks:`, so mocks don't leak between test cases.

## What NOT to mock

Idempotency-Key handling, CSRF, traceparent — these all happen inside Ruuter, not on an outbound call. They're exercised by `inprocess` mode automatically because the harness routes through the full axum stack. Don't mock them; assert on their effects (`replayed: true`, `header_present: [x-trace-id]`, `status: 403` from a rejected CSRF).
