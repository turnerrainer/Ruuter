# 066 — Add `#{const}` alternate syntax for constant interpolation

## Filed

2026-07-20 — surfaced during DSL-hygiene discussion for Baye/desk
(iron condor 0DTE DSL). Downstream user proposed the alternate
syntax; keeping [#const] indefinitely for backward-compat.

## Severity

**Low** — additive feature, no correctness fix. Improves DSL
readability + primitive symmetry.

## Motivation

Ruuter DSL currently uses two interpolation forms:
- `${variable.name}` — runtime variables (evaluated at request time)
- `[#constant.name]` — compile-time constants (baked from
  `constants.ini` at Ruuter startup)

The asymmetry (`${}` runtime vs `[#]` constant) is confusing at first
read, especially for developers coming from templating languages that
use `${}` / `#{}` conventions (JSP EL, Thymeleaf, Spring EL, etc.).

Downstream DSL authors (desk team on iron condor) requested a
forward-compatible constant syntax that visually mirrors `${}`.

## Proposal

Add `#{constant.name}` as an alias for `[#constant.name]` in the
Ruuter template tokenizer. Both syntaxes tokenize identically → same
AST → identical runtime behavior.

**No forced deprecation**. `[#]` continues to work indefinitely.
Migration is opportunistic and DSL-author-controlled.

## Fix

Two components (both required):

### Component 1 — parser alias

`src/dsl/template.rs` (or wherever the interpolation tokenizer lives):

- Extend the tokenizer to recognize `#{...}` in the same lexer state
  that currently handles `[#...]`
- Both produce the same `Constant(ident)` token
- Semantic layer downstream is unaware which surface syntax was used
- Test: every existing test that uses `[#foo]` gets a mirrored test
  using `#{foo}`; both must produce identical AST output and runtime
  behavior

### Component 2 — optional lint

New crate or module `ruuter-lint` (or extend existing if there is one):

- Command `ruuter-lint check path/to/dsl/` — grep DSL files, warn on
  `[#foo]` occurrences (opt-in via config flag, off by default)
- Command `ruuter-lint fix path/to/dsl/` — grep-swap `[#foo]` →
  `#{foo}` across a file or directory
- Deterministic: only touches literal `[#name]` patterns; doesn't
  interpret DSL semantics; safe to run in bulk

The lint is fully opt-in. Ruuter parser doesn't emit any warning by
default when `[#]` is encountered — dual syntax coexists silently.

## Testing

Minimum coverage:

- **Parser equivalence**: for every existing `[#foo]` test case,
  add a mirrored `#{foo}` test. Assert same AST + same rendered
  output.
- **Mixed usage**: a single DSL file containing both `[#a]` and
  `#{b}` in the same template resolves both correctly.
- **Escape handling**: verify `\#{escaped}` and `\[#escaped]` both
  render literally (or match whatever escape convention Ruuter uses
  today for `[#]`).
- **Lint golden tests**: input DSL file + expected `ruuter-lint fix`
  output diff.

## Out of scope for this task

- Deprecation of `[#]` — not proposed. Keep indefinitely.
- Migration of existing DSL files — DSL authors' choice; ruuter-lint
  fix is available on demand but not run automatically.
- Any semantic change to constant resolution — this is purely a
  surface-syntax addition.

## Estimated effort

- Parser alias: ~2-3 days (small tokenizer change + comprehensive tests)
- Lint tool: ~3-5 days (new binary or module + golden tests + docs)
- Documentation update (Ruuter book / README): ~1 day

Total: ~1-1.5 weeks.

## Not blocking anything

Purely additive. Downstream users continue on existing `[#]` syntax
until they choose to migrate. Recommend shipping in the next Ruuter
maintenance window without prioritization pressure.

## Related

- Downstream DSL author's fuller context (not required to read, but
  explains the motivation): `Baye/poc/options/0dte-spy-condor/docs/backlog/v0.2-dsl-hygiene-syntax-and-casing.md`
  in the Baye-Quant strategies repo.
- The camelCase-convention lint idea from the same discussion is a
  DIFFERENT concern (DSL-author style guide, not Ruuter parser
  feature). Ruuter could support it as a scope-aware lint rule
  extension IF an author-facing style-check API is added later, but
  that's a separate task not filed here.
