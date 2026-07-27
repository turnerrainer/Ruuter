# Run it locally

Two minutes. Proof it works before you learn anything else. Pick one
of the two paths below.

## Path A — pull the pre-built image (fastest)

If you just want to try Ruuter, don't clone anything. Pull the
official multi-arch image (linux/amd64 + linux/arm64) from Docker Hub
or GHCR:

```bash
docker run -d --name ruuter -p 8080:8080 \
    turnerrainer/ruuter-on-rust:latest
```

The published image bakes in `DSL/samples/` so every endpoint under
`/samples/*` works out of the box. Skip to
[step 3](#3-health-check) to verify.

**Bring your own DSL** — mount your tree over the sample one:

```bash
docker run -d --name ruuter -p 8080:8080 \
    -v $(pwd)/DSL:/app/DSL:ro \
    -v $(pwd)/constants.ini:/app/constants.ini:ro \
    turnerrainer/ruuter-on-rust:latest
```

**Verify the image** (optional, supply-chain hygiene). Images are
signed keyless via cosign; verify against the exact publisher
workflow:

```bash
cosign verify turnerrainer/ruuter-on-rust:latest \
    --certificate-identity-regexp \
      "^https://github.com/turnerrainer/Ruuter/\.github/workflows/publish\.yml@refs/tags/v.*$" \
    --certificate-oidc-issuer "https://token.actions.githubusercontent.com"
```

## Path B — build from source

For hacking on Ruuter itself, or when you want to modify the shipped
Dockerfile:

```bash
git clone -b dev https://github.com/turnerrainer/Ruuter.git ruuter-on-rust
cd ruuter-on-rust
docker compose up -d --build
```

First build is 60–90 s. Subsequent starts are seconds.

## 3. Health check

```bash
curl http://localhost:8080/health
# {"status":"ok"}
```

If you see that JSON, the framework is up and ready to serve DSL routes.

## 4. Hit a shipped sample

The repo ships 60 sample DSLs under `DSL/samples/`. Try three:

```bash
curl -i http://localhost:8080/samples/ping
# HTTP/1.1 202 Accepted
# xpingstatusheader: pong delivered
# "pong"

curl 'http://localhost:8080/samples/variables/incoming-params?id=42&name=Ada'
# {"received":{"id":"42","name":"Ada"},"message":"Received parameters successfully"}

curl -s http://localhost:8080/_/openapi.json | head -c 120
# {"openapi":"3.1.0","info":{"title":"Ruuter-on-Rust DSL API", ...
```

The OpenAPI document is regenerated at every boot from the DSL tree on disk — no annotations, no code-gen step.

## 5. Stop when you're done

Path A:

```bash
docker rm -f ruuter
```

Path B:

```bash
docker compose down
```

## What just happened

- `DSL/samples/GET/ping.yml` was auto-mounted as `GET /samples/ping`. The response body, status, and headers all came from that YAML file.
- The same is true for every file under `DSL/<project>/<METHOD>/<path>.yml`.
- Editing a DSL: by default, `docker compose restart ruuter-on-rust` (sub-second reload). For a dev-only zero-restart workflow, see [Hot reload](../ops/hot-reload.md).

Next: [Watch the automated tests pass](./automated-tests.md).
