# Prerequisites

The Getting Started path needs three tools. Two are mandatory, one is
optional.

| Tool | Why | Install check |
|---|---|---|
| **Docker** + Docker Compose v2 | Runs Ruuter as a container | `docker compose version` |
| **curl** | Hits endpoints from the shell | `curl --version` |
| **Postman** (Desktop) *or* **Newman** (CLI) | Runs the shipped functional-test collection | `newman --version` (skip if using the Postman app) |

Optional for the "watch the tests pass" chapter:

| Tool | Why |
|---|---|
| **Rust toolchain** (1.88+) | Compiles the Rust test suite (`cargo test`) and the `dsl-lint` / `dsl-test` binaries locally. Everything else works Docker-only. |

Install Rust with [rustup](https://rustup.rs): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`.

Install Newman (only if you don't have the Postman desktop app):
`npm install -g newman`.

Next: [Run it locally](./run-locally.md).
