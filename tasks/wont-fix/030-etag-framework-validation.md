# 030 — Framework-side ETag/If-Match value validation

## Why

The 0.4.0 framework enforces `If-Match` header presence and returns
428 if it's missing, but the header's VALUE is not compared against
any stored state — that's the DSL's job (via a Resql query with an
`AND latest_state_id = :expected_id` clause).

## Status

**Excluded by owner design decision**: Ruuter is a stand-alone
component with no requirement to know about Resql. Cross-component
coupling would break the "each component is independently usable"
invariant. Keeping the DSL as the enforcement point is intentional.

This ticket is filed for traceability so future contributors don't
propose the same coupling as a "missing feature."
