# Inert fields

Config fields the loader accepts (so an operator can port a Java
`application.yml` verbatim) but that don't influence any runtime
behaviour. Each of these emits a WARN line at boot, so the operator
can see which knob is being ignored.

## Why they exist

Java Ruuter's `application.yml` grew a large surface over years of
service. Porting an operator's file straight into Ruuter-on-Rust and
seeing the process refuse to boot on unknown fields is hostile — the
operator has to hunt through the diff to identify what got dropped.

Instead, the Rust loader accepts every historical noun, warns per
inert field, and documents the intended semantics next to the WARN
message. See `warn_on_stale_config_fields` in `src/config/mod.rs`.

## The current inert list

| Field | Warning surface | Why it's inert |
|---|---|---|
| `stop_in_case_of_exception: false` | WARN when explicitly set to `false` | The engine propagates every step error via `?`, so it always stops. Java's continue-on-error semantics are not implemented. Remove the setting or leave the default (`true`). |
| `dsl.allowed_filetypes` differing from `dsl.processed_filetypes` | WARN when the two lists differ | The loader only consults `processed_filetypes`. `allowed_filetypes` is a Java-parity noun with no gating effect. Fold the two into `processed_filetypes` or accept that `allowed_filetypes` is inert. |

The four Java-parity `logging.*` flags used to live here but are
now wired end-to-end — see [Logging](../logging/index.md).

## What breaks if you set it wrong

Nothing — these fields have no runtime effect regardless of value.
The WARN line at boot is the operator-visible signal that a knob was
noticed but ignored. If your log-management tooling treats WARN as
actionable, either remove the inert field or filter the specific
WARN by message text.

## When these might get wired

`stop_in_case_of_exception` is Java-shape parity whose continue-on-
error semantics don't survive translation to Ruuter-on-Rust's engine
model (every step propagates via `?`). `dsl.allowed_filetypes`
remains for source-level parity with Java sample configs.

The four `logging.*` flags used to live in this table but have
been wired end-to-end — see [Logging](../logging/index.md).

## Cross-links

- [Configuration overview](../ops/configuration.md)
