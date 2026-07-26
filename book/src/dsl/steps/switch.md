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

- Each `condition:` is a JS expression that must evaluate to `true` for the branch to be taken.
- Conditions are evaluated top-to-bottom; the first `true` wins.
- Falsy conditions include `false`, `0`, `""`, `null`, `undefined`.
- The trailing `next:` is the fallthrough. Omitting it and having no match falls through to the next step in source order.

## Runnable example

`DSL/samples/GET/conditionals/simple-switch.yml` (elided for the doc):

```yaml
check_age:
  assign: { age: "${parseFloat(incoming.params.age)}" }
  next: validate
validate:
  switch:
    - condition: "${age >= 18}"
      next: adult
    - condition: "${age >= 13}"
      next: teen
  next: child
adult: { return: { category: "adult" }, next: end }
teen:  { return: { category: "teenager" }, next: end }
child: { return: { category: "child" }, next: end }
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
