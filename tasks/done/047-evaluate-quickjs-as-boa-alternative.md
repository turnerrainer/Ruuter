# 047 — Evaluate/prototype QuickJS as a Boa alternative behind a Cargo feature

## Filed

2026-07-18 — originally filed as layer 5 of the Boa perf roadmap,
gated on 036 + 037 + 045 + 046. **Reprioritised 2026-07-19 after
v0.6.0 A/B bench data landed.**

**Updated priority: unblock 036 + 045 via QuickJS Send-compatibility.**

036 (per-project BoaContext pool) and 045 (pre-parsed Script cache)
are both blocked because `boa_engine::Context` and `Script` are
`!Send` (embedded `Rc` / `Gc`). If `rquickjs`'s Context/Function
types ARE `Send`, adopting QuickJS unblocks 036 + 045 without
needing a dedicated JS worker thread pool — which was the only
identified path to unblock them under Boa.

That reframes 047 from "engine swap for raw speed" (2-5×) into
"engine swap AND unblock the biggest 036+045 wins in one move"
(potentially 10-20× compound). Prototype-quality investigation
becomes higher-value than any single Boa follow-up.

**First-step deliverable**: a 200-line spike that answers
"Is `rquickjs::Context` Send?" — yes/no unlocks or forecloses
the whole 036+045 line of work.

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

Updated 2026-07-19 for the "unblock 036+045" reframing:

| Spike finding | Action |
|---|---|
| `rquickjs::Context: Send` — YES | Prototype swap. If bench shows parity+, immediately unblock 036 (per-project pool) via QuickJS. Ship as opt-in engine. |
| `rquickjs::Context: Send` — NO | Fall back to Boa's dedicated-OS-thread pool path (large refactor of 036). QuickJS still worth benching for raw perf, but the compound win is gone. |
| Boa 0.20+ makes Context Send | Reprioritise: stay on Boa, just upgrade version. |

## Risk

- QuickJS ECMAScript compatibility is high but not identical to
  Boa. A DSL that runs on Boa may behave subtly differently on
  QuickJS (edge cases in `Date` parsing, regex engines, `Number`
  precision at boundaries). Scenario-test coverage is the mitigation.
- Feature-flagged mutually-exclusive builds double the CI matrix.
  Manageable; not free.
