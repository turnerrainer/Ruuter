# 006 — Generic cron/timer source — **CANCELLED 2026-06-17**

**Status**: CANCELLED (architectural misalignment).
**Severity**: N/A — task is being closed without implementation.
**Cancelled by**: Rainer Türner / architecture review on 2026-06-17.

## Why cancelled

Scheduling is **CronManager's** dedicated role in the Buerostack
ecosystem (`/home/rainer/Desktop/Buerostack/CronManager`). CronManager
is already a deployed, Quartz-backed service that:

- Reads job definitions from YAML (cron expression + HTTP/shell action)
- Fires scheduled HTTP requests on demand
- Provides REST APIs for job management (`/jobs`, `/execute/...`, `/running`)
- Supports full Quartz cron syntax, time boundaries, manual triggers

The Buerostack architecture explicitly assigns each component one
role, and CronManager owns scheduling. Adding cron support to Ruuter
would duplicate functionality and violate the "every component has
its own dedicated role" principle.

## The correct pattern

For any service needing scheduled work:

1. Define the job in CronManager's `DSL/<group>/<job>.yaml`:
   ```yaml
   force_flatten:
     trigger: "0 55 15 * * MON-FRI"
     type: http
     method: POST
     url: http://ruuter:8080/<project>/POST/scheduled/force-flatten
   ```

2. In Ruuter, this is just a regular HTTP route — no new mechanism:
   ```
   DSL/<project>/POST/scheduled/force-flatten.yml
   ```

Ruuter's #004 trigger directory remains correct for **non-HTTP push
sources** only (WebSocket today; future MQTT, Kafka, etc.). HTTP-
triggered scheduling lives at the existing HTTP entrypoint, fired by
CronManager.

## Follow-up

See task #013 — documentation + sample DSLs showing the CronManager →
Ruuter integration pattern.
