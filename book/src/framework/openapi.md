# OpenAPI generation

`GET /_/openapi.json` — OpenAPI 3.1 spec generated from the DSL tree at boot. Regenerated only on process restart.

## Coverage

Every HTTP DSL file becomes one operation entry:

- Path: `/<project>/<endpoint-path>`
- Method: lower-cased method-directory name
- `operationId`: `<method>_<project>_<slug>`
- `summary`: DSL filename (stem)
- `tags`: `[<project>]`
- `responses`: inferred from `return.status` literals across the DSL's steps + framework baselines (400, 500)
- `description`, `parameters`, `requestBody`: derived from a top-level `declaration:` step when present

Excluded from the spec:

- `WS/` directories (not HTTP)
- `triggers/`, `sources/`, `cronmanager-jobs/` (reserved subdirs)

## Extending via `declaration`

Add a `declaration:` step to any route DSL to enrich its OpenAPI entry:

```yaml
declaration:
  description: "Cancel an order and archive its audit trail."
  allowed_body:   [order_id, reason]
  allowed_header: [Authorization]
  allowed_params: [correlation_id]

cancel:
  # ... steps ...
```

Effect on the emitted operation:

- `description` overrides the auto-generated one.
- `allowed_body` becomes an `object` `requestBody` schema (for POST/PUT/PATCH).
- `allowed_params` becomes query parameters.
- `allowed_header` becomes header parameters.

## Validation

The generated spec passes `redocly lint` cleanly (0 errors, 0 warnings on the sample corpus).

## Consumption

Fetch the spec:

```bash
curl http://localhost:8080/_/openapi.json > openapi.json
```

Validate it (swagger-cli works equivalently):

```bash
redocly lint openapi.json
```

Point Swagger UI, Redoc, Stoplight, or any OpenAPI-consuming tool at
`/_/openapi.json`.
