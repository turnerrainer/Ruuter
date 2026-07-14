# assign

Bind names in the local context.

```yaml
compute:
  assign:
    total:   "${items.reduce((a,b)=>a+b.price,0)}"
    now_iso: "${new Date().toISOString()}"
    literal: "just a string"
  next: reply
```

- Each key becomes a variable readable from later `${...}` expressions.
- **Evaluation order within a single `assign` block is undefined.** If `y` depends on `x`, put them in two separate `assign` steps.
- Variables scope to the current DSL run only; they do not persist across requests (use the [`state` step](./state.md) for that).

```yaml
# WRONG — y might evaluate before x
compute:
  assign:
    x: "${incoming.body.a + 1}"
    y: "${x * 2}"                # may see undefined x

# RIGHT
compute_x:
  assign: { x: "${incoming.body.a + 1}" }
  next: compute_y
compute_y:
  assign: { y: "${x * 2}" }
  next: reply
```
