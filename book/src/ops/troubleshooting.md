# Troubleshooting

## Container starts, `/health` returns 200, but every route returns 404

Cause: the DSL tree wasn't found or is empty.

```bash
docker exec ruuter-rs ls -la /app/DSL/
docker compose logs ruuter-rs | grep 'Loaded'
# Expected: "Loaded N HTTP DSLs across M projects, ..."
```

Fix: ensure `./DSL` is mounted; project subdirectories exist; method directories are named exactly `GET`/`POST`/etc.

## `Container is unhealthy` in `docker ps`

Cause: healthcheck failing. Verify manually:

```bash
docker exec ruuter-rs curl -fsS http://localhost:8080/health
```

If curl isn't found: you're on a pre-0.4.0 image. Rebuild.

## Every response has `"error": "Script evaluation error: SyntaxError: expected token ';'"`

Cause: object-literal at expression start. See [JavaScript gotchas](../dsl/js-gotchas.md). Wrap in parens:

```yaml
value: "${({ a: 1 })}"          # right
value: "${{ a: 1 }}"            # wrong
```

## An `http` step returns `"HTTP request rejected: outbound HTTP is disabled …"`

Cause: `internal_requests.disabled: true` in config. Either flip it to `false`, or add the target to `allowed_urls`.

## Two Ruuter replicas disagree on state / counter values

Cause: `StateStore` is in-process. See [state step](../dsl/steps/state.md#multi-instance-caveat). Front with Resql for cross-replica state.

## Idempotency retry re-runs the DSL

Causes:
- `Idempotency-Key` not sent by the client (framework only dedups when the header is present).
- Method not in `idempotency.methods` (default excludes GET).
- Different `Idempotency-Key` values → treated as different requests.
- Second call landed on a different replica (in-process cache is not shared — see [Idempotency-Key](../framework/idempotency.md)).

## `traceparent` on responses looks fresh even though the caller sent one

Cause: caller's traceparent doesn't match the W3C format `<version>-<trace_id 32-hex>-<span_id 16-hex>-<flags 2-hex>`. Framework generates a fresh one instead of echoing malformed input.

## WebSocket connection accepts frames but nothing happens

Cause: the WS DSL is present but has no `ws_send` step. Reply is optional; the framework only runs the DSL. Check the container logs for step errors.

## `${incoming.body.field}` is `undefined` in a POST DSL

Cause: request didn't send `Content-Type: application/json` — Ruuter only parses JSON bodies. Set the header on the client side.

## Sample DSL works from `curl` but fails from a browser

Almost always CORS. See [CORS](../framework/cors.md) — set `cors.allowed_origins`.

## Boa `Script evaluation error: RuntimeLimit`

Cause: `${...}` hit `max_loop_iterations` (default 1 000 000). Use the [`iterate` step](../dsl/steps/iterate.md) instead of a JS loop.

## Where do I look for logs?

```bash
docker compose logs -f ruuter-rs               # follow
docker compose logs --since 15m ruuter-rs      # last 15 min
RUST_LOG=debug docker compose up               # more verbose
```
