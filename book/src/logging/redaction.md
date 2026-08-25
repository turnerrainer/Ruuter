# Redaction & log-injection defence

Two threats the logging pipeline neutralises before any value
enters a log line:

1. **Secrets leaking** — an outbound HTTP body dump, a header
   log, or a DSL-authored `log:` message that carries a token,
   password, cookie, or PII field.
2. **Log-line splicing** — an attacker-controlled string
   containing `\n` or `\r` that would otherwise break out of
   its field and forge a fake log line downstream.

Both defences are applied unconditionally at every emission
point that touches request-derived data.

## Body-field redaction

- **How it matches**: case-insensitive comparison of a JSON
  key against `logging.redact_body_fields`.
- **What it replaces**: the whole value (regardless of type —
  string, number, object, array) becomes the string
  `"[REDACTED]"`.
- **Depth**: recursive. Nested objects and array items are
  walked; matched keys are redacted at every depth.
- **Nested-secret collapse**: if the matched key's value is
  itself an object containing more secrets, the WHOLE
  subtree becomes `"[REDACTED]"`. A `payload.credentials.password`
  under a top-level `credentials` field never leaks even if
  someone forgets to add `password` to the list.

Example. With the default `redact_body_fields` list and a body:

```json
{
  "user": "alice",
  "password": "hunter2",
  "profile": {
    "token": "s3cret",
    "name": "Alice"
  },
  "items": [
    { "secret": "s1" },
    { "secret": "s2" }
  ]
}
```

Logged as:

```json
{
  "user": "alice",
  "password": "[REDACTED]",
  "profile": {
    "token": "[REDACTED]",
    "name": "Alice"
  },
  "items": [
    { "secret": "[REDACTED]" },
    { "secret": "[REDACTED]" }
  ]
}
```

## Header redaction

- **How it matches**: case-insensitive comparison of a header
  name against `logging.redact_headers`.
- **What it replaces**: the value becomes `"[REDACTED]"`; the
  name is kept.
- **Where**: every header map emitted under
  `display_request_content` or `display_response_content`.

Example. With defaults and outbound headers:

```
Authorization: Bearer sk_live_...
Cookie: session=abc
X-Custom-Header: keep-me
```

Logged as:

```json
{
  "Authorization": "[REDACTED]",
  "Cookie": "[REDACTED]",
  "X-Custom-Header": "keep-me"
}
```

## Extending the lists

Add project-specific names in `ruuter.yaml`. The DEFAULTS are
INCLUDED — you're extending, not replacing. If you provide a
list, that list wins in its entirety (Rust default handling),
so remember to re-list the defaults when adding your own:

```yaml
logging:
  redact_headers:
    - authorization           # keep the default
    - proxy-authorization     # keep the default
    - cookie                  # keep the default
    - set-cookie              # keep the default
    - x-api-key               # keep the default
    - x-auth-token            # keep the default
    - x-buerostack-session    # project-specific
    - x-tenant-token          # project-specific

  redact_body_fields:
    - password                # keep the default
    - pass                    # keep the default
    - secret                  # keep the default
    - token                   # keep the default
    - access_token            # keep the default
    - refresh_token           # keep the default
    - api_key                 # keep the default
    - authorization           # keep the default
    - ssn                     # project-specific (PII)
    - dob                     # project-specific (PII)
    - patient_email           # project-specific (PII)
    - medical_record          # project-specific (PII)
```

## Body-size cap

`logging.max_body_bytes` (default 2048) truncates the serialised
body at that many bytes AFTER redaction. Truncation is safe:

- Cuts at a UTF-8 char boundary so no half-multibyte trailing.
- Ends with `…` so operators can tell "line was capped" from
  "log-store truncated the line".
- Applied after redaction, so removed secrets don't take up the
  budget.

Values above ~16 KiB defeat the point (most log-store ingest
paths rate-limit per-line above that). If you find yourself
raising it, consider whether the body should be logged at all —
usually the sentinel + status code is enough for triage.

## CRLF stripping (log-injection defence)

Every string value that would enter a log field is stripped of
`\n` and `\r` before emission. Applied to:

- Outbound URLs.
- Inbound / outbound header values.
- JSON body values (when they render as strings in body dumps).
- The interpolated `dsl.log` field on the DSL `log:` step.
- Error `Display` output.
- Every `cause_chain` hop.

**Why**: without this, an attacker who could plant `\n` in any
of the above (via a hostile client, a compromised upstream, or
a DSL that interpolates unsanitised input) could forge fake log
lines like:

```
INFO login-successful user=attacker
INFO real-line was-here
```

With CRLF stripping, the same payload comes out as:

```
INFO real-line user=attacker INFO login-successful ...
```

still one line, obviously suspicious under review — and never
parsable as two records.

## What's NOT redacted

- **URL query strings**. `url.full` is logged verbatim after
  CRLF-stripping and length capping (512 bytes). If your query
  string carries a session id, add a URL-rewriting step in your
  DSL so the outbound URL doesn't contain it — the framework
  cannot know which query params are secret.
- **Response status codes**. Always logged in full.
- **Trace ids**. Not sensitive — they're per-request random,
  and the correlation value is the point.
- **The DSL `log:` step's message body**, EXCEPT that the
  message string is CRLF-stripped. If the DSL authors write
  `log: "user=${incoming.body.user} password=${incoming.body.password}"`
  they get exactly what they asked for. Redaction is a
  framework-level defence for framework-generated log lines;
  DSL-authored `log:` values are the DSL author's responsibility.

## PII detection (not implemented, on purpose)

Field-name-based redaction is the industry standard. Content-based
redaction (regex-detect a credit card number, an email address, a
national id) is out of scope because:

- False positives destroy grep-ability.
- False negatives create a false sense of safety.
- Different jurisdictions redact different patterns.
- Fields with predictable names should be redacted by name;
  fields with unpredictable names should not be logged at all.

If your compliance regime requires content-based scrubbing, run
the log stream through a downstream scrubber (Vector, fluent-bit,
Datadog agent, OpenTelemetry Collector) after Ruuter emits.

## Testing your redaction

Turn on both content flags in a test environment and hit an
endpoint that carries the field you want to check:

```yaml
logging:
  format: json
  display_request_content: true
  display_response_content: true
  redact_body_fields:
    - password
    - ssn
```

```bash
curl -X POST http://localhost:8080/samples/POST/http/post-data \
  -H 'Content-Type: application/json' \
  -d '{"username":"alice","password":"hunter2","ssn":"123-45-6789"}'
```

Then grep the logs:

```bash
docker compose logs ruuter | jq -c '. | select(.fields."http.request.body" != null)'
```

The `password` and `ssn` values should render as `"[REDACTED]"`;
the `username` should render verbatim.

## Cross-links

- [Configuration reference](./configuration.md) — the
  `redact_headers` / `redact_body_fields` / `max_body_bytes`
  knobs.
- [Field vocabulary](./fields.md) — which fields carry
  redactable content.
- [Recipes / hardening](./recipes.md#hardening--extend-redaction).
