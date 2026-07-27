# Postman assets

Functional-test collection covering every DSL under `DSL/samples/`.

| File | Purpose |
|---|---|
| `ruuter.postman_collection.json` | One request per sample DSL, grouped by project |
| `ruuter.postman_environment.json` | Sets `{{baseUrl}}` to `http://localhost:8080` |

Full walkthrough (import, run, expected output) is in the book at
[`book/src/getting-started/postman.md`](../book/src/getting-started/postman.md).

## Quick use

**Postman desktop:** File → Import → drop both files, select the
"Ruuter-on-Rust (local)" environment, run the collection.

**Newman (CLI):**

```bash
newman run postman/ruuter.postman_collection.json \
       -e postman/ruuter.postman_environment.json
```

Both require the server to be running (`docker compose up -d --build`
from the repo root).

## Regenerating from the DSL tree

The collection is generated from Ruuter's own OpenAPI 3.1 document
(`GET /_/openapi.json`). Regenerate whenever you add or rename a DSL:

```bash
curl -s http://localhost:8080/_/openapi.json > postman/openapi.json
npx openapi-to-postmanv2 \
    -s postman/openapi.json \
    -o postman/ruuter.postman_collection.json -p
```

`openapi.json` is a working file, not committed. The Postman collection
and environment JSONs are the artefacts you check in.
