# iterate

Loop `do:` over each element of `over:`.

```yaml
work:
  iterate:
    over: "${orders}"                 # expression → array
    as: order                         # per-iteration binding
    max_items: 100                    # cap; default 10_000
    do:
      - assign: { net: "${order.qty * order.price}" }
    collect: "${({ id: order.id, net: net })}"   # optional; per-iter value
    into: totals                                # collected array bound here
  next: reply
```

## Semantics

- `over:` must evaluate to a JS array; otherwise the step errors.
- `as:` names the iteration variable; visible only inside `do:` and the `collect:` expression.
- Steps inside `do:` execute in source order.
- `return` inside `do:` short-circuits the outer DSL.
- `collect:` is optional. When set, each iteration's value is pushed onto a fresh array; `into:` binds that array into the parent context.
- `max_items:` protects against runaway; step errors if exceeded.

## Object-literal gotcha

`{...}` at the start of a JS expression is parsed as a block. Wrap in parens:

```yaml
collect: "${({ id: order.id, net: net })}"     # right
collect: "${{ id: order.id, net: net }}"       # WRONG — SyntaxError
```

## Runnable example

`DSL/samples/POST/advanced/iterate-batch.yml`:

```yaml
setup:
  assign:
    orders: "${incoming.body.orders || []}"
  next: work

work:
  iterate:
    over: "${orders}"
    as: order
    max_items: 100
    do:
      - assign:
          net: "${order.qty * order.price}"
    collect: "${({ id: order.id, net: net })}"
    into: totals
  next: reply

reply:
  return: { count: "${totals.length}", totals: "${totals}" }
  status: 200
  next: end
```

Request — with two orders:

```bash
curl -sX POST http://localhost:8080/samples/advanced/iterate-batch \
     -H 'Content-Type: application/json' \
     -d '{"orders":[{"id":"A","qty":3,"price":10.0},{"id":"B","qty":2,"price":25.5}]}'
```

Response:

```json
{"count":2,"totals":[{"id":"A","net":30},{"id":"B","net":51}]}
```

Request — empty body (still returns 200 with an empty result):

```bash
curl -sX POST http://localhost:8080/samples/advanced/iterate-batch \
     -H 'Content-Type: application/json' -d '{}'
```

Response:

```json
{"count":0,"totals":[]}
```
