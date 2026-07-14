# 027 — Template step: full recursive DSL invocation

## Why

`src/steps/template.rs::execute` currently logs a warning and
advances. Any DSL that uses `template:` runs to completion but does
nothing observable. README already flags this as `⚠️ basic`, but the
step type is documented in the samples corpus (`DSL/samples/GET/templates/`)
and callers will expect it to work.

## Java reference

Template step in the Java Ruuter loads another DSL file by name
(project-relative), calls it with a locally-scoped body/query/headers
override, and binds the result into the calling context under a
declared key.

## Acceptance

- `TemplateStep::template` names another DSL by its relative path
  under the current project (e.g. `"templates/user-profile"`).
- Executor loads the target DSL (from the pre-loaded HTTP tree — no
  filesystem re-read at runtime), builds a fresh `ExecutionContext`
  with the caller-provided `body`/`query`/`headers` overrides, runs
  through `StepEngine::run`, and binds the response value into the
  caller's context under `result`.
- Recursion budget: template calls count against the same
  `max_step_recursions` cap as any other step transitions to prevent
  A-calls-B-calls-A infinite loops.
- Integration test: `DSL/samples/GET/templates/call-template.yml`
  ends up returning the profile fetched via `user-profile.yml`.
