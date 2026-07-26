# Run it locally

Five commands. Two minutes. Proof it works before you learn anything else.

## 1. Clone

```bash
git clone -b dev https://github.com/turnerrainer/Ruuter.git ruuter-on-rust
cd ruuter-on-rust
```

## 2. Start

```bash
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

```bash
docker compose down
```

## What just happened

- `DSL/samples/GET/ping.yml` was auto-mounted as `GET /samples/ping`. The response body, status, and headers all came from that YAML file.
- The same is true for every file under `DSL/<project>/<METHOD>/<path>.yml`.
- Editing a DSL requires `docker compose restart ruuter-on-rust` (sub-second reload; hot-reload is not shipped in 0.7.0).

Next: [Watch the automated tests pass](./automated-tests.md).
