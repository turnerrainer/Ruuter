# Idempotency pattern (DSL-authored)

**Ruuter no longer implements framework-level `Idempotency-Key`
handling.** Every DSL run executes; no response is cached or replayed
by the framework, and no `Idempotency-Replayed` header is emitted.

The reason is that "the same request" is not a framework decision.
Cross-caller replay, body canonicalisation rules, tenant scoping,
and TTL are all product concerns. When the framework guessed at
these it created security surfaces (h2ck.me findings **S1** —
missing body-hash allowed cross-caller replay, and **S5** —
`Idempotency-Replayed` acted as an oracle for probing keys). Both
went away with the framework-level implementation.

DSL authors who need idempotency implement it explicitly, tailored to
their identity model.

## Pattern

Use `state.get` to look up a dedup entry keyed on
`(caller identity, endpoint, hash of the canonicalised body)`. If a
prior entry exists, return the stored response; otherwise run the
work and `state.set` the result.

```yaml
# svc/POST/create-order.yml
dedup-key:
  assign:
    key: "${incoming.origin + ':' + 'POST:/svc/create-order:' + sha256(canonical(incoming.body))}"
  next: check

check:
  state: { get: { key: "${key}", into: cached } }
  next: branch

branch:
  switch:
    - condition: "${cached != null}"
      next: replay
  next: work

replay:
  return: "${cached.body}"
  status: "${cached.status}"
  next: end

work:
  resql:
    query: insert-order
    args: "${incoming.body}"
  result: r
  next: store

store:
  state:
    set:
      key: "${key}"
      value: { status: 201, body: "${r.response.body}" }
      # DSL-side TTL — pick what fits your product
      ttl_seconds: 86400
  next: reply

reply:
  return: "${r.response.body}"
  status: 201
  next: end
```

## Why this shape

- **Body is part of the key.** The framework used to key on
  `Idempotency-Key` alone; that let a second caller reuse the same
  header to replay the first caller's response body. Include
  `sha256(canonical(body))` in the key you write.
- **Caller identity is part of the key.** `incoming.origin`
  respects `proxy.trusted` (only trusted proxies can supply
  `X-Forwarded-For`), so a hostile direct caller can't spoof
  another user's identity — but you still need the identity in the
  key so two different users using `Idempotency-Key: 1` don't
  collide. Adjust to the identity model your app already uses
  (JWT subject, session key, tenant + user).
- **No oracle.** The DSL controls the response — if you don't want
  callers to be able to tell replay from a fresh run, don't add a
  `replayed: true` marker to the body.

## Where to look next

- Book chapter [State step](./steps/state.md) — mechanics of
  `state.get` / `state.set` and TTL semantics.
- Book chapter [SSRF allow-list](../framework/ssrf.md) and
  [Traceparent & OpenTelemetry](../framework/tracing.md) for
  related identity/audit surfaces.
