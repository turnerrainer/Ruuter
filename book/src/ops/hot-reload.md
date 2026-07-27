# Hot reload

Opt-in filesystem watcher. When enabled, editing a DSL under
`config.config_path` (defaults to `./DSL`) republishes the HTTP + guard
trees without a server restart — the same axum `Router` handle keeps
serving, in-flight requests aren't affected, the swap is atomic.

## Enable

```yaml
# ruuter.yaml
dsl:
  allow_dsl_reloading: true
```

That's the whole surface. Default is `false`. When on, boot log
includes:

```
INFO hot-reload: watching ./DSL (debounce 300 ms)
```

Every subsequent reload logs:

```
INFO hot-reload: republished 61 HTTP DSL(s), 3 guard(s)
```

If a save produces a parse error (bad YAML, missing constant,
unreachable step), the reload is refused and the **previously-published
tree stays live**. A broken save cannot take the server down:

```
ERROR hot-reload: reload failed (previous tree still live): parse error at DSL/samples/GET/broken.yml
```

## Security posture

**Hot reload plus a writable DSL mount is remote code execution via
`${JS}` expressions.** Do not enable in production. The shipped
`docker-compose.yml` mounts `./DSL:/app/DSL:ro` and sets
`read_only: true` on the container filesystem — both defaults hold
even if this flag is on, so an operator flipping it does not by itself
open the RCE hole. But nothing in Ruuter *prevents* the operator from
mounting DSL read-write, so keep the flag off outside dev.

## What reloads, what doesn't

| Reloaded | Not reloaded |
|---|---|
| HTTP DSLs (`<project>/<METHOD>/*.yml`) | Trigger DSLs (`<project>/triggers/**`) |
| Guards (`.guard.yml`) | Source configs (`<project>/sources/**`) |
| OpenAPI cache (`GET /_/openapi.json`) | `constants.ini` |
| `template:` step lookup targets | `ruuter.yaml` (operator config) |
| WebSocket server DSLs (`<project>/WS/*.yml`) — new upgrades resolve against the new tree | Existing WebSocket **connections** (open frame pumps keep running against their originally-loaded DSL) |

For anything in the right column, restart the container. Rationale:

- Reloading trigger DSLs would leave the trigger dispatcher's owned
  map out of sync with the routing tree; not implemented in this pass.
- Reloading sources means graceful teardown + reconnect of live
  WebSocket subscriptions — a hard problem out of scope for a
  dev-focused feature.
- Constants are baked into DSLs at parse time. Changing a value would
  need a full re-parse plus operator awareness that in-flight
  substitutions might mix old and new values — restart is safer.

## How it works (in one screen)

1. `notify` watches `config.config_path` recursively.
2. Filesystem events are coalesced by `notify-debouncer-full` on a
   300 ms window — one editor "Save All" or `git checkout` triggers
   one reload, not dozens.
3. The debounced signal drives one call to
   `DslLoader::load_everything()` with the boot-time constants map.
4. On success, the new HTTP tree and guard tree are stored into the
   shared `ArcSwap` handles that the router and step engine both
   hold. A single atomic store publishes them to all readers.
5. On failure, the previous tree remains live and the error is logged.

Sources: [`src/dsl/hot_reload.rs`](https://github.com/turnerrainer/Ruuter/blob/dev/src/dsl/hot_reload.rs),
[`src/router/mod.rs`](https://github.com/turnerrainer/Ruuter/blob/dev/src/router/mod.rs) (`publish_dsls`, `from_shared`).

## Verification

Integration tests in `tests/hot_reload.rs` exercise the publish
primitive directly (add / rewrite / remove a DSL, assert the
route becomes / no longer available on the same router `Arc`) and
confirm the OpenAPI cache is rebuilt.
