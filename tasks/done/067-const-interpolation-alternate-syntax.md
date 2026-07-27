# 067 — Add `#{const}` alternate syntax for constant interpolation

## Filed

2026-07-20 — surfaced during DSL-hygiene discussion for Baye/desk
(iron condor 0DTE DSL). Downstream user proposed the alternate
syntax; keeping [#const] indefinitely for backward-compat.

## Landed

2026-07-26 — Component 1 (parser alias) shipped. Component 2
(`ruuter-lint fix` migration tool) intentionally deferred as
opt-in tooling; not required by any current caller. Implementation
notes:

- Central helper `src/dsl/interpolate.rs` (`substitute`, `iter_refs`,
  `ConstantRef`) — one regex now covers both syntaxes across the
  three call sites that previously each had their own copy.
- Migrated call sites: `src/dsl/parser.rs`, `src/bin/dsl_lint.rs`
  (both the substitution and unresolved-scan spots),
  `src/sources/config.rs::sub`.
- Tests: 9 unit tests in `interpolate.rs` cover both syntaxes,
  mixing, missing keys, adjacency, and non-interference with
  `${runtime}` variables. 4 integration tests in `tests/constants.rs`
  mirror every existing `[#KEY]` scenario with a `#{KEY}` twin
  (parser round-trip, mixed usage, WS source config, and error
  parity for missing keys).
- Docs: `book/src/dsl/constants.md` now documents both syntaxes and
  explains when to pick which. `[#KEY]` is retained for backward
  compatibility with a soft-deprecation stance — may be deprecated
  in a future major release; new DSLs should prefer `#{KEY}`. The
  original proposal body above (which said "keep indefinitely") is
  the historical record at filing time and predates this decision.

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
