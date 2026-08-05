# Default exception DSL

Java parity for `defaultDslInCaseOfException`: a fallback DSL to
invoke when an upstream HTTP call errors out and the calling step
has no local `error:` branch.

## What it is

An HTTP step that returns a status outside `http_codes_allow_list`
and has no `error:` handler bubbles the failure up to the framework.
When `default_dsl_in_case_of_exception` is set, the framework
dispatches the named DSL instead of returning the raw error to the
client.

## The config

```yaml
default_dsl_in_case_of_exception:
  dsl: default-dsl              # DSL file name (no path, no extension)
  request_type: POST            # HTTP method used to look up the fallback DSL
  project: framework            # DSL/<project>/<request_type>/<dsl>.yml
  body: {}                      # forwarded verbatim (see enrichment below)
  query: {}
  headers: {}
```

## The defaults and why

- `request_type: POST` — matches Java samples. The fallback DSL is
  typically a diagnostic-shaped POST handler.
- `project: framework` — Rust has a project layer that Java doesn't;
  operators drop a fallback under `DSL/framework/POST/default-dsl.yml`
  and reference it by bare `dsl: default-dsl`.
- `body`, `query`, `headers` all default to empty maps. Anything you
  put here is forwarded verbatim, on top of the framework's enrichment.

The block is absent by default. When absent, failed HTTP steps propagate
their error as before.

## Enrichment injected by the framework

Before invoking the fallback DSL, the framework merges these keys into
`body` (Java's `DefaultHttpDsl.executeHttpDefaultDsl`):

- `statusCode` — the failed upstream status.
- `responseBody` — the raw upstream body (or empty string on transport
  failure).
- `failedRequestId` — the current traceparent trace id, so the fallback
  can correlate against logs.

Your own `body:` entries take precedence over the enrichment keys if
names collide.

## What breaks if you set it wrong

- Naming a DSL that doesn't exist → boot succeeds (the config only
  names a file; existence is checked at dispatch time), but every
  triggered fallback returns a route-not-found error, hiding the
  original failure.
- Choosing `request_type: GET` when the fallback DSL is defined under
  `POST/…` → the router lookup misses and the same route-not-found
  surfaces. Keep `request_type` and the on-disk method directory in sync.

## Copy-clean YAML

```yaml
default_dsl_in_case_of_exception:
  dsl: default-dsl
  request_type: POST
  project: framework
  body:
    escalate: true
  query: {}
  headers:
    X-Origin: internal
```

Corresponding fallback DSL at `DSL/framework/POST/default-dsl.yml`:

```yaml
handle:
  log:
    message: "upstream call failed"
    context:
      status: "${incoming.body.statusCode}"
      trace:  "${incoming.body.failedRequestId}"
  next: end
```

## Cross-links

- [Upstream status filter](../framework/status-filter.md)
- [http step](../dsl/steps/http.md)
