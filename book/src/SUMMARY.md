# Summary

[Introduction](./introduction.md)
[Quickstart](./quickstart.md)

# DSL

- [File layout](./dsl/layout.md)
- [Expression language](./dsl/expressions.md)
- [Steps](./dsl/steps/index.md)
  - [assign](./dsl/steps/assign.md)
  - [return](./dsl/steps/return.md)
  - [switch](./dsl/steps/switch.md)
  - [log](./dsl/steps/log.md)
  - [http](./dsl/steps/http.md)
  - [state](./dsl/steps/state.md)
  - [iterate](./dsl/steps/iterate.md)
  - [template](./dsl/steps/template.md)
  - [ws_send](./dsl/steps/ws_send.md)
  - [declaration](./dsl/steps/declaration.md)
- [Context bindings](./dsl/context.md)
- [Constants](./dsl/constants.md)
- [Path parameters](./dsl/path-params.md)
- [Guards](./dsl/guards.md)
- [JavaScript gotchas](./dsl/js-gotchas.md)

# WebSocket

- [Server DSLs](./ws/server.md)
- [Sources & triggers](./ws/sources.md)

# Framework

- [Request pipeline](./framework/pipeline.md)
- [Response headers](./framework/response-headers.md)
- [Built-in endpoints](./framework/endpoints.md)
- [OpenAPI generation](./framework/openapi.md)
- [Idempotency-Key](./framework/idempotency.md)
- [CSRF](./framework/csrf.md)
- [CORS](./framework/cors.md)
- [SSRF allow-list](./framework/ssrf.md)
- [Method allow-list](./framework/methods.md)
- [Response size cap](./framework/size-cap.md)
- [Upstream status filter](./framework/status-filter.md)
- [Optimistic concurrency](./framework/optimistic-concurrency.md)
- [Traceparent & OpenTelemetry](./framework/tracing.md)
- [Script runtime limits](./framework/script-limits.md)
- [Default response headers](./framework/default-headers.md)

# Testing

- [Overview](./testing/overview.md)
- [dsl-lint](./testing/dsl-lint.md)
- [dsl-test](./testing/dsl-test.md)
- [Test file schema](./testing/schema.md)
- [Matchers](./testing/matchers.md)
- [Modes](./testing/modes.md)
- [Mocking upstream HTTP](./testing/mocking.md)
- [CI integration](./testing/ci.md)

# Operations

- [Configuration](./ops/configuration.md)
- [Environment variables](./ops/env.md)
- [Docker](./ops/docker.md)
- [Security hardening checklist](./ops/security-checklist.md)
- [Failure modes](./ops/failure-modes.md)
- [Troubleshooting](./ops/troubleshooting.md)

# Reference

- [Reserved subdirectories](./reference/reserved-subdirs.md)
- [Status codes emitted by the framework](./reference/status-codes.md)
- [What Ruuter does NOT do](./reference/non-goals.md)
- [Changelog](./reference/changelog.md)
