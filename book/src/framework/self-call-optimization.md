# Self-call short-circuit

When a `http.<verb>` step's URL resolves to Ruuter's own listener, the request is dispatched **in-process** through the DSL router instead of round-tripping through reqwest + TCP + the framework's accept loop. Semantic behaviour is preserved — guards run, path params resolve, response shape is byte-identical to a network loopback call.

## Why

A DSL that self-calls (composite orchestration endpoints, template ports of legacy code, test/dev environments simulating external peers) pays 2-5 ms per hop on TCP loopback even though the caller and callee live in the same process. That's serialise → framing → recv → reparse → dispatch, all for nothing. Skipping it makes composite DSLs meaningfully faster on the hot path and cuts CPU under sustained rps.

## When it fires

Every outbound HTTP request whose URL host+port matches one of Ruuter's own listeners (`SelfOrigins`) AND where the router handle has been wired at boot. Both conditions are automatic in the standard `main.rs` path; DSLs need zero changes.

Match rules (default `SelfOrigins` from a fresh `AppConfig`):

- Any URL targeting `localhost`, `127.0.0.1`, `0.0.0.0`, `[::1]`, or `::1` on the configured `port` short-circuits.
- Non-HTTP schemes (ws://, wss://, unix://) never match.
- Cross-instance URLs (different host or port) fall through to the network.

## What is preserved

| Behaviour | Preserved | Notes |
|---|---|---|
| Guards on the target route | **Yes** | The same guard chain the network path runs |
| Path parameter resolution | **Yes** | Trailing segments strip into `incoming.params.pathParams` |
| CSRF Origin check | **Yes** | Guard runs, sees the headers you passed |
| Request headers | **Yes** | Pass through into `incoming.headers` |
| Query string parsing | **Yes** | URL's `?a=1&b=2` merged with any explicit `query:` argument |
| Request body | **Yes** | Object bodies pass through; non-object bodies wrap under `_body` |
| Response status | **Yes** | Returned in `${upstream.response.status}` shape |
| Response headers | **Yes** | Returned in `${upstream.response.headers}` shape |
| Response body | **Yes** | Same JSON shape as a network response |
| `http_codes_allow_list` | **Yes** | Applied after the router returns |

## What is NOT preserved (yet)

- **Framework `Idempotency-Key` cache.** Removed in v0.7.0 — no longer relevant. Idempotency is a [DSL-authored pattern](../dsl/idempotency-pattern.md), so the outer and inner DSLs share it naturally when both call `state.get`/`state.set` with the same key derivation.
- **Response-size cap enforcement.** Self-calls have no wire transfer, so there's no natural point to apply the cap. If a DSL body could produce unbounded output, the outer TCP path's cap won't fire on inner self-calls.
- **`force_network: true` escape hatch.** Not shipped in v1. If a DSL author needs to bypass the short-circuit (e.g. to specifically test the fronting-proxy path), the workaround is to hit a non-self URL or configure a distinct listener. File a follow-up if this becomes common.

## Recursion guard

The engine's `max_step_recursions` cap (default 10 000) applies to the outer DSL's step transitions. A DSL that self-recursively calls itself accumulates step transitions on the outer engine — the cap fires and the DSL terminates. The framework does NOT track nested self-call depth separately; deep DSL nesting under short-circuit would exhaust the same cap the network path would.

If you have a pattern that legitimately needs deep self-call composition, raise `max_step_recursions` or restructure the DSL to fan out flat rather than recursively.

## SSRF interaction

The SSRF allow-list (`internal_requests.allowed_urls`, `internal_requests.allowed_ips`) applies to the TCP path, not the self-call path. A self-URL is definitionally "our own process" and is not gated by SSRF rules. The rationale: SSRF is about preventing DSLs from probing the internal network via a compromised outbound; short-circuiting *into ourselves* is not a network hop.

If you specifically want to prevent self-recursion (or prevent one project from cross-calling another project's routes), the answer is guards on the target routes — not SSRF rules on the caller.

## Failure modes

| Situation | Behaviour |
|---|---|
| Self-URL, router handle wired | Short-circuit fires |
| Self-URL, handler NOT wired (test build, misconfig) | Falls through to network path — no silent success |
| Non-self URL | Network path (reqwest) |
| Target route returns non-2xx | Status propagates in `HttpResponse.status` |
| Target route errors (500 / step failure) | Error propagates through `Result<HttpResponse>` |

## Verifying it's firing

There is no dedicated metric today. The best signal is a latency measurement: bench a DSL that self-calls with the short-circuit enabled vs. disabled (by manually clearing the handle in a test build). Expect ≥1.5 ms p50 improvement per self-hop.
