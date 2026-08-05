# compat/ — source-of-truth DSL corpus

This directory holds a snapshot of user-authored DSL files from the
Java source of truth. CI parses every file here through the Rust
loader; any parse error fails the build. Warnings are allowed
(see `EXPECTED-BASELINE.md` for the three expected on today's
corpus).

## Contents

- `java-ruuter/DSL/` — 40 files, mirror of
  `github.com/buerokratt/Ruuter/DSL/`.
- `java-ruuter/samples/` — 2 files, mirror of
  `github.com/buerokratt/Ruuter/samples/`.

## Upstream pin

Copied on **2026-08-04** from
`github.com/buerokratt/Ruuter@0454d08cf5f1b43558de195c2274e0de5a6282b1`
(the `main` HEAD at the time of copy). Re-syncing:

```sh
UPSTREAM=/path/to/buerokratt/Ruuter
rm -rf compat/java-ruuter/DSL compat/java-ruuter/samples
mkdir -p compat/java-ruuter/DSL compat/java-ruuter/samples
cp -r "$UPSTREAM/DSL/"* compat/java-ruuter/DSL/
cp -r "$UPSTREAM/samples/"* compat/java-ruuter/samples/
find compat/java-ruuter -type f -name '*.md' -delete
```

Update the SHA above and update `EXPECTED-BASELINE.md` if the parse
results change.

## License and attribution

Java Ruuter is MIT-licensed
(`github.com/buerokratt/Ruuter/blob/main/LICENSE`, © 2022
buerokratt). Ruuter-on-Rust is Apache-2.0. MIT-licensed files may
be included in an Apache-2.0 project provided the MIT notice is
preserved.

The MIT notice for these files:

```
MIT License

Copyright (c) 2022 buerokratt

Permission is hereby granted, free of charge, to any person obtaining
a copy of this software and associated documentation files (the
"Software"), to deal in the Software without restriction, including
without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to
permit persons to whom the Software is furnished to do so, subject to
the following conditions:

The above copyright notice and this permission notice shall be
included in all copies or substantial portions of the Software.
```

The `compat/java-ruuter/` tree is verbatim upstream (no modifications).
`compat/README.md` and `compat/EXPECTED-BASELINE.md` are Ruuter-on-
Rust originals under Apache-2.0.

## Expected baseline (2026-08-04)

Run `./target/debug/dsl-lint --dsl compat/java-ruuter/DSL --constants constants.ini`.
Expected:

```
dsl-lint: 40 file(s) scanned, 40 ok, 0 error(s), 3 warning(s)
```

The 3 warnings are all in Java demos of `next:` jump behaviour and
are **intentional** (steps deliberately unreachable from the entry).
See `compat/EXPECTED-BASELINE.md` for the exact list.

Run `./target/debug/dsl-lint --dsl compat/java-ruuter/samples --constants constants.ini`.
Expected:

```
dsl-lint: 2 file(s) scanned, 2 ok, 0 error(s), 0 warning(s)
```

## Files this gate does NOT cover

- **`.guard` files** (Java uses `.guard`; Rust looks for `*.guard.yml`).
  The two guard files in `compat/java-ruuter/DSL/GET/guards/{ok,fail}/.guard`
  are copied verbatim but the Rust loader skips them by extension.
  This is a documented filename divergence — the guard content
  itself parses fine (verified manually), but the discovery
  mechanism differs. Flag as a follow-up when the corpus is next
  expanded.

## CI gate

`.github/workflows/tests.yml` runs the `compat-parse` job after
`cargo build --release`. Exit non-zero on any parse error. Warnings
are allowed but reported.

The gate is intentionally strict on **errors** and permissive on
**warnings** because the corpus is upstream property and we don't
own the "should this warning fire?" decision — we own whether we
parse the file at all.
