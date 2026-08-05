# Configuration deep dive

The [Configuration](../ops/configuration.md) page in **Operations**
gives the copy-clean YAML template. This section documents each knob
individually — the default, why it exists, what breaks if it's wrong,
and how the setting maps to Java Ruuter's `application.yml`.

## Layered defaults

Ruuter's config resolution is layered:

1. `AppConfig::default()` — hard-coded defaults in
   `src/config/mod.rs`. Every field has one.
2. YAML file overrides — parsed with `serde_yaml_ng`. Unknown fields
   are **not** rejected globally, but nested structs use their own
   defaults for absent fields.
3. Env-var / CLI overrides — only for `--config` / `RUUTER_CONFIG`
   (path to the YAML file itself). Runtime config values are file-only.

There is no per-project override layer. One YAML, one process.

## Boot-time diagnostics

`warn_on_stale_config_fields` runs once at boot, after the YAML load
and before the listeners start. It emits a WARN line for each
accepted-but-inert field so operators can port a Java `application.yml`
verbatim and see exactly which knobs the Rust engine ignores. See
[Inert fields](./inert-fields.md).

## What this section covers

- [Response wrapper](./response-wrapper.md) — `response.default_wrapper`
- [Guards mode](./guards-mode.md) — `guards.mode`
- [Default exception DSL](./default-exception-dsl.md) — `default_dsl_in_case_of_exception`
- [Internal-requests / SSRF](./internal-requests.md) — SSRF allowlist and kill-switch
- [Reverse-proxy trust](./proxy-trust.md) — `proxy.trusted`
- [Listeners](./listeners.md) — multiple TCP/UDS binds
- [Unix-socket aliases](./unix-sockets.md) — `unix_socket_map`
- [Scripting limits](./scripting-limits.md) — Boa runtime caps
- [Inert fields](./inert-fields.md) — accepted-for-parity, no runtime effect

For the full field list with defaults, see the annotated example in
[Configuration](../ops/configuration.md).
