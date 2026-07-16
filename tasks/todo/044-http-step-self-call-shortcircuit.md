# 044 — `http.*` step self-call short-circuit (skip network for same-instance URLs)

## Filed

2026-07-16 — companion to task 043. Where 043 removes the TCP cost of
hitting an adjacent service, this task removes the network cost
entirely when the target of `http.<verb>` is Ruuter itself.

## Problem

A DSL step `call: http.post` targeting
`http://localhost:8080/svc/other` pays the full round trip on every
call, even though caller and callee live in the same process:

```
DSL A → HttpStepExecutor → reqwest serialize
      → TCP loopback (send)
      → axum accept → HTTP parse → route match
      → new BoaContext → DSL B runs
      → response serialize → TCP loopback (recv)
      → reqwest deserialize → back to DSL A
```

Estimated round-trip: **2-5 ms**, dominated by TCP framing and JSON
serialize/deserialize on both sides. For DSL trees where the natural
factoring puts common logic behind a self-hit route (X-Road adapter
in [`runtime_composition.md`](../../../../KeMIT/eFTI/Gate/docs/architecture/infrastructure/runtime_composition.md) §3, guard-shared endpoints, etc.),
this cost is on every request.

The [`template` step](../../book/src/dsl/steps/template.md) shipped in
0.4.0 (task 027) can invoke another DSL directly through the shared
engine — but its semantics differ from HTTP:

| Aspect | `template` step | `http.<verb>` to self |
|---|---|---|
| Guards on target route | **Bypassed** | Run |
| CSRF Origin check | Bypassed | Run |
| Method allow-list | Bypassed | Run |
| Idempotency-Key cache | Bypassed | Consulted |
| Path parameters | N/A (direct DSL invoke) | Resolved |
| Traceparent forward | Shared parent context | Forwarded via header |

So `template` is right for "call this DSL as a factored subroutine";
it is wrong as a general drop-in optimization for `http.<verb>`
because it silently changes the security posture. A DSL author
porting from Java Ruuter cannot safely rewrite `http.post` calls to
`template` without auditing every guard on every target route.

## Fix

Auto-detect self-URLs in `HttpStepExecutor::execute` and short-circuit
via `DslRouter::execute_dsl` instead of `reqwest`. **Preserve every
HTTP semantic** — guards run, CSRF runs, Idempotency-Key runs, path
params resolve, framework response headers get added.

### Detection

At boot, `main.rs` knows the listener bindings (task 043 extends this
to include UDS paths). Compute a set of "self-origins":

```rust
pub struct SelfOrigins {
    tcp: HashSet<(String, u16)>,  // ("127.0.0.1", 8080), ("localhost", 8080), ("0.0.0.0", 8080), ...
    unix: HashSet<PathBuf>,       // /var/run/ruuter/*.sock
}
```

Pass through to `HttpClient` at construction; `HttpStepExecutor` (or
`HttpClient::request()` internally) matches the outgoing URL against
this set before making a network call.

### Execution

When matched:

```rust
if let Some(self_target) = self_origins.match_url(&url) {
    let (method, path) = self_target.method_and_path();
    let router = router_handle.upgrade().ok_or(...)?;   // Arc<DslRouter>
    let result = router.execute_dsl(
        &project_from(path),
        method.as_str(),
        &path_from(path),
        body_map_from(body),
        query_map_from(query),
        headers_map_from(headers),
        origin_from_context(context),
    ).await?;
    // Re-shape result to match the `http` step's expected
    // { response: { status, body, headers } } contract:
    return Ok(bind_result(context, self.step.result.as_ref(), result));
}
// else: fall through to the existing reqwest path
```

The wiring needs a weak/shared handle to `DslRouter` reachable from
`HttpClient` (or from a new intermediary). Currently `DslRouter` owns
the `StepEngine` which owns `HttpClient`; the cycle needs care —
either `Arc<Weak<DslRouter>>` on `HttpClient` set at boot, or a
dedicated `RouterHandle` shared before `DslRouter` construction.

### What still goes over TCP

- URLs that don't match self-origins (external services, other Ruuter
  replicas, DLK AS4 sidecar).
- Explicitly-forced-network calls — add an escape hatch:
  ```yaml
  call: http.post
  args:
    url: "http://localhost:8080/svc/other"
    force_network: true      # bypasses the short-circuit
  ```
  Motivation: a DSL author testing production behaviour locally, or
  a scenario that specifically needs to exercise the fronting proxy
  or CSRF via Origin header.

## Numbers to hit

Bench scenarios (added to task 039):

| Scenario | Baseline (TCP self-call) | Target (short-circuit) |
|---|---:|---:|
| Self-call to thin DSL, warm keep-alive | ~3 ms p50 | ≤ 1 ms p50 |
| Self-call to guard-protected DSL | ~4 ms p50 | ≤ 1.5 ms p50 |
| Self-call at 1k rps sustained | CPU % | ≥ 20% reduction vs TCP |

## Interaction with existing framework features

- **Idempotency-Key**: caller passes the header via `http` args;
  `DslRouter::execute_dsl` sees it and consults the cache exactly as
  a network call would. Cache hit still returns the cached response.
- **CSRF Origin check**: caller sets `Origin` header explicitly in
  `http` args or via traceparent auto-forwarding. If missing on a
  state-changing method and `csrf.allowed_origins` is non-empty, the
  target guard fails 403 — same as network path.
- **`traceparent`**: already auto-forwarded by the `http` step
  (0.4.0). Behaviour preserved.
- **Response size cap**: N/A for self-calls — no network transfer,
  no cap needed. But `max_step_recursions` on the outer engine still
  applies to the total transitions across nested calls (prevents
  self-recursion loops).
- **SSRF allow-list**: self-URLs are explicitly the case the
  allow-list is designed to permit; short-circuit runs regardless of
  `internal_requests.disabled` because the request never leaves the
  process. Document this clearly.

## Acceptance

- `SelfOrigins` computed at boot from listener config.
- `HttpClient::request()` (or the step-level dispatcher) checks
  self-origins before dispatching to reqwest.
- Router invocation preserves guards, CSRF, method-allow-list,
  Idempotency-Key, path-params, response headers.
- Escape hatch `force_network: true` bypasses the short-circuit.
- Integration test: `tests/self_call.rs` verifies:
  - Self-call output matches a network-loopback call byte-for-byte.
  - Guards on the target route run and can reject the self-call.
  - Idempotency-Key cache hit is served identically.
  - `force_network: true` measurably takes the TCP path (assert on
    a metric or a mock hook).
- Book chapter: [`book/src/framework/self-call-optimization.md`](../../book/src/framework/self-call-optimization.md)
  (new) — covers when it fires, semantic guarantees, escape hatch,
  interaction with each framework feature.

## Recursion guard

A self-call short-circuit must NOT create an unbounded stack:

```yaml
loop:
  call: http.get
  args: { url: "http://localhost:8080/svc/loop" }
```

The router's `max_step_recursions` engine counter needs to also
count self-calls (increment on every short-circuited invocation).
When the counter hits the cap, the inner call returns the same error
a runaway `next:` loop would.

## Non-goals

- **Cross-instance short-circuit.** If URL points at another Ruuter
  replica behind a load balancer, the short-circuit does NOT trigger
  — the caller is deliberately going through the LB and probably wants
  the round-trip for consistency/routing reasons.
- **Global HTTP semantics change.** DSL syntax unchanged; only the
  transport is elided. The observable behaviour of `http.<verb>` is
  identical to a network call in every case except performance.
- **Automatic template-step conversion.** Distinct optimization from
  the `template` step — this task keeps `http.<verb>` semantics;
  `template` explicitly declares the DSL is a callable subroutine
  with reduced ceremony. Both remain in the DSL vocabulary.
