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
    headers: { Authorization: "Bearer #{API_TOKEN}" }
```

## Which syntax to pick

- `[#KEY]` — original Java-Ruuter form. Kept indefinitely; every
  existing DSL still parses.
- `#{KEY}` — alternate form added by task 067. Visual pairing with
  `${runtime}` makes it obvious at a glance which interpolations are
  compile-time (`#{}`) and which are per-request (`${}`).

Both resolve against the same `constants.ini`. Mixing forms in one
file is fine. There is no plan to deprecate `[#KEY]`.

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

## File location

Read from `./constants.ini` at Ruuter's working directory. In Docker:
mount as `/app/constants.ini:ro`.

## Secrets

Ruuter does not fetch secrets from Vault / KMS / Docker secrets / any
external store. Mount the fully-resolved file. Rotation is the deploy
pipeline's job.
