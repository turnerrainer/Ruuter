# 018 — Path parameter parity with Java Ruuter

## Why

The Rust router does exact-key lookup only
(`src/router/mod.rs:execute_dsl`). The Java Ruuter accepts arbitrary
trailing segments and exposes them to the DSL as
`incoming.params.pathParams` — documented at
`backup/Ruuter/samples/general/params.md` and implemented at
`backup/Ruuter/src/main/java/ee/buerokratt/ruuter/service/DslService.java:204-215`.

This is a parity gap, not a new feature. Any consumer porting a Java
Ruuter project to the Rust runtime hits the gap on every RESTful
collection-and-item URL (`/things` + `/things/{id}`).

## Java convention (reference implementation)

`DslService.execute()` looks up the requested DSL key. On miss:

1. Strip the trailing path segment.
2. Push the stripped value to the **front** of
   `requestQuery["pathParams"]` (so multi-strip preserves URL order).
3. Recurse with the shortened key.

Effect: a single DSL file at `GET/v2/orders.yml` serves both
`GET /v2/orders` (pathParams=[]) and `GET /v2/orders/abc-123`
(pathParams=["abc-123"]) and `GET /v2/orders/abc-123/legs`
(pathParams=["abc-123","legs"]). The DSL switches on
`pathParams.length` to branch behavior.

Note: there is NO `{name}.yml` filename template in Java Ruuter. Path
params are positional, not named. Naming is the DSL author's job
(`${pathParams[0]}` or local `assign`).

## Scope

1. In `DslRouter::execute_dsl`, on `FileNotFound` from the exact
   lookup, strip the last `/`-segment off `dsl_key`, push the stripped
   token to the front of an accumulator `Vec<String>`, retry.
   Continue until either:
   - a key hits, or
   - the key shortens past `<METHOD>/` (no DSL ancestor exists) → 404.
2. Expose the accumulator under `incoming.params.pathParams` so DSLs
   read it via the existing JS evaluator (no new step type).
3. Guards must still match against the **original** (full) request
   path, not the shortened one — the parent guard at `/admin` should
   still protect `/admin/users/123`. Verify `applicable_guards` still
   walks the unmodified `dsl_key`.

## Files likely touched

- `src/router/mod.rs` (`execute_dsl`, guard application)
- `src/context/` (where `incoming.params` is built)
- `src/scripting/` (only if `pathParams` needs special JS binding;
  the Java version stuffs it into the query map which is already
  exposed)

## Tests

- `GET /samples/scripting/passing-path-parameters/v1/v2/v3` returns
  `["v1","v2","v3"]` (mirrors the Java sample).
- Exact-match still wins: a literal DSL at the full key is preferred
  over the suffix-strip fallback.
- Guard at `/admin` still protects `/admin/users/123` after the
  recursive strip.
- A request that strips down past the project's deepest DSL ancestor
  returns 404 (not 500, not infinite loop).

## Out of scope

- Named path-param syntax (`{id}.yml` filename templates). Java Ruuter
  doesn't have it; introducing it would be a divergence, not parity.
- Greedy / typed params. Same — parity work only.

## Acceptance

The Java sample at `backup/Ruuter/samples/general/params.md` runs
unchanged against the Rust runtime, with identical output.
