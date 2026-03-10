# CLAUDE.md — Project Instructions

## What This Project Is

This is a Rust port of [tup](https://github.com/gittup/tup), a file-based build system
originally written in C (~407K lines, 144 .c files). The original is GPL-2.0 by Mike Shal.
The original C source is at `~/git/tup/`.

## Key Files

- `PLAN.md` — Master conversion plan with ~53 PRs. Check status here first.
- `ARCHITECTURE.md` — Maps every C module to its Rust crate/module.
- `C_ANALYSIS/` — Detailed specs of each C module (function signatures, data structures, behavior). **Read these instead of the C source when implementing.**
- `crates/` — Rust workspace with 9 crates.

## How to Work on This Project

1. **Check PARITY_PLAN.md** for the current focus area
2. **Find the C function** in `~/git/tup/src/tup/` that implements the feature
3. **Read the C code** line by line, then port the logic to Rust
4. **Write tests** — every PR must have tests
5. **Run `cargo build && cargo test && cargo clippy && cargo fmt --all`** before creating a PR
6. **Never use `git commit --no-verify`** — fix underlying issues instead of bypassing hooks
7. **Create PR** on `petemoore/tup-rust` via `gh pr create`
8. **Continue working autonomously** — don't stop to ask questions, make reasonable decisions and keep going. The goal is complete feature parity with the C implementation.

## CRITICAL: Port from C, Don't Invent

**Every feature must be ported by reading the C source first.** Do NOT design
solutions from scratch. The C implementation has already solved every problem.

1. **Find the C function** that implements the feature (in `~/git/tup/src/tup/`)
2. **Read it line by line** — understand the algorithm, the SQL queries, the error messages
3. **Port the logic to Rust** — translate C idioms to Rust idioms, but preserve the algorithm
4. **Match error messages exactly** — tests grep for specific strings
5. **Match SQL queries** — the DB schema is identical; queries should be too
6. **Match the update phases** — C has: scan → config → create → modify → execute
7. **If unsure, check C first** — never guess when the answer is in the source
8. **Preserve the architecture** — if C uses a thread, use a thread. If C uses fork,
   use fork. If C uses a socket, use a socket. Do NOT substitute different paradigms
   (e.g. don't replace a notifier thread with inline reads, don't replace fork with
   std::process::Command, don't replace socketpair with channels). The C code has
   been tested and debugged for years — changing the paradigm introduces new bugs.

The C source at `~/git/tup/src/tup/` is the specification. The 989 test scripts
are the acceptance criteria. When in doubt, run `VERBOSE=1 ./tests/compat/run_tests.sh tNNNN`
against both C tup (`/usr/local/bin/tup`) and our binary to compare behavior.

### Key C source files (by importance):
- `updater.c` (3080 LOC) — the heart: phased update engine
- `db.c` (7841 LOC) — all DB operations, flag propagation, link graph queries
- `parser.c` (4351 LOC) — Tupfile parsing, rule storage
- `graph.c` (1444 LOC) — DAG topological sort, cycle detection
- `luaparser.c` (990 LOC) — Lua Tupfile support
- `entry.c` (835 LOC) — node lifecycle, ghost management
- `file.c` (876 LOC) — file metadata, output verification

## Conventions

- Use Rust idioms (Result, enums, traits, iterators) — but preserve C algorithms
- Use `thiserror` for library error types, `anyhow` for application errors
- Use `BTreeMap`/`Vec` instead of porting BSD tree.h/queue.h macros
- `tupid_t` is a newtype around `i64`
- Match C behavior exactly before optimizing
- Tests go in the same file as the code (`#[cfg(test)] mod tests`)
- Integration tests go in `tests/` at workspace root

## Crate Dependency Order (build bottom-up)

```
tup-types → tup-db → tup-graph → tup-parser → tup-updater
                                              → tup-server
            tup-platform
            tup-monitor
All → tup-cli
```

## GitHub

- Repo: petemoore/tup-rust
- PRs: one per PLAN.md item, merge to main
- Branch naming: `pr/short-description`
- **Branch protections are enabled on `main`** — CI must pass before merging
- After creating a PR, wait for CI checks to complete (`gh pr checks <number>`)
- After merging, verify with `gh pr view <number> --json state` that state is "MERGED"
- If CI fails, check logs with `gh run view --log-failed --job=<job-id>`, fix, push, and wait for CI again
