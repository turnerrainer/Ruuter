# Scripting engines (Boa vs QuickJS)

Ruuter has two interchangeable JavaScript backends behind the same `${expr}` / `$=expr=` DSL syntax. The operator picks one at build time; DSL authors never see the difference.

## Picking one

```bash
# Default — Boa, pure Rust, no C deps
cargo build --release

# QuickJS via rquickjs
cargo build --release --no-default-features --features scripting-quickjs
```

Exactly one of `scripting-boa` (default) or `scripting-quickjs` must be enabled. The build errors clearly if both or neither are set.

## When to pick which

| | Boa | QuickJS |
|---|---|---|
| Language | Pure Rust | C library (via `rquickjs` wrapper) |
| Binary size | Baseline | +~500 KB |
| Send/Sync | `!Send` (blocks pooling) | `Send + Sync` (with `parallel` feature) |
| Per-eval perf | 1× (baseline) | 2-5× faster on typical workloads |
| Per-request session pool (task 036) | Blocked | **Enabled** |
| ECMAScript compat | High | High (same corpus in Ruuter's DSL-tests passes on both) |
| CVE surface | Rust safety net | C library, non-zero |

### Measured deltas (v0.6.6, 3-run median, laptop)

| Scenario | Boa | **QuickJS + 036 + 045** | Δ vs Boa |
|---|---:|---:|---|
| guarded (guard + auth check + main DSL) | 1,401 rps | **6,955 rps** | **+396%** (5×) |
| js-heavy (`Date.now()` + object literal) | 3,245 rps | **7,735 rps** | **+138%** (2.4×) |
| path-params (switch + condition eval) | 2,098 rps | **8,111 rps** | **+286%** (3.9×) |
| thin-dsl (037 fast-path — no engine call) | 77,777 rps | 80,398 rps | parity |
| framework-baseline (no DSL, no engine) | ~95k rps | ~95k rps | parity |

The full compound of "engine swap (051) + per-request session pool (036) + pre-parsed expression registry (045)" moves Boa-hitting DSLs from the 1-3k rps band into the 6-9k rps band. Framework baseline unchanged.

Rerun on an isolated host (see `bench/AWS-RUNBOOK.md`) if you need shipping-grade numbers; the localhost run has real noise but the direction is robust across runs.

**Default recommendation**: Boa. It's the default for a reason — pure Rust, no CVE surface, and DSL-hot-path perf is dominated by the framework, not the engine. Only reach for QuickJS if:

- Your DSL corpus is JS-heavy AND per-request Boa cost is your measured bottleneck (verify with `bench/run.sh` on `js-heavy`), or
- You want to enable task 036 / 045 (per-request context pool, pre-parsed script cache — both need a `Send` context).

## Compatibility guarantees

The full `DSL-tests/samples/**` corpus passes byte-identically on both engines as of v0.6.4. This is enforced as part of the CI matrix — a scenario diverging between engines fails the build.

Known behaviour deltas (documented for DSL authors who care):

- **NaN serialisation** — both engines refuse to serialise NaN into a variable assignment (fires the same `Invalid number` error), but the exact code path differs.
- **`Number.prototype.toString` precision** — irrational values may differ in the last decimal digit. Any DSL that pins on exact string form of a computed number is fragile in both engines; prefer number equality with a tolerance.
- **`Date` parsing edge cases** — non-ISO date strings can accept/reject differently between engines. Stick to ISO 8601.
- **Regex flavors** — complex Unicode classes may compile differently. QuickJS is more lax with some tokens.

If a DSL scenario in your project trips a divergence, either:
1. Rewrite the DSL to use the engine-agnostic intersection (usually possible), or
2. Pin the engine at build time and document the pin in the deployment README.

## Runtime limits

Both engines honor the same `ScriptLimits`:

```yaml
scripting:
  max_loop_iterations: 1000000
  max_stack_size: 400
```

- Boa maps these to its native `runtime_limits_mut()` API.
- QuickJS maps `max_stack_size` to a byte budget (128 KB × depth by convention). `max_loop_iterations` is planned to be enforced via QuickJS's interrupt handler in a follow-up; today it's advisory on the QuickJS backend.

## Metrics

`boa_context_created_count()` returns the count of native contexts created since process start — same helper for both backends. The name is historical (from when Boa was the only engine); on the QuickJS backend it counts QuickJS `Context::full()` calls.

## The 037 fast-path applies to both

Task 037's literal fast-path (skip the engine entirely for values with no `${...}`) runs BEFORE either backend's `evaluate()` fires. Result: expression-free values cost the same on both engines (both are `input.clone()`).

## Why this split exists (task 051)

Boa's `Context` embeds `Rc<...>` and is `!Send + !Sync`. That property forecloses per-request context pooling (task 036) and pre-parsed script caching (task 045) — both need to hold a context across `.await` boundaries in the async framework.

rquickjs (with the `parallel + futures` features) exposes `Send + Sync` types. Adopting it as an alternative backend was the compact path to unblock those tasks, verified by the task 047 spike (see `tests/spike_047_quickjs_send.rs`).

Boa stays the default because it's the most conservative choice; QuickJS is opt-in for operators who specifically want its perf profile or need the Send-compatibility for future work.
