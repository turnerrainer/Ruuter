# Scripting limits

CPU-bounded caps on the embedded JavaScript engine — protect the
tokio worker from a runaway `${…}` evaluation.

## What it is

Both Boa and QuickJS run **synchronously** on the tokio worker that
picked up the request. There's no cooperative-cancel API for either.
Instead, the engine imposes CPU-bounded caps: a loop-iteration ceiling
and a stack-depth ceiling per `${…}` / `$= … =$` evaluation.

## The config

```yaml
scripting:
  max_loop_iterations: 1000000
  max_stack_size:      400
```

## The defaults and why

- `max_loop_iterations: 1_000_000` — comfortably above any legitimate
  DSL template, low enough that even a bare `while(true){}` aborts in
  well under a second. Lower it (100 K is plenty for most templates)
  when you want a tighter safety margin.
- `max_stack_size: 400` — Boa's own default. Raise only if you have
  deeply-nested JS expressions and you know why. Setting it very high
  invites native-stack overflow before the JS-level check trips.

## What breaks if you set it wrong

- Setting `max_loop_iterations` too low → legitimate loops in
  templates abort mid-flight with `Script evaluation error: RuntimeLimit`.
  Symptom: previously-working DSLs suddenly 500 on certain inputs.
- Setting `max_stack_size` too high → a genuinely-runaway recursive
  expression can crash the whole process with a native SIGSEGV before
  the JS-level check trips. Keep close to the default.
- No wall-clock cap exists — a pathological but bounded-work script
  (999 999 iterations of expensive math) can still hog a worker for
  the duration. Prefer the [`iterate` step](../dsl/steps/iterate.md)
  for anything that looks like a loop.

## Cross-links

Full-behaviour reference: [Framework — Script runtime limits](../framework/script-limits.md).
Engine choice: [Scripting engines (Boa vs QuickJS)](../framework/scripting-engines.md).
