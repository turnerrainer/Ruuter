# 048 — Lightweight expression subset for common `${...}` patterns

## Filed

2026-07-18 — layer 6 of the Boa perf roadmap. **Gated on tasks
036 + 037 + 045 + 046 completing AND telemetry showing >60% of
production `${...}` uses fall into the "simple path/comparison"
class.**

## Problem

Even with a fast JS engine, invoking a general-purpose scripting
runtime for every `${incoming.body.field}` is heavier than the
operation warrants. In typical Ruuter DSLs, most `${...}` uses
fall into a small number of patterns:

- Path access: `incoming.body.field.subfield`, `incoming.headers['x-user']`
- Simple equality: `${x === 'value'}`
- Presence check: `${!!incoming.headers['authorization']}`
- Nullish coalescing default: `${x ?? 'default'}`
- Simple arithmetic: `${prev + 1}`
- Ternary: `${x > 0 ? 'positive' : 'negative'}`

None of these require a full JS engine. Interpreted directly in
Rust, each costs ~10-50 ns instead of ~200 µs through Boa.

## Fix

Introduce a **detection-and-dispatch** layer at DSL load:

1. When parsing a `${...}` expression, first attempt to parse it
   with a **restricted expression grammar** (path access, binary
   ops, ternary, `??`, `!`, literals).
2. If it parses cleanly → compile to a native Rust closure (or a
   small ADT that a hand-written evaluator interprets). Store
   alongside the step, marked "fast-path".
3. If it fails to parse → fall back to Boa. Store the pre-parsed
   Boa AST (task 045), marked "js-path".
4. At runtime, the executor checks the marker and dispatches.

DSL authors write the same `${...}` syntax; the framework picks
the engine invisibly.

## Grammar (proposed subset)

Roughly a subset of ES that covers ~80% of typical use:

```
expr    := ternary
ternary := binary ('?' expr ':' expr)?
binary  := unary (op unary)*
op      := '===' | '!==' | '==' | '!=' | '<' | '<=' | '>' | '>='
         | '&&' | '||' | '??' | '+' | '-' | '*' | '/' | '%'
unary   := ('!' | '-' | '+')? primary
primary := literal | path | '(' expr ')'
path    := ident ('.' ident | '[' expr ']')*
literal := number | string | 'true' | 'false' | 'null' | 'undefined'
```

Rejects (falls through to Boa): function calls, method calls,
array literals, object literals, arrow functions, template literals,
regex, `new`, `typeof`, everything else.

Object/array literal use (`${({ a: 1 })}`, `${[1,2,3]}`) is common
enough that it may be worth including. Decide during design based
on telemetry.

## Prerequisites

- Instrument production Ruuter to log the AST shape of every
  `${...}` evaluation for a week. Aggregate: what fraction fall
  into the subset above?
- If <40%: don't ship — the maintenance cost of a second engine
  exceeds the benefit.
- If 40-80%: proceed with the subset. The remaining fall through
  to Boa transparently.
- If >80%: strong ship signal. Optimizes the vast majority of
  DSL evaluations.

## Interaction with tasks 036/037/045/046/047

- 037 already skips values with no `${...}`. This task addresses
  values that HAVE `${...}` but of a simple shape.
- 045 pre-parses Boa expressions. This task adds a parallel
  "pre-analyze into fast-path or Boa" pipeline. Composes.
- 046 collapses fully-static expressions at load. This task
  handles the residual runtime-dependent-but-simple expressions.
- 047 (QuickJS) is orthogonal — if Boa is replaced by QuickJS,
  the fallback path uses QuickJS instead. Fast-path unchanged.

## Acceptance

- Grammar implemented as a hand-written or nom-based parser.
- Fast-path evaluator with unit tests covering every operator +
  path-access shape.
- DSL load-time analysis marks each `${...}` as fast-path or
  js-path.
- Perf bench: DSLs whose expressions all fall into the fast-path
  subset show throughput close to the framework-baseline (~60k rps
  on `/health`), not the DSL baseline (~3k rps today).
- Semantic-equivalence tests: for every fast-path expression, the
  output MUST match what Boa would have produced given the same
  input. Property tests are the right shape here.
- Documented in [`book/src/dsl/expressions.md`](../../book/src/dsl/expressions.md):
  which shapes are fast-pathed, which fall through to full JS.

## Non-goals

- Making the fast-path visible to DSL authors as a different
  delimiter. Same `${...}` syntax; engine choice is invisible.
- Replacing Boa/JS entirely. The fallback stays. Complex JS remains
  available; simple JS just gets faster.
- Precise JS semantic compatibility on edge cases (e.g. `==`
  coercion tables). If we can't exactly match JS's weird bits,
  we conservatively fall through to Boa. Better to be slower and
  identical than fast and slightly wrong.

## Risk

- **Semantic divergence.** JavaScript's `==` and `+` operators
  have surprising coercion rules. The fast-path interpreter must
  either match them exactly (hard) or refuse to fast-path any
  expression using them (easier — conservatively fall through).
- **Maintenance cost.** Two evaluators to keep in sync with future
  DSL feature additions. Contained by: keeping the fast-path
  grammar deliberately small and only adding to it when telemetry
  proves the addition covers meaningful traffic.
