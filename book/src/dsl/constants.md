# Constants

Substitute values from `constants.ini` at DSL parse time using `[#KEY]`.

## File format

```ini
# comments start with '#'; blank lines ignored
[DSL]                     # section headers accepted but IGNORED — keys are flat
DOMAIN_URL=https://api.example.com
API_TOKEN=abc123

[other]                   # this section header is decorative only
FEED_KEY=xyz
```

Keys resolved by:

```yaml
call:
  args:
    url: "[#DOMAIN_URL]/v1/orders"
    headers: { Authorization: "Bearer [#API_TOKEN]" }
```

## Substitution rules

- Happens **before** JS evaluation. `[#KEY]` becomes literal text in the parsed DSL, then `${...}` runs against that text.
- **Missing key in a DSL body**: substituted as the literal string `[#KEY]` — visible at runtime.
- **Missing key in a WS source config** (`url:` or `headers:`): load-time error, source refuses to start.
- Section headers (`[DSL]`, etc.) are accepted for Java-Ruuter compatibility and silently dropped — keys are always flat.

## File location

Read from `./constants.ini` at Ruuter's working directory. In Docker: mount as `/app/constants.ini:ro`.

## Secrets

Ruuter does not fetch secrets from Vault / KMS / Docker secrets / any external store. Mount the fully-resolved file. Rotation is the deploy pipeline's job.
