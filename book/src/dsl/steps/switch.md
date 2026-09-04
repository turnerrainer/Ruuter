# switch

Conditional branching. First matching condition wins.

```yaml
route:
  switch:
    - condition: "${incoming.body.action === 'buy'}"
      next: buy_step
    - condition: "${incoming.body.action === 'sell'}"
      next: sell_step
  next: default_step        # runs if no condition matched
```

- Each `condition:` is a JS expression that must evaluate to a **truthy** value for the branch to be taken. JS `ToBoolean` semantics apply: `${a && b}` fires when both operands are truthy, even when the result is not literally boolean `true`. Wrapping in `!!(...)` is never needed.
- Conditions are evaluated top-to-bottom; the first truthy result wins.
- **Falsy** values: `false`, `0`, `NaN`, `""`, `null`, `undefined`. Everything else — including any non-empty string, non-zero number, `[]`, and `{}` — is truthy.
- The trailing `next:` is the fallthrough. Omitting it and having no match falls through to the next step in source order.
- Diverges from Java Ruuter, which requires strict boolean `true`. The reason: Ruuter's expression language is JavaScript, so `${a && b}` naturally returns `b` (not `true`) when `a` is truthy — the JS-side semantics and the switch-side semantics should agree.

## Runnable example

`DSL/samples/GET/conditionals/simple-switch.yml` (elided for the doc):

```yaml
check_age:
  assign:
    age: "${parseFloat(incoming.params.age)}"
  next: validate

validate:
  switch:
    - condition: "${age >= 18}"
      next: adult
    - condition: "${age >= 13}"
      next: teen
  next: child

adult:
  return:
    category: "adult"
  next: end

teen:
  return:
    category: "teenager"
  next: end

child:
  return:
    category: "child"
  next: end
```

Request — teenage branch:

```bash
curl -s 'http://localhost:8080/samples/conditionals/simple-switch?age=15'
```

Response:

```json
{"age":15,"category":"teenager","message":"You are a teenager"}
```

Request — adult branch:

```bash
curl -s 'http://localhost:8080/samples/conditionals/simple-switch?age=25'
```

Response:

```json
{"age":25,"category":"adult","message":"You are an adult"}
```

(The `message` field comes from a follow-up `assign` step in the real
DSL; the switch itself produces the branch, not the message.)
