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
| `WS/`               | WebSocket server DSLs (`ws://.../<project>/<path>`) | no (WS upgrade) |
| `triggers/`         | Trigger DSLs dispatched from sources | no |
| `sources/`          | WS source configurations | no |
| `cronmanager-jobs/` | Companion CronManager configs | no |

Any subdirectory not on this list is treated as an HTTP method name and its files become routes under that method — usually a bug. Keep custom directories under `triggers/` or `sources/` or move them outside the DSL tree entirely.

## Guard files

Guards can appear at any depth:

- Sibling: `<method>/<path>/<stem>.guard.yml` → protects `<method>/<path>/<stem>/*`
- In-folder: `<method>/<path>/<dir>/.guard.yml` → protects `<method>/<path>/<dir>/*`
- Bare extension-less: `<method>/<path>/<dir>/.guard` — also accepted (strict Java parity).
