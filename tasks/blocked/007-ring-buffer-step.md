# 007 — Generic rolling-window / ring-buffer step

**Status**: BLOCKED on #017 (externalize StateStore).

**Why blocked**: this step would deepen reliance on the in-process
`StateStore`, which #017 has identified as compromising Buerostack's
stateless-architecture principle. Wait until the externalization
design lands before adding more API surface that depends on the
in-process implementation.
**Severity**: MEDIUM (every signal-processing use case wants a fixed-
size sliding window; without it each DSL has to reimplement push/trim
in JS, paying the Boa cost per event).
**Effort**: 0.5 day.
**Filed**: 2026-06-17.
**Blocked by**: #003 (state).

## What's wrong

DSLs that compute moving averages, rate-limits, or any sliding-window
statistic have to either:
- evaluate a JS expression that loads an array from `state`, pushes,
  trims, writes back (expensive), or
- keep the buffer outside Ruuter entirely.

## Fix

Add a step type:

```yaml
- push_window:
    key: "bars.AAPL.close"
    value: "${incoming.body.close}"
    max_len: 50
    result: "window"          # binds the resulting Vec into the context
```

Implemented natively in Rust against the `StateStore` from #003. O(1)
per push using a `VecDeque`-backed value type kept alongside the
generic JSON values.

Companion read-only step (or just JS binding):

```yaml
- assign:
    sma: "${window.reduce((a,b)=>a+b,0)/window.length}"
```

## Verification

- Unit test: push 100 values with `max_len: 10`; expect the last 10.
- Concurrency: two events to the same key from parallel tasks produce
  a buffer whose final length is 10 (last-write semantics OK; no
  panics).

## Why this is generic

A bounded ring buffer is a primitive. It knows nothing about prices,
sensors, or any domain.
