# Ruuter-on-Rust

Declarative HTTP + WebSocket router. You drop YAML files on disk;
Ruuter serves them as routes.

```yaml
# DSL/samples/GET/ping.yml
response:
  status: 202
  return: pong
```

Request:

```bash
curl -i http://localhost:8080/samples/ping
```

Response:

```http
HTTP/1.1 202 Accepted
xpingstatusheader: pong delivered

{"response":"pong"}
```

No compile step, no annotations, no code-gen. Restart the container
after editing a file and the route is live — or opt in to
[hot reload](./ops/hot-reload.md) for zero-restart edits during
development.

**Version:** 0.9.8-rc (pre-release; v1.0.0 is the next stable target) · **License:** Apache-2.0 · **Repository:** [turnerrainer/Ruuter](https://github.com/turnerrainer/Ruuter)

## What ships in the box

- File-based routing: `DSL/<project>/<METHOD>/<path>.yml` → `<METHOD> /<project>/<path>`
- 12 DSL step primitives (`assign`, `return`, `switch`, `log`, `http`, `state`, `iterate`, `template`, `ws_send`, `ws_tag`, `single_flight`, `declaration`)
- WebSocket server endpoints + upstream WebSocket source consumption
- OpenAPI 3.1 spec auto-generated from the DSL tree at `/_/openapi.json`
- Two shipped test binaries: `dsl-lint` (static) and `dsl-test` (runtime)
- A Postman collection covering every shipped sample
- Batteries: CSRF, SSRF allow-list, `X-Forwarded-For` trusted-proxy gating, W3C traceparent, response-size cap, request-method allow-list, in-process state store — all configurable, safe defaults

## Read in order

If it's your first time, follow these five short chapters — you'll have
a running server, three green test suites, and a working Postman
collection in about ten minutes.

1. [Prerequisites](./getting-started/prerequisites.md)
2. [Run it locally](./getting-started/run-locally.md)
3. [Watch the automated tests pass](./getting-started/automated-tests.md)
4. [Try the Postman collection](./getting-started/postman.md)
5. [What to read next](./getting-started/next-steps.md)

The rest of the book is reference material — DSL, framework, testing,
ops — organised by topic, meant to be dipped into once you know what
you're looking for.

## Audience

Third-party clients: DSL authors, integrators, operators. Not internal
contributors. If you're modifying Ruuter itself, read the source and
`HANDOFF.md`.
