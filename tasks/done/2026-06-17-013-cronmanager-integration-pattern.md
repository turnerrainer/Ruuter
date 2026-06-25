# 013 — Document the CronManager → Ruuter integration pattern

**Status**: BACKLOG.
**Severity**: LOW (no code change needed; documentation gap).
**Effort**: 1-2 hours.
**Filed**: 2026-06-17.
**Replaces**: #006 (cancelled — scheduling is CronManager's role,
not Ruuter's).

## Why this exists

When #006 was filed I assumed Ruuter needed an in-process cron source.
After reading the Buerostack architecture, that was wrong: scheduling
is **CronManager's** dedicated component
(`/home/rainer/Desktop/Buerostack/CronManager`, port 9010, Quartz-
backed). The right pattern is:

```
CronManager (cron expression) → HTTP POST → Ruuter route → DSL
```

There's no Ruuter code to write — a scheduled job is just a regular
HTTP route. But there's no documented sample either, which makes the
pattern non-obvious to anyone wiring up their first scheduled service.

## What to add

1. **Sample DSL** at `DSL/samples/POST/scheduled/heartbeat.yml`
   — a trivial endpoint that writes a timestamp into `StateStore`
   to prove the wire-up.

2. **Sample CronManager job** at
   `DSL/samples/cronmanager-jobs/heartbeat.yaml` (just an example
   file, not loaded by Ruuter — illustrative). Shows the expected
   shape:

   ```yaml
   heartbeat_ping:
     trigger: "0 */1 * * * ?"
     type: http
     method: POST
     url: http://ruuter:8080/samples/scheduled/heartbeat
     headers:
       Content-Type: application/json
     body: '{"reason": "scheduled"}'
   ```

3. **README section** in this repo's top-level `README.md` titled
   "Scheduled jobs" explaining: scheduling lives in CronManager;
   Ruuter exposes the work as a normal HTTP DSL; CronManager hits
   that URL on cron.

4. **Auth note**: CronManager → Ruuter requests typically need a
   shared secret header (e.g. `X-Internal-Caller`) verified by a
   Ruuter guard. Note this is `guards` territory (currently a
   placeholder in Ruuter per README.md) — file a follow-up if a
   guard primitive is needed before this can be production-safe.

## Verification

- `docker-compose up` Ruuter, point a CronManager instance at
  `samples/scheduled/heartbeat` every minute, observe one `last_seen`
  update per minute in `StateStore` via a `GET` companion DSL.

## Why this is generic

The integration pattern is documentation + samples. No service-
specific code lands in Ruuter. The sample CronManager job is
illustrative and lives outside the Ruuter loader's scope.
