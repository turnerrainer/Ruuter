# 008 — Source lifecycle + supervisor

**Status**: BACKLOG.
**Severity**: MEDIUM (one bad source config should not take down the
process or starve other sources).
**Effort**: 0.5 day.
**Filed**: 2026-06-17.
**Blocked by**: #005, #006.

## What's wrong

Once #005 and #006 land, each source is a long-lived tokio task.
Without supervision:
- A panic in one source silently kills only that source.
- There's no way to observe source health.
- Hot reload (future) can't shut sources down cleanly.

## Fix

Introduce a `SourceSupervisor` that:

1. Owns a `JoinSet` of source tasks, each tagged with `(project,
   source_name, kind)`.
2. Wraps each task in `tokio::spawn` with a `catch_unwind`-style
   guard. On panic: log, increment a metric, restart with backoff.
3. Exposes `GET /_/sources` (built-in admin route, behind a config
   flag) returning each source's current state: `connected`,
   `reconnecting`, `dead`, last error, last activity timestamp.
4. Exposes `tracing` spans `source.{kind}.{project}.{name}` so
   OpenTelemetry traces tie events back to their source.

## Verification

- Force-panic a source via test harness; observe restart + metric
  bump.
- Hit `/_/sources` while a source is reconnecting; observe state.

## Why this is generic

Supervisor knows only about `(project, name, kind)` strings. No
service-specific behaviour.
