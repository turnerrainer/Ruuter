# 019 — Guards-in-folder parity with Java Ruuter

## Why

The Rust router today recognizes guards only as **sibling files**:
`<METHOD>/<dir>.guard.yml` protects `<METHOD>/<dir>/*`.

The Java Ruuter convention (and the documented original) is **guards
inside the folder they protect**, as a literal `.guard` file (no
extension):
- `DSL/GET/template-test/.guard`
- `DSL/GET/guards/ok/.guard`
- `DSL/GET/guards/fail/.guard`

(See `backup/Ruuter/DSL/GET/`.) The Rust port's sibling pattern is a
divergence; consumers porting Java DSL trees expect the in-folder
convention to work.

## What breaks today

A consumer authoring `DSL/<proj>/POST/<dir>/.guard.yml` (the closest
Rust-compatible attempt — hidden file + `.yml` extension):

1. `is_processable_file` accepts it (extension is `yml`). ✓
2. `is_guard_file` accepts it (filename contains `.guard.`). ✓
3. `build_guard_key`:
   - `rsplitn(3, '.')` on `.guard.yml` yields `["yml", "guard", ""]`,
     `nth(2)` returns `""` (the leading-dot fragment).
   - With empty stem and parent `<dir>`, the computed guard_key
     becomes `<METHOD>/<dir>/` (trailing slash).
4. `applicable_guards` does `format!("{}/", guard_key)` →
   `<METHOD>/<dir>//`. A DSL at `<METHOD>/<dir>/foo` does **not**
   start with that double-slash prefix → guard never applies.

Result: silently disabled guard.

## Fix

1. In `is_processable_file`: also accept literal filename `.guard`
   (no extension) so the original Java convention works.
2. In `build_guard_key`: handle the in-folder case explicitly. If the
   filename is `.guard` or starts with `.guard.`, set the guard_key
   to the containing directory's rel-path (no trailing slash):
   `<METHOD>/<parent-dir>`.
3. Keep the existing `<name>.guard.yml` sibling convention working
   for back-compat with any consumer that already adopted it.
4. `applicable_guards` is unchanged — both keys end up shaped the
   same (`<METHOD>/<dir>`), and the slash-prefix match works.

## Tests

- `DSL/samples/GET/guards-in-folder/.guard.yml` (or `.guard`)
  protects `DSL/samples/GET/guards-in-folder/data.yml`. A request
  to `/samples/guards-in-folder/data` runs the guard first.
- The existing sibling `<dir>.guard.yml` sample keeps passing.
- A request to a DSL whose ancestor folder has a `.guard` file but
  whose own folder does not still gets guarded by the ancestor
  (hierarchical guard composition still works).

## Out of scope

- New guard semantics (multi-condition stacking, opt-out per route,
  etc.). Parity work only.

## Acceptance

Java Ruuter consumers can copy their DSL tree into the Rust runtime
unchanged and guards apply correctly.
