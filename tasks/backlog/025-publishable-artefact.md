# 025 — Produce a publishable artefact for partner delivery

## Why

The repo currently has no git remote (`git remote -v` returns empty)
and the 0.4.0 changes live only as an uncommitted diff on the local
working tree. There is nothing to hand a critical partner beyond
"clone this local checkout." Whatever the eventual distribution
channel is (GitHub public repo, GHCR image, tarball on Buerostack
infra), it needs to exist as an addressable artefact.

## Acceptance

- Owner decision recorded on distribution channel:
  - Public GitHub repo under `buerokratt/`?
  - Private mirror + OCI image on a Buerostack registry?
  - Tarball snapshot delivered out-of-band?
- If GitHub: repo created, remote added, `main` branch pushed,
  0.4.0 tag cut, release notes = `CHANGELOG.md` 0.4.0 section.
- If OCI: image tagged `<registry>/ruuter-rs:0.4.0`, digest recorded
  in release notes, README's "Set up from scratch" updated to
  `docker pull <registry>/ruuter-rs:0.4.0` instead of `--build`.
- CI job (GH Actions or equivalent) that runs `cargo test --release`
  + `cargo audit` on every push to `main`, blocking release on failure.
