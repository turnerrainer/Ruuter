# Internal-only DSLs

**Status: v0.12.0-rc (issue #143).** Opt-in. Zero wire change on
upgrade from v0.11.x without `ruuter.yaml` changes.

A DSL marked `internal: true` is **not reachable via external HTTP**.
The dispatcher returns `404 Not Found` (not `403`, to avoid leaking
that the route exists) for any external request. In-process callers —
`template:` sub-calls and self-call-shortcircuited `http.*` steps —
reach the DSL normally.

Motivated by `kemit-ee/ljvis-2#515`: a `ruuter-internal` service with
36 cron / X-Road / admin DSLs externally reachable with no auth.
Shared-secret guards (`required_one_of` on
`X-Internal-Service-Token`) were the only workaround — per-DSL
opt-in, easy to miss, secret-leak sensitive, not an engine invariant.

## The three-level fallback

An absent `declaration.internal` resolves top-to-bottom; the first
level that returns a value wins:

1. **Per-DSL** — `declaration.internal` on the DSL itself.
2. **Per-instance** — `declarations.default_internal` in `ruuter.yaml`.
3. **Framework default** — hard-coded `false` (public). Guarantees
   that an upgrade without touching `ruuter.yaml` keeps every route
   public. The feature is strictly opt-in.

```
┌─────────────────────────────────────────────────────────┐
│ declaration.internal set on this DSL?                   │
│     ├── true  → external HTTP returns 404               │
│     └── false → external HTTP routed normally           │
│         (also used as explicit opt-out when operator    │
│          flipped the per-instance default to `true`)    │
├─────────────────────────────────────────────────────────┤
│ not set — resolve via declarations.default_internal:    │
│     ├── true  → external HTTP returns 404               │
│     ├── false → external HTTP routed normally           │
│     └── (block absent) → framework default `false`      │
│                          → external HTTP routed normally│
└─────────────────────────────────────────────────────────┘
```

## Marking a DSL internal

```yaml
# DSL/svc/POST/internal/webhook-callback.yml
declaration:
  internal: true
  description: "Called by resql via template:; never from the public API."

s:
  state:
    set:
      key: last_callback_id
      value: "${incoming.body.callback_id}"
  next: r

r:
  return: ok
  next: end
```

External `POST /svc/internal/webhook-callback` → `404`. A sibling
public DSL can still invoke it:

```yaml
# DSL/svc/POST/public/trigger.yml
declaration:
  internal: false

s:
  template: internal/webhook-callback
  request_type: POST
  body:
    callback_id: "${incoming.body.id}"
  result: inner
  next: r

r:
  return: '${inner}'
  next: end
```

## Operator-level default (private-by-default instance)

For a `ruuter-internal`-shaped deployment where **every** DSL should
be private unless explicitly opted out, flip the per-instance
default:

```yaml
# ruuter.yaml
declarations:
  default_internal: true
  missing_internal_policy: silent     # silent | warn | error
```

Then DSLs that legitimately expose a public surface opt out:

```yaml
# DSL/svc/POST/public-webhook.yml
declaration:
  internal: false      # opt out of the operator-level default
```

Every other DSL in the tree — the ones that forgot to declare
anything — stays private.

## Boot-time policy — `missing_internal_policy`

Controls what happens when a DSL has no `declaration.internal` AND
the operator-level default is explicitly set:

| Value | Behaviour |
|---|---|
| `silent` (default) | No boot output. Keeps upgrade logs quiet. |
| `warn` | One WARN per DSL at boot naming the file path. Opt-in signal that the config gap exists. |
| `error` | Refuse to boot. Fail-fast posture for strict deployments that require every DSL to declare its reachability. |

Default `silent` was chosen so that upgrading from a pre-feature
release emits zero new log output. Operators who want upgrade-time
visibility flip to `warn`.

## CI enforcement — `dsl-lint --require-internal-explicit`

Non-strict `dsl-lint` is silent about missing `declaration.internal`
— the config block's `missing_internal_policy` handles runtime
diagnostics. For CI pipelines that want to catch the gap at build
time:

```bash
dsl-lint --dsl DSL --constants constants.ini --require-internal-explicit
```

Errors on any HTTP DSL that omits `declaration.internal`. Opt-in; a
default `dsl-lint` run is unchanged.

## Observability

- **`/_/openapi.json`** — operations whose DSL explicitly declares
  `internal: true` get an `x-internal: true` extension. The
  operator-level fallback does **not** synthesize the extension —
  only explicit per-DSL `true` is marked, so flipping
  `default_internal: true` doesn't silently relabel the whole spec
  as internal.
- **`/_/unguarded`** — every audited route gains an `internal: bool`
  field alongside `guards`. Operators can tell "externally reachable
  without a guard" (the actual risk surface) from "internal and not
  reachable at all" (declared private).

## Semantics in detail

### What the gate does

In `DslRouter::handle_request_inner`, after the pass-through proxy
early-dispatch but BEFORE the 16 MiB inbound Content-Length
preflight:

1. Resolve `(project, method, endpoint_path)` to a candidate DSL
   via `DslRouter::resolve_dsl_with_path_params`.
2. If no DSL resolves, fall through to the normal 404 path.
3. If a DSL resolves, call
   `Dsl::effective_internal(config.declarations.default_internal)`.
4. If the result is `true`, return `404 Not Found` with the
   standard `{"error":"Not Found"}` body. Log at DEBUG only — the
   gate avoids giving a pentester a signal.

### Why it bypasses in-process paths

The `template:` step and the self-call short-circuit both call
`DslRouter::execute_dsl` **directly** — they don't go through the
axum request pipeline where `handle_request_inner` lives. So the
gate is physically unreachable from in-process callers, which is
exactly the semantics we want: an internal DSL can still be called
by other DSLs in this process, just not from the outside.

### Guards still run

Internal DSLs reached via `template:` or self-call still execute
their guard chain. Internal classification is a **reachability**
control, not an **authorization** control. A DSL that wants to skip
guards must declare `override_ancestors: true` on an applicable
guard — same escape hatch as before (see [Guards](./guards.md)).

### Interaction with pass-through proxy

A `declaration.proxy:` route is dispatched to the proxy handler
BEFORE the internal gate check runs — proxy routes have their own
dispatch path at the top of `handle_request_inner`. If you want a
proxy route to be internal-only, you can do it, but it currently
falls under "operator responsibility" — the proxy handler itself
doesn't consult `effective_internal`. If this becomes a real
requirement, file a follow-up issue.

## Migration

Nothing. Both the per-DSL field and the config block are additive
with safe defaults.

- Existing DSLs — no edits needed. Framework default `false` keeps
  them public.
- Existing `ruuter.yaml` — no edits needed. Absent `declarations:`
  block means every DSL stays public.
- Upgrade logs — no new WARN output by default
  (`missing_internal_policy: silent`).

For deployments adopting the feature:

1. Audit the DSL tree. `GET /_/unguarded` now returns an
   `internal: bool` field — use it to decide which routes should
   flip to `internal: true` and which stay public.
2. Flip `default_internal: true` on `ruuter-internal`-shaped
   instances; add explicit `internal: false` on the handful of
   routes that are genuinely public.
3. Add `dsl-lint --require-internal-explicit` to CI if you want
   build-time enforcement.
4. Optionally flip `missing_internal_policy: warn` for a release or
   two to catch any DSL that slipped through the audit; move back to
   `silent` once the tree is clean.

## Alternatives considered

- **Filesystem convention `/DSL/<project>/private/*`.** Mirrors
  existing idioms (`GET/`, `.guard.yml`, `WS/`) and is impossible to
  forget. Rejected because flipping a DSL from internal → external
  becomes `git mv` instead of a 1-line edit; nested-method paths
  get awkward (`/DSL/project/private/POST/foo.yml`); per-instance
  operator defaults can't be expressed in a filesystem layout.
- **Hybrid filesystem + flag with DSL overriding.** Two sources of
  truth; reviewer cost. Rejected.
- **Private by default globally.** Breaks every existing Ruuter
  deployment on upgrade. Non-starter for an additive feature.

## See also

- [`declaration` step — `internal` field](./steps/declaration.md#internal-issue-143)
- [Configuration — `declarations:` block](../ops/configuration.md#internal-only-dsls-issue-143)
- [`/_/openapi.json` + `/_/unguarded`](../framework/endpoints.md)
- [Self-call short-circuit](../framework/self-call-optimization.md)
- [Request pipeline](../framework/pipeline.md)
- [Security hardening checklist](../ops/security-checklist.md)
- [`dsl-lint`](../testing/dsl-lint.md)
- [Reserved subdirectories](../reference/reserved-subdirs.md)
- [Self-audit (`/_/audit/dsl`)](../framework/audit-dsl.md) — the Category E `declaration.internal_missing` finding is the runtime equivalent of `dsl-lint --require-internal-explicit`.
