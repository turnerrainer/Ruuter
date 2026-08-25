# 070 — Declaration parity with Resql (typed params + returns + strict posture)

## Closed

2026-08-25 — all five coordinated changes landed. Live-verified
against `DSL/samples/POST/typed-users/create.yml`; strict posture
returns 400 with a diagnostic naming the unknown field, typed
`requestBody` + `response_201` schemas visible at
`GET /_/openapi.json`, boot-time WARN gated by
`dsl.warn_on_missing_declaration` (default on). Per operator
instruction: missing declaration NEVER halts Ruuter — it just
WARNs (silenceable via config).

- 12 new integration tests in `tests/declaration_parity.rs` (all green).
- Book: `book/src/dsl/steps/declaration.md` rewritten to cover the
  full richer shape; `book/src/ops/configuration.md` documents the
  new `dsl.warn_on_missing_declaration` toggle.
- Sample: `DSL/samples/POST/typed-users/create.yml` demonstrates
  the rich shape end-to-end.
- Divergences: D-39 added (declaration parity extension over Java).

## Filed

2026-08-25 — filed as a sibling to Resql task 008
(`Resql-on-Rust/tasks/backlog/008-declaration-section.md`). Resql is
adopting a mandatory YAML declaration block at the top of every
`.sql` file — modelled on Ruuter's existing `declaration:` step —
and using it for typed parameter validation + OpenAPI 3.1 generation.
Because Ruuter and Resql are siblings from the same operator's
perspective, their declaration semantics should converge instead of
drift. This task audits Ruuter's current declaration model against
the richer Resql shape and closes the gaps that a partner would
reasonably expect to be equivalent.

## Severity

**Medium.** Nothing broken today; DSLs run, OpenAPI generates. But
partner-facing spec quality is lower than it needs to be (every
field types as `string`, every field marks `required: false`, every
response shape is `object, additionalProperties: true`), and the
runtime allowlist semantics are permissive in a way that surprises
operators who assume a validation surface. Both matter more once
Resql ships #008 and consumers start asking why Ruuter's spec is
thinner than the sibling service.

## Current shortcomings (findings, 2026-08-25)

Reading `src/dsl/mod.rs::DeclarationStep`, `src/openapi.rs`, and
`src/router/mod.rs:300-350`:

1. **Field types are not modelled.** `DeclarationStep` carries
   `allowed_body: Option<Vec<String>>` — a list of names, nothing
   more. `Allowlist { body: Vec<DslField> }` where `DslField` has a
   single `field: String` field. The comment on `DslField`
   (`src/dsl/mod.rs:66-69`) explicitly notes Java "reserves room for
   per-field metadata like `format:`, `required:`" but that room has
   never been used at runtime or in the OpenAPI generator.

2. **All OpenAPI parameters are `string, required: false`.**
   `openapi.rs::build_parameters` (lines 262-290) and
   `build_request_body` (lines 292-315) emit
   `{"type": "string"}` for every field with `required: false` on
   parameters and no `required` array on the body schema. Callers
   generating a client see no distinction between "must send" and
   "may send," and every field arrives as a string.

3. **No `returns:` — response shape is opaque.** `build_responses`
   (lines 209-260) emits `{"type": "object", "additionalProperties":
   true}` for every 2xx. There is no way to declare the response
   body shape at DSL authoring time, so downstream clients cannot
   type-check their side of the wire.

4. **Unknown-key posture is filter-and-continue.**
   `router/mod.rs:308-349` calls `filter_str_keyed` on body / query /
   headers to strip anything not in the allowlist, then continues.
   No error, no log. An operator writing a DSL that mistypes a
   consumer's field name (`user_name` instead of `userName`) never
   finds out — the field silently disappears from `${incoming.body}`
   and the branch that depended on it takes the default path. Resql
   is taking the opposite stance in #008 (reject unknown key with
   400); worth reconsidering here too, at least behind a per-DSL
   `strict: true` flag on the declaration to preserve back-compat.

5. **`method:` / `accepts:` / `returns:` fields exist on
   `DeclarationStep` but nothing reads them.** `namespace`,
   `version`, `description` are consumed by `openapi.rs`; the other
   three are dead fields. Either wire them up or delete them from
   the struct so operators aren't misled about which knobs exist.

6. **Declaration is optional.** Missing declaration = permissive
   (matches Java). Resql #008 takes the opposite stance
   (mandatory). Ruuter can't make declarations mandatory without a
   corpus migration, but should log a boot-time diagnostic per DSL
   that has no declaration, so an operator moving from Resql
   understands why their Ruuter tree behaves less strictly.

## Fix / Design

Land as five coordinated changes (each independently testable):

### 1. Rich per-field metadata on `Allowlist`

Extend `DslField` from `{field: String}` to:

```rust
pub struct DslField {
    pub field: String,
    pub r#type: Option<String>,      // string|integer|number|boolean|array|object|date|datetime|uuid
    pub required: Option<bool>,      // default: false
    pub format: Option<String>,      // OpenAPI format hint
    pub description: Option<String>,
    pub default: Option<Value>,
    pub items: Option<Box<DslField>>, // when type == array
}
```

Backwards-compat: a bare `- field: userName` YAML entry still parses
(all new fields Option-default to None). Java-parity form
`- {field: userName}` still works. A richer entry
`- {field: userName, type: string, required: true}` opts into the
new behaviour.

Keep the flat `allowed_body: Vec<String>` shape working; it maps to
`[{field: X, type: None, required: false}]` internally. Removing it
is out of scope (would break every existing DSL in the corpus).

### 2. OpenAPI enrichment

Update `openapi.rs::build_parameters` and `build_request_body` to
emit the declared type, `required` array, and `format` when the
richer entries are present. Fall back to today's `type: string,
required: false` when they aren't. Type set matches Resql #008
(same table, same semantics). This means a partner who consumes both
services' `openapi.json` sees the same schema vocabulary.

### 3. Add `returns:` for response shape

New optional field on `DeclarationStep`:

```yaml
declaration:
  returns:
    - name: id
      type: integer
    - name: email
      type: string
      nullable: true
```

`openapi.rs::build_responses` uses it for the 200 response schema
(properties + required list). Absent → keep today's
`additionalProperties: true` fallback with a note pointing at the
declaration.

### 4. Optional strict-unknown-keys posture

New optional field on `DeclarationStep`:

```yaml
declaration:
  strict: true    # default: false (Ruuter's traditional behaviour)
```

When `true`, the allowlist check in `router/mod.rs:308-349` rejects
requests carrying unknown keys instead of silently filtering. Error
returned as `RuuterError::DslExecution { step: "declare", message:
"Unexpected field: <name>" }` → 400. Same 400-shape response.
Default `false` preserves back-compat.

Operators who want Resql-equivalent semantics flip the flag. A
follow-up task (call it 071) can consider flipping the default once
the sample corpus and any downstream corpora have been audited.

### 5. Boot diagnostic for DSLs without a declaration

At DSL load time (`src/dsl/loader.rs`), for every HTTP-routed DSL
missing a `declaration:` block, emit a single-line WARN with the
DSL key: `"declaration missing; openapi spec will be blank for this
route"`. Operators moving from Resql will grep for these and know
where to focus.

Also purge the dead fields (`method`, `accepts`, `returns` on
`DeclarationStep` if `returns` doesn't get wired in step 3) or wire
them up. Whichever we pick, no dead knobs in the struct after this
task.

## Acceptance

- [ ] `DslField` accepts `type`, `required`, `format`, `description`,
      `default`, `items` in addition to `field`. Bare-name shape and
      Java-parity `{field: X}` shape continue to parse.
- [ ] `openapi.rs` emits declared types, `required` array on
      request bodies, and `format` where set. Sample DSLs updated
      to demonstrate the richer shape.
- [ ] `returns:` on a DSL declaration flows into a typed 200
      response schema.
- [ ] `strict: true` on a DSL declaration rejects unknown body /
      query / header keys with a 400.
- [ ] DSLs missing a declaration emit a WARN at boot with the DSL
      key.
- [ ] No dead fields on `DeclarationStep` — every `Option<T>` field
      either has a runtime consumer or is deleted with a
      migration note.
- [ ] Golden-file test against `openapi.rs` output covering: rich
      declaration → typed spec, bare declaration → today's spec,
      no declaration → spec still generates.
- [ ] `book/src/reference/declaration.md` (or wherever the current
      declaration docs live) updated with the richer schema, the
      `strict:` flag, and the `returns:` shape.
- [ ] Sample DSLs under `DSL/samples/` include at least one endpoint
      demonstrating the full richer shape.

## Estimated effort

2 days.

- Rich `DslField` + parser + tests: 0.5 day.
- OpenAPI enrichment + golden-file tests: 0.5 day.
- `returns:` wiring + tests: 0.5 day.
- `strict:` flag + router integration + tests: 0.25 day.
- Boot diagnostic + docs + samples: 0.25 day.

## Dependencies

None. Coordinate release notes with Resql #008 so operators see a
single "declaration parity" story across the two services.

## Non-scope

- **Making declarations mandatory (Resql #008 posture).** Requires a
  corpus migration Ruuter can't force on downstream consumers. File
  as a follow-up if operator consensus lands.
- **Auto-generating a Postman collection from the richer spec.** Task
  069 covers Postman generation; this task changes the spec quality
  069 will consume, so 069 gets a downstream quality bump for free.
- **Response schema for non-2xx codes.** Framework baselines (400,
  500) keep the existing `Error` schema.

## Risks

- **Sample DSL corpus size.** Every sample under `DSL/samples/` that
  has a `declaration:` block will need reviewing to confirm the flat
  shape still parses. Mitigation: parser tests using the exact
  YAML that ships in `samples/`.
- **`strict: true` regressions.** Flipping any live DSL to `strict:
  true` could break consumers already sending extra fields. Ship
  the flag opt-in and document loudly.

## Related

- Resql task 008 (`Resql-on-Rust/tasks/backlog/008-declaration-section.md`)
  — the parent driver.
- `src/dsl/mod.rs` — `DeclarationStep`, `Allowlist`, `DslField`.
- `src/openapi.rs` — spec generator; consumers of the richer types.
- `src/router/mod.rs:300-350` — allowlist enforcement site.
- Task 069 (Postman) — indirect beneficiary of the richer spec.
