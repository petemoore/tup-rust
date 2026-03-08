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

1. **Check PLAN.md** for the next `[ ]` PR to work on
2. **Read the relevant `C_ANALYSIS/*.md`** file for the spec
3. **Implement in the correct crate** per ARCHITECTURE.md
4. **Write tests** — every PR must have tests
5. **Run `cargo build && cargo test && cargo clippy`** before creating a PR
6. **Create PR** on `petemoore/tup-rust` via `gh pr create`
7. **Update PLAN.md** to mark the PR as `[x]`

## Conventions

- Use Rust idioms (Result, enums, traits, iterators) — don't write C-in-Rust
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
- Branch naming: `pr/NN-short-description`
