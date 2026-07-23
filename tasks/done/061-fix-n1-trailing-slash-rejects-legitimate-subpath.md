# 061 — SSRF allowlist trailing-slash entry rejects legitimate subpath (F1)

## Filed

2026-07-20 — surfaced by h2ck.me second-round "break-the-fix"
sweep (finding F1, `POST-FIX-REVIEW-2.md`). Regression introduced
by task 057.

## Severity

**High** (functional regression). The bug hits every operator who
writes an allowlist entry with a trailing `/` — which is the exact
idiom the review recommends for boundary safety. First deploy after
upgrade breaks every outbound DSL that relies on a path-scoped
allowlist entry.

## Reproduction

```rust
// tests/security_new_probes_2.rs::break_n1_trailing_slash_entry_rejects_legitimate_subpath
allowed_urls: ["http://127.0.0.1:<port>/v1/"]
// DSL calls: http://127.0.0.1:<port>/v1/legit
// Expected: request goes out (path is under /v1/)
// Actual:   `url not in internal_requests.allowed_urls`
```

Test file: `tests/security_new_probes_2.rs`. Currently FAILS —
the failure IS the finding.

## Root cause

`src/http_client/mod.rs:686-720` — the trailing-slash branch of
`allow_entry_matches`. The boundary check:

```rust
if !req_full.starts_with(&entry_full) {
    return false;
}
let tail = &req_full[entry_full.len()..];
tail.is_empty()
    || tail.starts_with('/')
    || tail.starts_with('?')
    || tail.starts_with('#')
```

`entry_full` INCLUDES the trailing `/` the operator wrote. For
entry `http://host/v1/` and request `http://host/v1/legit`:

- `entry_full` = `http://host:80/v1/`
- `req_full`   = `http://host:80/v1/legit`
- `tail`       = `"legit"` — none of `/`, `?`, `#`; not empty → false.

The fix's comment says "A path already ends-with `/` on the entry
naturally accepts any deeper path since the next char is that
trailing `/` itself." That premise is inverted: the `/` is
consumed by `starts_with`; `tail` begins with the character AFTER
the `/`.

## Fix

Short-circuit the tail check when the entry's own last character
is a URL delimiter — the boundary is already closed by the entry
itself.

```rust
// src/http_client/mod.rs — inside allow_entry_matches, after
// the `starts_with` check.
let entry_closed_at_boundary = matches!(
    entry_full.chars().last(),
    Some('/') | Some('?') | Some('#')
);
if entry_closed_at_boundary {
    return true;
}
let tail = &req_full[entry_full.len()..];
tail.is_empty()
    || tail.starts_with('/')
    || tail.starts_with('?')
    || tail.starts_with('#')
```

## Acceptance

- `tests/security_new_probes_2.rs::break_n1_trailing_slash_entry_rejects_legitimate_subpath`
  must flip from FAILED to PASSED (victim listener IS reached).
- `tests/security_new_probes.rs::s2_residual_path_scoped_entry_starts_with_bypasses_boundary`
  must remain PASSED (bare `/v1` still rejects `/v1anything`).
- `tests/security_hardening.rs::ssrf_prefix_check_is_substring_and_leaks_to_lookalike_domain`
  must remain PASSED (bare-origin exact match unaffected).
- Add positive test: entry `http://host/v1/`, request
  `http://host/v1/` (exact match with trailing slash) — allowed.
- Add positive test: entry `http://host/v1?tok=X`, request
  `http://host/v1?tok=X` and `http://host/v1?tok=X&extra=1` —
  both allowed.

## Cross-reference

- `projects/Ruuter-on-Rust/POST-FIX-REVIEW-2.md § F1`
- Task 057 (the fix this regresses).

Effort estimate: 10 min including tests.
