# Optimistic concurrency (If-Match)

Framework enforces the PRESENCE of an `If-Match` header on state-changing methods. Validation of the token value is the DSL's job.

## Configuration

```yaml
optimistic_concurrency:
  require_if_match: false                       # opt in
  enforce_on_methods: [PUT, PATCH, DELETE]
```

## Enforcement

When `require_if_match: true` and the request method is in `enforce_on_methods`:

- No `If-Match` header → `428 Precondition Required` with `{"error": "If-Match header is required for this method"}`. DSL is not run.
- Header present → framework passes it through as `incoming.headers['if-match']`. DSL validates against the actual state.

## DSL-side validation

```yaml
# The framework only checks presence — the DSL compares against actual state.
check_etag:
  state:
    get:
      key: "user:${incoming.body.id}:etag"
      into: current
  next: compare

compare:
  switch:
    - condition: "${current !== incoming.headers['if-match']}"
      next: stale
  next: mutate

stale:
  status: 412
  return:
    error: "resource changed"
  next: end

mutate:
  # ... perform the write ...
```

## Companion `ETag` on GET responses

Set an `ETag` on read responses so clients can send it back:

```yaml
read:
  state:
    get:
      key: "user:${incoming.body.id}:etag"
      into: etag
  next: reply

reply:
  return:
    # ... your response body ...
  headers:
    ETag: "${etag}"
  next: end
```

## Design note

The framework does not know your aggregate model, so it cannot compare `If-Match` to "the current version". That's DSL + Resql (or whatever your persistence layer is).
