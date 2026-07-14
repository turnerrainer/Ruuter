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

## Verified example

```yaml
# GET /samples/conditionals/simple-switch?age=15 → teenager
# GET /samples/conditionals/simple-switch?age=30 → adult
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
