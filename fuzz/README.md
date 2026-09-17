# Fuzz targets — h2ck.me v1 T-23

`cargo-fuzz` targets for the highest-value parsers in Ruuter-on-Rust.
Fleet doctrine §9.3 names DSL loading as adoption target #1 and DTO
deserialisation as adoption target #2.

## Layout

```
fuzz/
├── Cargo.toml                    # nightly-only fuzz crate
├── fuzz_targets/
│   ├── dsl_yaml_load.rs          # DSL YAML → in-memory AST
│   └── http_body_deserialize.rs  # JSON HTTP body → serde_json::Value
└── corpus_seed/                  # checked-in seed inputs (copy into corpus/ on first run)
    ├── dsl_yaml_load/            # a slice of DSL/samples/*.yml
    └── http_body_deserialize/    # JSON bodies from real test fixtures
```

## Running

Requires **nightly Rust** (libfuzzer-sys uses the LLVM sanitizer,
which is nightly-only) and `cargo fuzz`:

```bash
cargo install cargo-fuzz
rustup toolchain add nightly

# Copy the checked-in seeds into the runtime corpus directory
# (cargo-fuzz will grow this as new coverage is found — the runtime
# dir is git-ignored, the seed dir is not).
mkdir -p fuzz/corpus/dsl_yaml_load
cp fuzz/corpus_seed/dsl_yaml_load/* fuzz/corpus/dsl_yaml_load/
mkdir -p fuzz/corpus/http_body_deserialize
cp fuzz/corpus_seed/http_body_deserialize/* fuzz/corpus/http_body_deserialize/

# Run either target for 10 minutes.
cargo +nightly fuzz run dsl_yaml_load -- -max_total_time=600
cargo +nightly fuzz run http_body_deserialize -- -max_total_time=600
```

## What each target proves

### `dsl_yaml_load`
- The DSL parser (`DslParser::parse_content`) never panics on
  arbitrary input.
- Malformed YAML surfaces as `Err`, not `panic!()`.

### `http_body_deserialize`
- `serde_json::from_slice::<Value>()` — the same call the
  inbound handler makes at `src/router/mod.rs:917` — never
  panics.
- Round-trip invariant: `parse(serialize(v)) == v` for every
  accepted value. Any parser that silently loses information
  (numeric precision, key drop, duplicate-key collision)
  fails the assertion.

## CI

`.github/workflows/fuzz.yml` runs both targets nightly for 10
minutes each. Findings are uploaded as artefacts so the next
committer can reproduce locally.

## Adding a new target

1. Add a `fuzz_targets/<name>.rs` file with `#![no_main]` and a
   `libfuzzer_sys::fuzz_target! { |data: &[u8]| { ... } }` block.
2. Add a `[[bin]]` entry to `fuzz/Cargo.toml`.
3. If you want a seed corpus checked in, place inputs under
   `corpus_seed/<name>/`.
4. Add a matrix entry in `.github/workflows/fuzz.yml`.

Prefer invariant assertions (`assert_eq!(parse(serialize(x)), x)`)
over "does it panic" — invariants find silent bugs that "no
panic" misses, and every finding becomes a checked-in `#[test]`
in the main suite afterwards.

## Historical crashes

Whenever a fuzz run surfaces a crash, do NOT just fix the code —
also copy the failing input into `tests/` as a `#[test]` case so
the finding stays pinned even if the fuzz-corpus rotation loses
it. Findings compound; the fuzz suite is worth more each release.
