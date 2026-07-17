# 047 — Evaluate/prototype QuickJS as a Boa alternative behind a Cargo feature

## Filed

2026-07-18 — layer 5 of the Boa perf roadmap. **Gated on tasks 036 +
037 + 045 + 046 completing first.** If those close the gap enough,
this task is deprioritized or dropped.

## Problem

Boa is pure Rust with no JIT. Its per-eval cost is ~2-5× QuickJS on
typical workloads (documented benchmarks; ~10-50× vs V8 with JIT).
For DSLs that do meaningful JS work inside `${...}` — string
manipulation, JSON reshaping, array reducers — the engine itself
becomes the bottleneck no amount of context-pooling can fix.

## Fix (evaluate, then decide)

Prototype a QuickJS backend behind a Cargo feature flag:

```toml
[features]
default = ["scripting-boa"]
scripting-boa    = ["boa_engine"]
scripting-quickjs = ["rquickjs"]

# Mutually exclusive; the trait impl is one or the other.
```

`ScriptEngine` becomes a thin trait; `BoaScriptEngine` and
`QuickJsScriptEngine` implement it. Runtime-limit configuration
(`max_loop_iterations`, `max_stack_size`) mapped to each engine's
equivalent primitives (QuickJS has interrupt handlers + stack
limits).

## What to measure before committing

Bench (extend task 039):

- Thin DSL (`/samples/basic/hello`): should be neutral — no JS in
  the hot path.
- JS-heavy DSL (`/samples/variables/complex-object`,
  `/samples/advanced/iterate-batch`): expected 2-5× throughput.
- Cold-start impact: QuickJS has different init cost than Boa
  (typically lower).
- Binary size: expected +500 KB with QuickJS via `rquickjs`.
- Memory profile under 5k rps sustained.
- CVE surface: `rquickjs` wraps C code; audit the wrapper's
  soundness and QuickJS's own CVE history.

## Gate before rollout

- Every DSL scenario test in `DSL-tests/` passes byte-identical
  outputs on both engines.
- No new dependency risk classes (QuickJS is a C library — same
  broad category as `libssl` or `libpq`; smaller than V8 by orders
  of magnitude but non-zero).
- Ops story documented: how to pick which engine at build time,
  what the fallback is if the chosen engine has a bug.

## Non-goals

- V8 via `rusty_v8`. Explicitly rejected: 30-50 MB binary bump,
  ~100 ms init cost, heavy C++ CVE surface. Wrong tradeoff for a
  compact router.
- SpiderMonkey, JavaScriptCore, Hermes. Similar concerns.
- Making QuickJS the default. Even if it wins on perf, Boa stays
  the default until QuickJS proves out across the sample corpus
  + a release cycle of real-world use. Cargo feature gate stays
  for a full major version at minimum.

## Decision matrix

| After 036+037+045+046 land, if the DSL-heavy bench... | Action |
|---|---|
| Meets throughput targets (>10k rps typical DSL) | Stop. Ship. Boa is fine. |
| Falls short by <2× | Layer 048 (lightweight expression subset for common cases) may be enough. Try that first. |
| Falls short by >2× | Prototype QuickJS. If bench confirms 2-5× improvement AND scenario tests pass, ship as opt-in. |

## Risk

- QuickJS ECMAScript compatibility is high but not identical to
  Boa. A DSL that runs on Boa may behave subtly differently on
  QuickJS (edge cases in `Date` parsing, regex engines, `Number`
  precision at boundaries). Scenario-test coverage is the mitigation.
- Feature-flagged mutually-exclusive builds double the CI matrix.
  Manageable; not free.
