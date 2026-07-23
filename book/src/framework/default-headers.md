# Default response headers

Static headers merged into every response.

## Configuration

```yaml
response_default_headers:
  X-Content-Type-Options: nosniff
  X-Frame-Options: DENY
  Strict-Transport-Security: "max-age=31536000; includeSubDomains"
```

## Precedence

Applied LAST — will NOT overwrite a header already set by:

- The DSL's `return.headers`
- Framework-added `traceparent`, `x-trace-id`
- The JSON response body's `content-type`

If you configure `content-type: text/xml` here, a DSL that returns JSON (auto-setting `content-type: application/json`) wins — your default is a no-op for that route.

## Use cases

- Security headers (`X-Content-Type-Options`, `X-Frame-Options`, `Strict-Transport-Security`, `Referrer-Policy`, `Permissions-Policy`).
- Version stamp (`X-App-Version: 0.4.0`).
- Fleet identity (`X-Instance-Id: eu-west-1-a`).
