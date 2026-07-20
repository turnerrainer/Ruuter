# 057 — Enforce path-segment boundary in SSRF allowlist entries (h2ck.me N1, residual S2)

## Filed

2026-07-20 — surfaced by h2ck.me post-fix follow-up sweep
(finding N1, `POST-FIX-REVIEW.md`). Pinned by
`tests/security_new_probes.rs::s2_residual_path_scoped_entry_starts_with_bypasses_boundary`.

## Severity

**Medium** — same bug class as S2 (which was closed for
bare-origin entries), just relocated to the path segment. An
operator who writes a bare path prefix without a trailing slash
gets substring-confusion escape between path segments.

## Problem

`src/http_client/mod.rs:596-614` — `allow_entry_matches` splits
into two cases based on `has_path_component(entry)`:

- **Bare-origin entry** (no path) — enforces exact origin equality.
  This is the S2 fix and it works.
- **Path-scoped entry** — reconstructs `entry_full` as
  `origin + entry_tail` and matches with `req_full.starts_with(&entry_full)`.

The `starts_with` in the path-scoped branch has the same substring
problem the origin branch avoids:

```rust
let req_full = format!("{}{}", req_origin, req_url_path_query(req_url));
let entry_tail = &entry[entry_scheme_authority_len(entry)..];
let entry_full = format!("{}{}", entry_origin, entry_tail);
req_full.starts_with(&entry_full)   // ← boundary bug
```

Operator writes `allowed_urls: ["http://api.example.com/v1"]`
intending "everything under /v1/". Attacker calls
`http://api.example.com/v1anything/steal` — `req_full` starts
with `http://api.example.com:80/v1`, `starts_with` returns true,
request is allowed.

## PoC

`tests/security_new_probes.rs::s2_residual_path_scoped_entry_starts_with_bypasses_boundary`
— whitelists `http://127.0.0.1:<port>/v1`, calls `/v1private/steal`,
confirms the victim listener receives the request.

## Fix

After the `starts_with` check succeeds, require the character at
`req_full[entry_full.len()]` to be one of: `/`, `?`, `#`, or
end-of-string. That preserves the "prefix match on paths" contract
without leaking across path segments.

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

Edge cases to keep in the fix:

- Entry `http://host/v1/` matches `http://host/v1/anything` — tail
  is `anything`, `starts_with('/')` false but the entry already
  ended in `/`, so the match includes the boundary. Reads fine
  with the check above.
- Entry `http://host/v1?token=X` matches
  `http://host/v1?token=Xanything`. The `?` boundary check
  handles this — after the entry ends at `X`, `tail` begins with
  `anything`, none of `/?#`, → reject. Correct.
- Entry `http://host/` (bare path root) — tail after `/` is
  whatever came after `/` in the request. If the request is
  exactly `http://host/`, tail is empty → allow. Correct.

## Acceptance

- `allow_entry_matches` enforces a path-segment boundary after the
  entry ends.
- Flip
  `tests/security_new_probes.rs::s2_residual_path_scoped_entry_starts_with_bypasses_boundary`
  — victim listener must NOT receive the request; the DSL should
  see `"url not in internal_requests.allowed_urls"` in the error.
- Add positive test: entry `http://127.0.0.1:<port>/v1`, request
  `http://127.0.0.1:<port>/v1/legit` — allowed.
- Add positive test: entry `http://127.0.0.1:<port>/v1?tok=X`,
  request `http://127.0.0.1:<port>/v1?tok=X&extra=1` — allowed
  (query params extending the entry are fine as long as the entry
  matched up to `X`).

## Cross-reference

- `projects/Ruuter-on-Rust/POST-FIX-REVIEW.md § N1`
- `projects/Ruuter-on-Rust/REVIEW.md § S2` (bare-origin case,
  already fixed for context)
- Bundle with task 052 (SSRF redirect) — both in
  `src/http_client/mod.rs`, both one-function edits.

Effort estimate: 20 min including the two new positive tests.
