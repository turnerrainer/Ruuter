# Ruuter-RS

Declarative REST + WebSocket router. YAML DSLs on disk become HTTP routes and WebSocket endpoints.

- **File-based routing.** `DSL/<project>/<METHOD>/<path>.yml` → `<METHOD> /<project>/<path>`.
- **YAML DSL.** Named steps with `${JS}` expressions between them. No compiling; DSLs are read at boot.
- **WebSocket.** Server endpoints and upstream sources first-class.
- **OpenAPI 3.1** auto-generated from every DSL at `/_/openapi.json`.
- **Batteries included.** Idempotency-Key, CSRF, SSRF allow-list, traceparent propagation, response-size cap, request-method allow-list, in-process state store — all configurable, all off-by-default unless dangerous to default-off.

Version: 0.4.0 · License: Apache-2.0.

## Audience for this book

Third-party clients: DSL authors, integrators, operators. Not internal-code contributors. If you're modifying Ruuter itself, read the source and `docs/todo.md`.
