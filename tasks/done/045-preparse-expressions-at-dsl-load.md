# 045 — Pre-parse `${...}` expressions at DSL load time

## Filed

2026-07-18 — layer 3 of the Boa perf roadmap (layers 1-2 are tasks 036 + 037).

## Status

**Deferred (2026-07-19)** — v1 implementation attempted after task 036 landed on the QuickJS backend, then reverted. Findings below.

### v1 attempt

With QuickJS's per-request session pool (task 036) in place, the natural v1 was "cache compiled expressions PER session — first eval of a given expression source registers it as `globalThis.__fn_<id> = function(){...}`, subsequent evals in the same session invoke `__fn_<id>()`."

Correctness: 142/142 Rust tests + 99/99 DSL scenarios still passed on both engines.

Perf A/B (3-run median, laptop, `scripting-quickjs` build):

| Scenario | QuickJS+036 | QuickJS+036+045 v1 | Δ |
|---|---:|---:|---|
| guarded | 6,118 rps | 6,773 rps | +11% |
| js-heavy | 7,906 rps | 8,293 rps | +5% |
| path-params | 8,486 rps | 7,219 rps | **-15% (regression)** |
| thin-dsl | 80,027 rps | 81,522 rps | +2% |

The wins came from expression repetition (guarded's guard evaluates `${incoming.headers.authorization}` from multiple conditions). The regression on path-params is because per-request session almost never sees the same expression twice — each `${...}` is unique per request → cache misses only → net loss from Mutex + double-eval on cache miss.

Reverted. Filed here for the redesign.

### Why per-session caching doesn't work well

The original task 045 assumed cross-request caching. Task 036 shipped as a per-request session pool, not a cross-request pool (a cross-request pool would require dedicated OS worker threads even on QuickJS — the pool wants its Runtime to outlive individual requests, which is orthogonal to the Send-compat 051 delivered).

Per-request caching only wins when the same expression fires multiple times in a single request. On the current DSL corpus, that's rare (mostly happens inside `iterate` bodies).

### Viable redesigns

1. **Compile-at-DSL-load, cache-by-source, per-session invoke.** At DSL load time, extract every `${...}` and `$=...=` and record their source strings on the DSL step data. At session init, bulk-compile all cached expressions in one `eval` (define all `globalThis.__fn_N`). This front-loads the compile cost to session init (once per request) but eliminates ALL per-expression compile costs at evaluate time. Net: cheaper if request has ≥2 evals (which most do). Correctness gate is a full DSL-tests corpus rerun.

2. **Cross-request cache with dedicated JS worker threads.** Bigger refactor: spawn N=cpus worker threads, each owning a persistent Runtime + expr cache; DSL evaluate() sends work over a channel and awaits response. This is what task 036 was originally designed for on Boa; task 051 made it optional by unlocking per-request pooling. But cross-request pooling on top of 051 IS possible and would let 045 use `Persistent<Function>` cached at DSL load. Bigger scope; file as a follow-up when the perf ceiling is actually hit.

3. **Only cache when eval count crosses a threshold.** Track eval counts per source; register a cached function on the 2nd eval. Adds a small per-eval hashmap-touch overhead but no double-eval on the miss path. Might pay off; would need bench.

For now, task 036 + task 051 delivers the biggest measured win (~2-4× on Boa-hitting DSLs). 045 stays deferred until (a) an iterate-heavy corpus makes redesign #1 worthwhile, or (b) the perf story needs the compound win from redesign #2.

## Original design (superseded)

**Boa-era blocker** — Boa 0.19's API forecloses both shipping shapes:

1. `Script::parse(src, realm, context)` requires a `&mut Context`. It
   cannot be called at DSL load time in isolation — needs a live Boa.
2. The resulting `Script` holds a `Realm` inside a `Gc<Inner>`.
   `boa_gc::Gc<T>` is `!Send + !Sync` (single-threaded GC), so parsed
   Scripts cannot be stored in a shared cache and cannot cross an
   `.await` boundary.
3. Even a per-context cache (build a Script on first eval, cache in
   that Context, reuse on subsequent evals hitting the same Context)
   requires holding Contexts across requests — which is exactly what
   task 036 blocks on.

Options mirror 036: dedicated OS worker threads that own both the
BoaContext AND a `HashMap<expression, Script>` cache, or upgrading
Boa to a version where Script is Send. Either path pulls significant
scope beyond a "pre-parse at load" change.

Interim mitigation: task 037's literal fast-path already removes
Boa entirely for expression-free values. For values that DO have
expressions, the parse cost stays on the hot path until 036 unblocks.

## Problem

`ScriptEngine::evaluate()` today calls `Source::from_bytes(script)` then
`boa.eval(source)` on every request. The parse step runs each time,
even for expressions that are structurally identical across millions
of requests. Estimated cost: **~200-500 µs per evaluation**, i.e.
15-25% of the per-call Boa overhead.

## Fix

At DSL load time (`DslParser::parse_file`), walk every step's values
and extract every `${...}` and `$=...=$` expression body. Parse each
once into either:

- A Boa `Script` if Boa's API allows sharing parsed scripts across
  contexts (needs verification against `boa_engine 0.19` — the
  `Script` type may be `Send + Sync` in 0.20+ but was not in 0.19).
- Cached source bytes + a per-context parse cache
  (`HashMap<expression_string, ParsedScript>`) stored alongside the
  pooled `BoaContext` (task 036). Every context in the pool builds
  its parse cache on first use; subsequent eval skips the parse step.

At runtime, `ScriptEngine::evaluate()` looks up the pre-computed
identifier for each expression, retrieves the parsed AST from the
context's cache (or from a shared read-only structure), and calls
eval directly.

## Interaction with task 036

Composes cleanly: 036 pools contexts; 045 pools parsed ASTs. Each
context in the pool gains a parse cache on first use, then hits the
cache on every subsequent request. Cache size is bounded by the
number of distinct `${...}` expressions in the loaded DSL tree —
finite and small (typically < 500 for a large corpus).

## Interaction with task 037

037 handles values that contain **no** `${...}` (pure literals) —
skips Boa entirely. 045 handles values that **do** contain `${...}`
— skips only the parse step. Both compose; neither replaces the other.

## Acceptance

- DSL load walks steps, extracts expression bodies, assigns a stable
  identifier per unique expression string.
- Expression identifiers stored inline on the parsed step
  representation (not re-scanned at runtime).
- `ScriptEngine::evaluate()` uses the pre-computed identifier to skip
  the parse step.
- Perf bench (task 039 suite): `/samples/variables/complex-object`
  (JS-heavy DSL) shows ≥25% throughput improvement over the
  036+037 baseline.
- Correctness: every existing integration test still passes
  byte-identical outputs.

## Non-goals

- Sharing parsed ASTs across projects. Different projects have
  different constants substituted in; two projects with the same
  `${incoming.body.x}` may have different surrounding context.
  Per-project cache scope only.
- Evaluating expressions at load time (that's task 046).

## Risk

- If Boa 0.19's `Script` type turns out to hold context-specific
  state (e.g. interned identifiers keyed to the parsing context),
  the per-context parse cache is the only viable shape. Verify
  before committing to the shared-Script approach.
- Adds a small memory overhead per pooled context (~KB per cache
  entry). Bounded by DSL corpus size, not request rate.
