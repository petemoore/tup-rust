# Drop-in Replacement Parity Plan

This plan covers the work needed to go from "working build system with the
right architecture" to "drop-in replacement for C tup."

## Gap Analysis

### Gap 1: Database-Driven Incremental Builds (CRITICAL)
**Current state:** Updater re-parses all Tupfiles from scratch every time.
**Target state:** Database is the source of truth. Only modified files trigger
re-parsing, and only affected commands are re-executed.

### Gap 2: C Test Suite (CRITICAL)
**Current state:** 65 hand-written integration tests.
**Target state:** All ~986 C tests pass against the Rust binary.

### Gap 3: FUSE/LD_PRELOAD Functional Testing (HIGH)
**Current state:** Abstraction layers and C source written but untested.
**Target state:** LD_PRELOAD works on Linux, FUSE mounts and intercepts.

### Gap 4: Parser Edge Cases (MEDIUM)
**Current state:** Core syntax works, edge cases untested.
**Target state:** Byte-for-byte compatible parsing with C tup.

### Gap 5: Ghost/Group Lifecycle (MEDIUM)
**Current state:** Ghost upgrade on create works, but full ghost reclamation
(at commit), group link management, and transient file handling are stubs.
**Target state:** Full ghost lifecycle matching C behavior.

---

## Phase 1: Database-Driven Build Cycle

This is the single most important piece. Without it, tup-rust is just
a fancy script runner.

### PR D1: Scan → Database Sync
- [ ] On `tup upd`, scan filesystem and compare to node table
- [ ] Create FILE nodes for new files, flag MODIFY for changed mtimes
- [ ] Flag DELETE for files in DB but not on disk
- [ ] Flag CREATE for directories containing modified Tupfiles
- [ ] Tests: add file → scan → node appears in DB

### PR D2: Parse → Database Commands
- [ ] Parse only directories in create_list (not all Tupfiles)
- [ ] For each rule: create CMD node, link inputs → CMD → outputs
- [ ] Use tup_db_create_node with ghost upgrade semantics
- [ ] Track command identity by hashing command string
- [ ] When a command changes: flag MODIFY
- [ ] When a command is removed: mark for deletion
- [ ] Tests: parse Tupfile → verify CMD/link nodes in DB

### PR D3: Update → Execute from Graph
- [ ] Build graph from modify_list (flagged commands)
- [ ] Topological sort for execution order
- [ ] Execute only commands in the graph
- [ ] After success: clear MODIFY flag, update output mtimes
- [ ] After failure: leave flags for next run
- [ ] Tests: modify input → only affected command re-runs

### PR D4: Ghost Reclamation
- [ ] At commit: scan ghost_root tree
- [ ] Reclaim ghosts with no references (no dir, no links)
- [ ] Multiple passes for nested ghosts
- [ ] Tests: delete file → ghost created → ghost reclaimed

### PR D5: Output Tracking
- [ ] After command execution: verify declared outputs exist
- [ ] Detect undeclared outputs (files created but not in output list)
- [ ] Handle transient files (flag 't': delete after consumers finish)
- [ ] Update output node mtimes in database
- [ ] Tests: command creates extra file → warning

### PR D6: Cross-Directory Dependencies
- [ ] Inputs from other directories create links across dir boundaries
- [ ] When an included file changes, re-parse the including directory
- [ ] Variant directory mirroring of source tree
- [ ] Tests: change header in dir A → command in dir B re-runs

---

## Phase 2: C Test Suite Porting

### PR T1: Test Runner Adaptation
- [ ] Create a shell script wrapper that aliases `tup` to the Rust binary
- [ ] Adapt test/tup.sh helper functions to work with Rust binary
- [ ] Handle differences in output format (if any)
- [ ] Run t0000-t0005 (init tests) — fix any failures

### PR T2: t0xxx-t1xxx Tests
- [ ] Port/run t0000-t0005 (initialization)
- [ ] Port/run t1000-t1012 (basic commands)
- [ ] Fix all failures
- [ ] Document any intentional behavioral differences

### PR T3: t2xxx Tests (Parsing)
- [ ] Port/run t2000-t2217 (~200 tests)
- [ ] These will expose parser edge cases
- [ ] Fix all failures

### PR T4: t3xxx Tests (Variables/Groups)
- [ ] Port/run t3000-t3093 (~94 tests)
- [ ] Group dependency handling
- [ ] Fix all failures

### PR T5: t4xxx+ Tests (Advanced)
- [ ] Port/run t4000+ (~600 tests)
- [ ] Monitor interaction, refactoring, compilation
- [ ] Fix all failures (many may require FUSE/LD_PRELOAD)

---

## Phase 3: FUSE/LD_PRELOAD Testing

### PR F1: LD_PRELOAD Linux Testing
- [ ] Set up Linux test environment (Docker or VM)
- [ ] Verify ldpreload.so compiles and loads
- [ ] Test: run `gcc -c foo.c` with LD_PRELOAD, verify depfile
- [ ] Test: compare depfile output with C tup's depfile
- [ ] Wire into ProcessServer for automatic use on Linux

### PR F2: FUSE Integration
- [ ] Add `fuser` crate dependency (conditional on FUSE availability)
- [ ] Implement TupFuseOps for PassthroughFuse
- [ ] Mount test: create mount, read file through it, verify access recorded
- [ ] Wire into ProcessServer as ServerMode::Fuse

### PR F3: Depfile Wire Format Compatibility
- [ ] Verify byte-level compatibility with C tup's depfile format
- [ ] Test: C ldpreload writes, Rust reads (and vice versa)
- [ ] Handle platform alignment differences if any

---

## Phase 4: Parser Hardening

### PR P1: Edge Case Audit
- [ ] Systematic comparison of parser.c line-by-line with Rust parser
- [ ] Test each C parser branch that we haven't covered
- [ ] Specific areas: |> splitting, run directive, preload directive
- [ ] Bang macro extension variants (!cc.S, !cc.cpp)

### PR P2: Variable Expansion Parity
- [ ] $(TUP_CWD), $(TUP_VARIANTDIR), $(TUP_VARIANT_OUTPUTDIR)
- [ ] @(CONFIG_VAR) with dependency tracking
- [ ] &(node_var) references
- [ ] Quoted percent variants (%'f, %"f)
- [ ] Numbered inputs (%1f, %2o)

### PR P3: Lua API Completeness
- [ ] tup.include(file)
- [ ] tup.getrelativedir(dirname)
- [ ] tup.nodevariable(path) with __tostring metamethod
- [ ] tup.getdirectory()
- [ ] Verify tup.definerule return value matches C behavior

---

## Phase 5: Performance & Polish

### PR B1: Benchmarks
- [ ] Criterion benchmarks for: parse, scan, graph build, topo sort
- [ ] Comparison script: C tup vs Rust tup on same project
- [ ] Profile hot paths

### PR B2: Error Messages
- [ ] Match C tup's error message format exactly
- [ ] File:line references in parse errors
- [ ] Colored output matching C tup's color scheme

### PR B3: CLI Completeness
- [ ] tup dbconfig (show internal DB config)
- [ ] tup entry / tup type / tup tupid (DB query commands)
- [ ] tup refactor (parse-only, detect changes)
- [ ] tup generate (export build script)
- [ ] tup compiledb (compile_commands.json)
- [ ] tup todo (show planned commands)

---

## Priority Order

1. **Phase 1 (D1-D3)** — Without database-driven builds, nothing else matters
2. **Phase 2 (T1-T2)** — Run basic C tests to catch obvious differences
3. **Phase 1 (D4-D6)** — Complete the database integration
4. **Phase 2 (T3-T5)** — Run remaining C tests
5. **Phase 4 (P1-P3)** — Fix parser issues found by tests
6. **Phase 3 (F1-F3)** — FUSE/LD_PRELOAD for Linux
7. **Phase 5 (B1-B3)** — Performance and polish

Estimated effort: Phases 1-2 are the bulk of the work (~60%).
Phases 3-5 are important but less critical for basic parity.
