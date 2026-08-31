# Reserved subdirectories

Under `DSL/<project>/`:

| Subdirectory        | Purpose | Routed as HTTP? |
|---------------------|---------|-----------------|
| `GET/`              | HTTP GET routes | yes |
| `POST/`             | HTTP POST routes | yes |
| `PUT/`              | HTTP PUT routes | yes |
| `PATCH/`            | HTTP PATCH routes | yes |
| `DELETE/`           | HTTP DELETE routes | yes |
| `OPTIONS/`          | HTTP OPTIONS routes | yes |
| `WS/inbound/`       | WebSocket server DSLs (`ws://.../<project>/<path>`) | no (WS upgrade) |
| `WS/outbound/`      | WebSocket source configurations (outbound feeds) | no |
| `WS/` (legacy)      | Legacy — files directly under `WS/` still load as inbound handlers with a WARN | no (WS upgrade) |
| `sources/` (legacy) | Legacy — WS source configs still load with a WARN; rename to `WS/outbound/` | no |
| `triggers/`         | Trigger DSLs dispatched from `WS/outbound/` feeds | no |
| `cronmanager-jobs/` | Documentation-only — companion CronManager configs. Ruuter does not schedule or execute jobs from this directory. | no |

Any subdirectory not on this list is treated as an HTTP method name
and its files become routes under that method — usually a bug. Keep
custom directories under `triggers/`, `WS/outbound/`, or move them
outside the DSL tree entirely.

## WS layout — new vs legacy

- **New (preferred):** `WS/inbound/<path>.yml` for handshake DSLs,
  `WS/outbound/<name>.yml` for outbound feeds. Introduced 2026-08-05.
- **Legacy (still accepted with WARN):** `WS/<path>.yml` directly and
  `sources/<name>.yml`. Both paths still load; the boot log emits a
  WARN pointing at the canonical replacement. Rename before v1.
- When BOTH `WS/outbound/` and `sources/` are present, `WS/outbound/`
  wins and the loader emits a WARN naming the collision.

## Guard files

Guards can appear at any depth:

- Sibling: `<method>/<path>/<stem>.guard.yml` → protects `<method>/<path>/<stem>/*`
- In-folder: `<method>/<path>/<dir>/.guard.yml` → protects `<method>/<path>/<dir>/*`
- Bare extension-less: `<method>/<path>/<dir>/.guard` — also accepted (strict Java parity).
- **Project-level (issue #39): `<project>/.guard.yml` → protects every DSL in the project across every HTTP method.** Only one per project; two variants at the project root (`.guard.yml` alongside `.guard.yaml`, etc.) is a load-time error naming both offending files.

See [Guards mode](../config/guards-mode.md) for stacked vs closest-only
evaluation and [Guards DSL reference](../dsl/guards.md) for the file
conventions and the `override_ancestors` escape hatch.
