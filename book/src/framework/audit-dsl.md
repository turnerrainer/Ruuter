# Self-audit (`/_/audit/dsl`)

**Status: v0.13.0-rc (issue #146).** Admin-gated.

Reports production-readiness gaps in the currently loaded DSL tree:
missing declarations, allowlist drift (declared vs actually used),
security-posture gaps, and reachability-config gaps.

The same engine (`src/dsl/audit.rs`) runs behind `dsl-lint --audit`
for build-time checks against the filesystem; the endpoint reports
against the LIVE tree so hot-reload is reflected.

## Why

The `declaration:` block drives TWO separate things in Ruuter:

1. **OpenAPI generation** (`src/openapi.rs`) — operation description,
   request body / parameters / headers schemas, response schemas
   (`returns:`), `x-internal` extension.
2. **Wire-time security enforcement** (`src/router/mod.rs::apply_declaration`)
   — allowlist filtering (default / `strict:` / `additive:`),
   `required:` / `required_one_of:` presence checks, issue #75
   field-type coercion.

Both fail silently when the declaration drifts from the DSL body.
Fields in the allowlist that the DSL never reads waste client
bandwidth; fields the DSL reads that aren't in the allowlist are
silently stripped (default filter) or 400'd (strict). The audit
surfaces these gaps so operators spot them before a client does.

## Enabling

Admin-gated — same posture as `/_/unguarded`, `/_/openapi.json`,
`/_/sources`, `/_/state-stats`:

```yaml
# docker-compose.yml
environment:
  - RUUTER_ADMIN_ENABLED=true
```

## Response shape

```json
{
  "totals": {
    "projects": 3,
    "dsls": 42,
    "errors": 0,
    "warnings": 11,
    "info": 7
  },
  "findings": [
    {
      "project": "ljvis",
      "dsl": "POST/users",
      "severity": "warning",
      "code": "declaration.body.over_declared",
      "message": "...",
      "fields": ["note"]
    }
  ]
}
```

- **Flat list, not nested by project.** Dashboards `group_by` on any
  field.
- **Sorted** by `(project, dsl, code)` — polls across time produce
  stable diffs.
- **`code`** is a stable string key (adding one is minor-bump surface,
  renaming an existing one is breaking).
- **`severity`** — `error` = unambiguously broken, `warning` = drift
  / posture gap, `info` = soft signal.

## Severity model

| Severity | Fail CI? | Meaning |
|---|---|---|
| `error` | yes (via `dsl-lint --audit`) | Unambiguously broken; operator must fix. |
| `warning` | no | Drift or posture gap; operator should fix. |
| `info` | no | Soft signal; nice-to-have (missing description, no `internal:` field, etc.). |

Operators who want to fail CI on warnings grep the JSON output; the
default exit code only flips on `error` findings.

## Check catalogue (v1)

Grouped by category — see issue #146 for the design rationale.

### Category A — Declaration completeness (OpenAPI quality)

| Code | Severity |
|---|---|
| `declaration.missing` | warning |
| `declaration.description_missing` | info |
| `declaration.returns_missing` | info |
| `declaration.legacy_flat_allowlist` | info |
| `declaration.body.type_missing` | info |

### Category B — Allowlist drift (declared vs actually used)

| Code | Severity |
|---|---|
| `declaration.body.over_declared` | warning |
| `declaration.body.under_declared` | warning |
| `declaration.params.over_declared` | warning |
| `declaration.params.under_declared` | warning |
| `declaration.headers.over_declared` | warning |
| `declaration.headers.under_declared` | warning |

Framework-level headers (`authorization`, `traceparent`,
`content-type`, `x-trace-id`) are excluded from the over/under
checks — OpenAPI needs them, the DSL may legitimately not reference
them.

### Category C — Security posture gaps (shipped in v1)

| Code | Severity |
|---|---|
| `declaration.required_but_unused` | warning |

### Category E — Internal + reachability (ties to issue #143)

| Code | Severity |
|---|---|
| `declaration.internal_missing` | info |

### Promoted to parse-time errors (deliverable 1)

These are blocking parse errors — the DSL fails to load, previous
hot-reload version stays in place. They are NOT reported by the
audit endpoint (the broken DSL isn't in the loaded tree):

- `strict: true` without any allowlist (meaningless posture).
- `required_one_of` referencing a field not in THIS DSL's allowlist
  (resolution is purely local; guards don't contribute).
- Body allowlist on `GET` / `DELETE` routes (bodyless methods — the
  declaration is inert).
- `strict: true` + `additive: true` (contradictory posture; parser
  already caught, re-pinned under issue #146).

### Deferred to v2

- `declaration.unchecked_dereference` — DSL unconditionally
  dereferences `${incoming.body.<f>}` outside a `switch:` null
  check. Needs expression AST walking + flow analysis.
- `declaration.untyped_arithmetic` — arithmetic on an untyped body
  field. Needs expression AST operator inference.
- Category D (`guard_required_header_dropped_by_target`,
  `guard_type_mismatch`) — cross-DSL contract checks.
- Category F (`returns.status_mismatch`) — DSL `return:` statuses
  vs declared `returns:` schema.

Follow-up tracked in the issue #146 thread.

## Running the audit from CI

The same engine runs behind a `dsl-lint` flag:

```bash
dsl-lint --dsl DSL --constants constants.ini --audit
```

- Findings with `error` severity flip the exit code; `warning` /
  `info` print but don't fail. CI jobs that want strict-mode on
  warnings can grep `--json` output.
- Pairs with the existing `--require-guard` (issue #45) and
  `--require-internal-explicit` (issue #143) flags — orthogonal
  check modes.

## Related

- [`/_/unguarded`](./endpoints.md#_unguarded) — complementary audit
  of guard-chain coverage. A route can appear in both.
- [`declaration` step](../dsl/steps/declaration.md) — the field
  reference the audit's checks are grounded in.
- [Internal-only DSLs](../dsl/internal-dsls.md) — issue #143's
  `declaration.internal` posture that Category E checks for.
- [`dsl-lint`](../testing/dsl-lint.md) — build-time surface that
  calls the same engine.
