# 028 — Ship a TIM/JWT verification guard (or explicitly rule it out)

## Why

PATTERNS.md §1 declares TIM-issued HttpOnly cookies as the auth
transport for every Buerostack app. The framework offers no built-in
verifier, so every DSL that needs auth writes its own guard by hand
— cookie parse, JWT signature check, expiry, claim projection. Bug
surface is per-project.

## Status

**Excluded by owner design decision**: Ruuter is a dumb pipe. IAM
validation is out of scope. This ticket is filed for traceability;
the answer is "don't build it here; consumers verify via TIM sidecar
or a service-mesh policy layer."

## Acceptance (if ever unexcluded)

- Ship a `guards/tim-cookie.guard.yml.example` sample using the
  DSL as-is (no framework code).
- README calls out the TIM sidecar / mesh-policy pattern explicitly
  so partners don't expect Ruuter to do it.
