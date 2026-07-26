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

## Runnable example

`DSL/samples/GET/variables/assign-simple.yml`:

```yaml
assign_vars:
  assign:
    name: "John Doe"
    age: 30
    city: "Tallinn"
  next: return_result

return_result:
  return:
    user:
      name: ${name}
      age: ${age}
      city: ${city}
  next: end
```

```console
$ curl -s http://localhost:8080/samples/variables/assign-simple
{"user":{"age":30,"city":"Tallinn","name":"John Doe"}}
```

## Runnable example — read query params

`DSL/samples/GET/variables/incoming-params.yml`:

```yaml
extract_params:
  assign:
    user_id: ${incoming.params.id}
    user_name: ${incoming.params.name}
  next: respond

respond:
  return:
    received:
      id: ${user_id}
      name: ${user_name}
    message: "Received parameters successfully"
  next: end
```

```console
$ curl -s 'http://localhost:8080/samples/variables/incoming-params?id=42&name=Ada'
{"message":"Received parameters successfully","received":{"id":"42","name":"Ada"}}
```
