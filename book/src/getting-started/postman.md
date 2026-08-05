# Try the Postman collection

The repo ships a Postman collection covering every DSL under
`DSL/samples/`. Import it, point it at your local server, and hit
every sample endpoint from the GUI.

**Prerequisite:** the server is running from
[Run it locally](./run-locally.md). Check with
`curl http://localhost:8080/health`.

## Files

Under `postman/` at the repo root:

| File | Purpose |
|---|---|
| `ruuter.postman_collection.json` | One request per sample DSL, grouped by project |
| `ruuter.postman_environment.json` | Sets `{{baseUrl}}` to `http://localhost:8080` |
| `README.md` | Regeneration recipe |

## GUI — Postman desktop app

1. **File → Import** → drop both JSON files.
2. Top-right environment selector → pick **"Ruuter-on-Rust (local)"**.
3. Open the `samples` folder → click any request → **Send**. First few to try:
   - `GET samples/ping` — 202 with `{"response":"pong"}`
   - `GET samples/variables/incoming-params?id=42&name=Ada` — echoes the params
   - `GET samples/basic/status-codes` — custom status
4. Click the collection root → **Run collection** → **Run Ruuter-on-Rust DSL API**.

You'll see every request execute with its status and duration. Failures
(if any) mean either the server isn't running or a DSL was changed
without regenerating the collection — see the regen recipe below.

## CLI — Newman

Same collection, headless:

```bash
newman run postman/ruuter.postman_collection.json \
       -e postman/ruuter.postman_environment.json
```

Exit code is non-zero on any failed request.

## Regenerating the collection

The collection is not hand-written — it's generated from
`GET /_/openapi.json` (the OpenAPI 3.1 document Ruuter emits at boot
from the DSL tree on disk).

```bash
curl -s http://localhost:8080/_/openapi.json > postman/openapi.json
npx openapi-to-postmanv2 -s postman/openapi.json \
    -o postman/ruuter.postman_collection.json -p
```

Regenerate whenever you add or rename a DSL.

## Where to look next

Every request in the collection maps 1:1 to a file:

| Request | DSL file | Test file |
|---|---|---|
| `GET samples/ping` | `DSL/samples/GET/ping.yml` | `DSL-tests/samples/GET/ping.test.yml` |
| `GET samples/variables/incoming-params` | `DSL/samples/GET/variables/incoming-params.yml` | `DSL-tests/samples/GET/variables/incoming-params.test.yml` |

Pick a request that's interesting, open the matching DSL to see how it
works, then read the [step reference](../dsl/steps/index.md) for the
primitives it uses.

Next: [What to read next](./next-steps.md).
