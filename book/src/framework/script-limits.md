# Script runtime limits

Boa engine is CPU-bounded per `${...}` evaluation to protect the tokio worker from a runaway script.

## Configuration

```yaml
scripting:
  max_loop_iterations: 1000000       # default
  max_stack_size:      400           # default
```

## Behaviour

- Loop hits the iteration cap → evaluation aborts with `Script evaluation error: RuntimeLimit`.
- Recursion depth exceeds stack size → same error.
- No wall-clock cap. A pathological but bounded-work script (e.g. 999 999 iterations of expensive math) can hog a worker for the duration.

## Choosing values

- `max_loop_iterations`: at 1 M the ceiling is well above any legitimate DSL. Lower for tighter safety (100 K is fine for most templates).
- `max_stack_size`: 400 is Boa's own default. Raise only if you have deeply-nested JS expressions and you know why.

## Alternative: keep JS small

If a `${...}` needs a loop, it's usually a signal to use the [`iterate` step](../dsl/steps/iterate.md) instead — that gets its own step-level budget (`max_items`, default 10 000).
