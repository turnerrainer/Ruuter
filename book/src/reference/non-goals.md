# What Ruuter deliberately does NOT do

- **IAM / JWT validation.** Guards can inspect `Authorization` headers and reject; the framework itself does no cryptographic verification. Verify tokens in a TIM sidecar or via a service-mesh policy.
- **Secret fetching.** No Vault / KMS / Docker-secret integration. Mount the resolved `constants.ini` file.
- **Persistent state.** `state` step is in-process, not durable, not cross-replica. Front with Resql (SQL → REST) for anything that must survive a restart.
- **Framework-level `Idempotency-Key` dedup.** Removed in v1.0.0 (h2ck.me findings S1 + S5). The framework never keys, caches, or replays by `Idempotency-Key`; DSL authors implement the pattern themselves via `state.get`/`state.set` — see [Idempotency pattern](../dsl/idempotency-pattern.md). Cross-replica dedup then follows whichever state backend the operator configures.
- **ETag value validation.** Framework enforces presence of `If-Match` (opt-in); comparing to actual state is the DSL's job via Resql or equivalent.
- **Scheduled work.** CronManager fires HTTP at Ruuter endpoints on schedule; Ruuter itself has no timer facility.
- **Hot DSL reload.** DSLs are read at boot. Restart the container after edits.
- **Rate limiting.** Nothing built-in. Terminate at a reverse proxy (nginx, Envoy) or add per-DSL rate logic via `state`.
- **Body streaming.** Requests + upstream responses are read into memory (with the caps documented in [Response size cap](../framework/size-cap.md) and the 16 MiB inbound cap). No streaming multipart upload path.
- **GraphQL.** Ruuter is REST + WS only.
- **Path templating (`{id}` in URLs).** Path parameters are handled by trailing-segment stripping (see [Path parameters](../dsl/path-params.md)); there are no `{brace}` templates in DSL keys.
