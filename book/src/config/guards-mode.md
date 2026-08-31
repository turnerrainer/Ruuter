# Guards mode

Controls whether nested guard files stack (Rust default) or only the
closest-matching guard runs (Java parity).

## What it is

Java Ruuter's `DslService.getGuard` picks the single innermost ancestor
guard whose directory prefix matches the request path, and runs
**only** that one.

Ruuter-on-Rust default (`stack`) runs **every** matching ancestor
guard from outermost to innermost. That's stricter — an inherited
`admin.guard.yml` at `/admin/` also fires for `/admin/users/delete`
even when a more specific `.guard.yml` is present in the same tree.

## The config

```yaml
guards:
  mode: stack           # default — every ancestor guard runs outer-first
```

or

```yaml
guards:
  mode: closest_only    # Java parity — only the innermost matching guard runs
```

## The default and why

`stack` — safer default. A DSL author dropping a broader guard file
higher up in the tree gets the check they wrote, regardless of what
finer-grained guards live below it. Operators porting from Java pick
`closest_only` to preserve existing skip-outer behaviour.

Guards that set `override_ancestors: true` are the escape hatch in
either mode.

## Interaction with project-level guards (issue #39)

`guards.mode` narrows *method-scoped* guards only. A project-level
`<project>/.guard.yml` (issue #39) always runs as the outermost guard
regardless of the mode — silently dropping it under `closest_only`
would break the "add auth once, protects everything" contract that
the project-level convention exists for.

If a nested method-scoped guard sets `override_ancestors: true`, the
project-level guard is still bypassed for that subtree — the escape
hatch replaces *every* ancestor, project-level included. That's the
mechanism for carving out a public endpoint under an otherwise-
protected project.

## What breaks if you set it wrong

- Setting `closest_only` when your DSL tree assumes stacking → the
  outer auth check disappears for any request whose route sits under a
  more specific `.guard.yml`. This can silently open access.
- Leaving the default `stack` when porting from Java → previously
  bypassed outer guards now fire and may reject requests the Java
  system passed through. Symptoms: unexpected 403s on formerly-working
  routes.

## Migration from Java

Set `guards.mode: closest_only` to preserve Java semantics one-for-one.
Then, per route, decide whether the extra stacked check is desirable
and either promote it into a specific guard or set `override_ancestors:
true` on the innermost guard to opt back into single-guard behaviour.

## Cross-links

- [Guards DSL reference](../dsl/guards.md)
- [Reserved subdirectories](../reference/reserved-subdirs.md)
