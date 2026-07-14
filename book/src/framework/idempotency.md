# Idempotency-Key

Client-supplied dedup key. Prevents a retried write from executing twice.

## Client contract

```
POST /svc/orders
Content-Type: application/json
Idempotency-Key: 7b1e9c5f-3f8f-4c8f-9f8f-4c8f9f8f4c8f

{"amount": 100}
```

- **First call**: DSL runs, response is cached, headers include `Idempotency-Key: <same>`.
- **Subsequent calls with the same key**: cached response replayed. Headers include `Idempotency-Replayed: true`. DSL does not re-run.
- **Different key**: treated as a new request.

## Cache scope

`dedup_key = sha256(idempotency_key || method || project || path)`.

Two requests with the same `Idempotency-Key` but different method or path do NOT collide.

## Storage

In-process `DashMap` with configurable TTL. **Not shared across replicas** in 0.4.0 (see [What Ruuter does NOT do](../reference/non-goals.md)).

## Configuration

```yaml
idempotency:
  enabled: true                          # default true
  ttl_seconds: 86400                     # default 24 h
  methods: [POST, PUT, PATCH, DELETE]    # methods eligible; GET is never cached
```

## Verified round-trip

```
$ K=$(uuidgen)
$ curl -sSD - -X POST http://localhost:8080/samples/idempotent-transfer \
    -H "Content-Type: application/json" -H "Idempotency-Key: $K" \
    -d '{"amount":1}' -o /dev/null | grep -i idempotency
idempotency-key: <uuid>

$ curl -sSD - -X POST http://localhost:8080/samples/idempotent-transfer \
    -H "Content-Type: application/json" -H "Idempotency-Key: $K" \
    -d '{"amount":1}' -o /dev/null | grep -i idempotency
idempotency-replayed: true
idempotency-key: <uuid>
```

Response body is byte-identical between the two calls.
