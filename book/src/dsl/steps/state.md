# state

Project-scoped in-process key-value store.

```yaml
read:   { state: { get: { key: "counter", into: n } }, next: bump }
bump:   { assign: { n2: "${(n ?? 0) + 1}" }, next: write }
write:  { state: { set: { key: "counter", value: "${n2}" } }, next: reply }
wipe:   { state: { delete: { key: "counter" } }, next: end }
```

## Semantics

- **Scope**: per-project. Two different projects never see each other's keys.
- **Type**: any JSON value. Numbers, strings, arrays, objects — stored as-is.
- **Missing key on `get`**: binds `null`.
- **Persistence**: in-process only. **Restart wipes everything.**
- **Concurrency**: `DashMap`-backed; concurrent writes to the same key are last-write-wins.

## Verified example

```yaml
# POST /samples/state/inc — increment counter, return new value
read_counter:
  state: { get: { key: "counter", into: current } }
  next: bump
bump:
  assign: { next_value: "${(current == null ? 0 : current) + 1}" }
  next: write_counter
write_counter:
  state: { set: { key: "counter", value: "${next_value}" } }
  next: respond
respond:
  return: { counter: "${next_value}" }
  next: end
```

## Multi-instance caveat

Two Ruuter replicas do **not** share state. For durable / cross-replica state, front with Resql (SQL → REST). See [What Ruuter does NOT do](../../reference/non-goals.md).
