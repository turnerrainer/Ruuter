# tasks/ — Ruuter-RS

Project board for operational work on Ruuter-RS, the generic Rust
implementation of the Ruuter declarative router.

**Scope guardrail.** Ruuter is a generic core component. It may
NEVER contain service-specific source code (no stock-trading,
no Alpaca, no domain-specific helpers). Every service is defined
exclusively through `constants.ini` + YAML DSLs + source configs.
Tasks that would violate this guardrail must be rejected or
re-scoped to a generic primitive.

## Layout

```
tasks/
├── backlog/        ← things to do (any priority; pull from here when picking next work)
├── in-progress/    ← the ONE task currently being worked
├── blocked/        ← waiting on external (time, evidence, decision)
└── done/           ← completed; frozen historical record
```

`in-progress/` holds one task at a time — the current focus.

## File-naming convention

- **Active** (`backlog/`, `in-progress/`, `blocked/`):
  `NNN-short-slug.md` where NNN is a stable monotonic ID
- **Done**: `YYYY-MM-DD-NNN-short-slug.md`

## Moving a task between statuses

```bash
mv tasks/backlog/X.md tasks/in-progress/        # promote
mv tasks/in-progress/X.md tasks/blocked/        # external blocker
mv tasks/in-progress/X.md tasks/done/YYYY-MM-DD-X.md   # complete
```

## ID namespace

This board has its own ID namespace. Numbering starts at 001.

## Conventions

- IDs are stable across status moves
- Append to `backlog/` (or `in-progress/`) with the next available ID
- Don't reuse IDs on cancel; mark cancelled at the top of the file and
  move to `done/` instead
