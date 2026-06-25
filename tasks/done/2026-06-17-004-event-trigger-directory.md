# 004 — Event-trigger DSL directory + dispatcher

**Status**: BACKLOG.
**Severity**: HIGH (prerequisite for any push-driven workload).
**Effort**: 1 day.
**Filed**: 2026-06-17.
**Blocks**: #005.
**Blocked by**: #003 (state) — events without shared state are useless
for the intended workloads.

## What's wrong

Ruuter today only knows how to react to HTTP requests
(`handle_request` in `src/router/mod.rs:167`). DSLs live under
`DSL/<project>/<METHOD>/<path>.yml` and are looked up by HTTP method
+ URL path.

For push-driven sources (WebSocket, MQTT, future Kafka, etc.) the
trigger is not an HTTP request — it's an inbound message. We need a
parallel directory + dispatch path.

## Fix

1. Introduce a new top-level directory inside each project:
   `DSL/<project>/triggers/<channel>/<key>.yml`.
   - `<channel>` is a free-form string chosen by the source config
     (e.g. `bars`, `trade_updates`, `device_telemetry`).
   - `<key>` is matched against the dispatch key derived from the
     inbound message (e.g. a symbol, a topic, a device ID). A literal
     `_default.yml` is matched when no per-key DSL exists.

2. Extend `DslLoader` to also load this `triggers/` tree into a new
   map: `HashMap<(project, channel), HashMap<key, Dsl>>`.

3. Add a `TriggerDispatcher` (lives next to `DslRouter`) that exposes:
   ```rust
   async fn dispatch(&self, project: &str, channel: &str, key: &str,
                     payload: Value) -> Result<()>;
   ```
   It builds an `ExecutionContext` whose `body` is `payload` and whose
   `query`/`headers` are empty maps, then reuses the existing
   `execute_steps` engine.

4. Trigger DSLs do not produce HTTP responses. The `return` step's
   value is logged at debug; non-zero `status` becomes a warn.

## Verification

- Load a sample `DSL/samples/triggers/test/echo.yml` that logs its
  payload; invoke the dispatcher directly from an integration test.
- Verify project + channel isolation: a payload to project A doesn't
  resolve to project B's DSLs.

## Why this is generic

`channel` and `key` are arbitrary strings. Ruuter knows nothing about
Alpaca, MQTT, Kafka, or any specific source — it only knows how to
route inbound messages to YAML pipelines.
