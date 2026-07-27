# Constants

Substitute values from `constants.ini` at DSL parse time using
`[#KEY]` or `#{KEY}`. Both syntaxes are accepted and produce
identical output — pick whichever reads better in context.

## File format

```ini
# comments start with '#'; blank lines ignored
[DSL]                     # section headers accepted but IGNORED — keys are flat
DOMAIN_URL=https://api.example.com
API_TOKEN=abc123

[other]                   # this section header is decorative only
FEED_KEY=xyz
```

Keys resolved by either syntax:

```yaml
call:
  args:
    # Legacy Java-Ruuter syntax (indefinite backward compat):
    url: "[#DOMAIN_URL]/v1/orders"
    # Alternate syntax — visually mirrors `${...}` runtime variables:
    headers:
      Authorization: "Bearer #{API_TOKEN}"
```

## Which syntax to pick

- **`#{KEY}`** — preferred for new DSLs. Visually pairs with
  `${runtime}` so at a glance you can tell which interpolations are
  compile-time (`#{}`) and which are per-request (`${}`).
- `[#KEY]` — original Java-Ruuter form. Retained for backward
  compatibility so existing DSLs keep parsing, but **may be
  deprecated in a future major release**. New DSLs should prefer
  `#{KEY}`.

Both resolve against the same `constants.ini`. Mixing forms in one
file works today; if you're touching an existing DSL, migrating its
`[#KEY]` references to `#{KEY}` is a low-risk change (identical
substitution, same tests pass).

## Substitution rules

- Happens **before** JS evaluation. `[#KEY]` and `#{KEY}` become
  literal text in the parsed DSL, then `${...}` runs against that
  text.
- **Missing key in a DSL body**: substituted as the literal string
  (`[#KEY]` or `#{KEY}` — whichever the author wrote) and visible at
  runtime. The `dsl-lint` tool surfaces every unresolved reference
  with its exact form.
- **Missing key in a WS source config** (`url:` or `headers:`):
  load-time error, source refuses to start.
- Section headers (`[DSL]`, etc.) are accepted for Java-Ruuter
  compatibility and silently dropped — keys are always flat.

## Runnable example

`constants.ini` (shipped with the repo):

```ini
[DSL]
DOMAIN_URL=https://example.com
LOCAL_RUUTER=http://localhost:8080
PORT=8080
```

`DSL/samples/GET/constants/demo.yml`:

```yaml
respond:
  return:
    domain: "[#DOMAIN_URL]"
    also_domain: "#{DOMAIN_URL}"
    port: "#{PORT}"
    note: "Both bracket and brace forms are equivalent — pick whichever reads better."
  next: end
```

Request:

```bash
curl -s http://localhost:8080/samples/constants/demo | jq .
```

Response — the constants were baked in at DSL parse time (no runtime
lookup):

```json
{
  "also_domain": "https://example.com",
  "domain": "https://example.com",
  "note": "Both bracket and brace forms are equivalent — pick whichever reads better.",
  "port": "8080"
}
```

## File location

Read from `./constants.ini` at Ruuter's working directory. In Docker:
mount as `/app/constants.ini:ro`.

## Secrets

Ruuter does not fetch secrets from Vault / KMS / Docker secrets / any
external store. Mount the fully-resolved file. Rotation is the deploy
pipeline's job.
