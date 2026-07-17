# 046 — Static evaluation of runtime-independent expressions at DSL load

## Filed

2026-07-18 — layer 4 of the Boa perf roadmap. Extends task 037
(runtime literal fast-path) with load-time hoisting.

## Status

**Deferred (2026-07-14)** — reviewed against the current DSL corpus
on the v0.5 branch during the Boa-perf pass. Every `${...}` expression
in `DSL/samples/**` references at least one runtime binding
(`incoming.*`, previously-assigned variables, `Date.now()`, etc.).

Ultra-safe subset (hoist only expressions whose AST has zero
identifiers/property accesses/calls) → 0 hoistable expressions in the
corpus. Full subset with a `Math.*`/`JSON.*`/etc. allow-list → still
~0 hoistable expressions, because DSL authors write literal values as
YAML literals, not as `${1 + 1}`.

The task's motivating example — `${'https://' + '[#DOMAIN]' + '/v1'}`
after constant substitution — assumes a coding style that isn't used
here. `[#CONSTANT]` appears exactly once in the corpus, and it's a
plain string interpolation, not inside a `${...}`.

Cost of implementation:
- AST walker over `boa_ast::Expression` recursing all variants
- Safe-globals allow-list with the `Date`/`Math.random` carve-outs
- Substitution logic distinguishing whole-string vs interpolation
- Roughly 20-30 tests to prove correctness against runtime path

Given zero measured wins on today's corpus, defer until a DSL pattern
that would benefit emerges (a project heavy on constant-substitution
into expressions, or one that authors static computations for
readability).

Task 037's literal fast-path already handles the actual shipping
optimization on this corpus.

## Problem

Task 037 skips Boa for values that contain **no** `${...}` /
`$=...=$`. But some `${...}` expressions are ALSO effectively
literal — their AST references no runtime bindings:

```yaml
# All static after `[#DOMAIN]` constant substitution runs:
url:      "${'https://' + '[#DOMAIN]' + '/v1'}"      # → "https://api.example.com/v1"
retries:  "${3 + 2}"                                  # → 5
allow:    "${['a','b','c'].join(',')}"                # → "a,b,c"
default:  "${{ok: true, count: 0}}"                   # → {ok: true, count: 0}
```

Every request today re-evaluates these. They can be computed once
at DSL load and replaced with their literal result in the parsed
step representation, after which task 037's runtime fast-path picks
them up as pure literals.

## Fix

DSL load pipeline gains a **static-evaluation pass** after the
`[#KEY]` constant substitution step:

1. For each `${...}` expression, walk the AST looking for references
   to runtime bindings (`incoming.*`, previously-assigned variables,
   step results, `state.*`, etc.).
2. If NO runtime references found → evaluate in a throwaway Boa
   context, capture the resulting JSON `Value`, and substitute the
   entire `${...}` in the source with the JSON representation of
   that value.
3. Loop until fixed-point (no more expressions collapse to literals).
4. After the pass, the DSL tree contains only:
   - Pure literals (task 037 skips Boa entirely).
   - Genuinely-runtime-dependent expressions (unchanged, task 045
     runs them through the pre-parsed path).

## Runtime binding detection

The AST walker needs to know which identifiers are "runtime":

- `incoming` and any property access under it.
- Any variable name that appears as an `assign:` target, `state.get`
  `into:` target, `http.result` name, or `iterate.into` target,
  ANYWHERE in the same DSL. (Safe overestimate — treat any name
  that could be runtime as runtime.)
- Free identifiers not in the runtime set AND not in JS globals →
  treat as errors at load time (undefined variable, would fail at
  runtime anyway; better to catch early).

Anything else — `Math.*`, `Date.*` (unless the expression semantics
require per-request time — see gotcha below), `JSON.*`, literal
values, arithmetic on literals — is static-evaluable.

## `Date` gotcha

`${new Date().toISOString()}` looks static (no runtime bindings)
but evaluates to a different value per request. Explicit rule:
treat `Date.now`, `new Date`, `Date.UTC`, `performance.now`, and
any other time-source as runtime references, never hoist. Similar
treatment for `Math.random`.

Small allow-list of hoistable "impure but stable" globals:
`Math.PI`, `Math.E`, `Number.MAX_SAFE_INTEGER`, `String.fromCharCode`,
etc. Documented explicitly.

## Interaction with tasks 037, 045

- 046 runs FIRST at DSL load, collapses static expressions to
  literals.
- 037's runtime fast-path then picks up the collapsed literals as
  expression-free values.
- 045 pre-parses whatever `${...}` remains (only genuinely
  runtime-dependent ones).

Combined effect: at runtime, Boa only ever evaluates expressions
that actually need runtime data. Everything else is pre-computed.

## Acceptance

- Static-eval pass runs after constant substitution in
  `DslParser::parse_file`.
- Collapsed literals visible in the parsed step representation
  (verified via a `--dump-parsed-dsl` CLI flag or equivalent).
- `Date`, `Math.random`, and other per-invocation-varying globals
  are on an allow-list check; misclassifying them as static must
  cause a load-time error, not a silent bad evaluation.
- Perf bench (task 039 suite): a synthetic DSL with 10 mostly-static
  `${...}` expressions runs at within 10% of the same DSL rewritten
  by hand to pure literals. (Proves the pass is doing its job.)
- Book chapter update: [`book/src/dsl/expressions.md`](../../book/src/dsl/expressions.md)
  documents which expressions get hoisted, with the `Date` gotcha.

## Non-goals

- **Partial evaluation.** `${incoming.body.x + 1}` doesn't collapse
  to `${incoming.body.x + 1}` (constant folding on partially-runtime
  expressions). Cost/complexity tradeoff not worth it for the return.
- **Cross-step folding.** If `assign: { x: 5 }` then `assign: { y: "${x + 1}" }`,
  don't try to inline `x` into `y`'s expression. Different step,
  don't cross the boundary — some DSL patterns rely on the
  step-by-step semantics.

## Risk

- False positives ("this looks static but isn't"): mitigated by the
  runtime-binding allow-list being explicit and the fold-only-on-safe-globals
  rule.
- False negatives ("this looks runtime but is actually static"):
  cost is a missed optimization, not incorrect behaviour. Acceptable.
