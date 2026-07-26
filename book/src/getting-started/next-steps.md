# What to read next

You've run the server, watched the automated tests pass, and hit every
sample endpoint through Postman. That's the whole "does this work?"
loop. Everything below is on demand.

## By role

| You are | Start here |
|---|---|
| Writing your first DSL | [File layout](../dsl/layout.md) → [Steps](../dsl/steps/index.md) → [Guards](../dsl/guards.md) |
| Wiring Ruuter into an existing HTTP stack | [Built-in endpoints](../framework/endpoints.md) → [CSRF](../framework/csrf.md) → [CORS](../framework/cors.md) |
| Calling upstream services safely | [SSRF allow-list](../framework/ssrf.md) → [Response size cap](../framework/size-cap.md) → [Method allow-list](../framework/methods.md) |
| Adding WebSocket endpoints | [Server DSLs](../ws/server.md) → [Sources & triggers](../ws/sources.md) |
| Testing your own DSLs | [Testing overview](../testing/overview.md) → [Test file schema](../testing/schema.md) → [Matchers](../testing/matchers.md) |
| Deploying | [Docker](../ops/docker.md) → [Configuration](../ops/configuration.md) → [Security hardening checklist](../ops/security-checklist.md) |

## By example

Every sample in the Postman collection is backed by a DSL under
`DSL/samples/` and a test under `DSL-tests/samples/`. Pick a feature
you want to see working, then read those two files side by side.

- **Parameters & JSON output** — `GET samples/variables/incoming-params`
- **Status codes and headers** — `GET samples/basic/status-codes`, `GET samples/basic/custom-headers`
- **Conditional logic** — `GET samples/conditionals/simple-switch?age=25`
- **Calling an external HTTP API** — `GET samples/http/simple-get`
- **In-process state** — `POST samples/state/*` (see the `state` step reference)
- **WebSocket** — `DSL/samples/WS/*` (Postman doesn't do WS; use `wscat`)
- **Guards** — `GET samples/vault/secret` (protected by `.guard.yml`)

## Reference material

- [DSL step reference](../dsl/steps/index.md) — all 11 primitives, one page each
- [Framework surface](../framework/pipeline.md) — pipeline, CSRF, CORS, SSRF, tracing, script limits
- [Testing tools](../testing/overview.md) — `dsl-lint`, `dsl-test`, matchers, mocking
- [Operations](../ops/configuration.md) — every config knob, env var, failure mode
- [What Ruuter does NOT do](../reference/non-goals.md) — the non-goals list (idempotency, persistent storage, JWT, ...)
