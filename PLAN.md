# Tup-Rust Conversion Plan

This is the master plan for converting [tup](https://github.com/gittup/tup) from C to Rust.
Each PR is a self-contained, testable unit of work.

**Status key:** `[ ]` = not started, `[~]` = in progress, `[x]` = done

## Phase 0: Project Foundation

- [x] **PR #1: Project skeleton**
  - Workspace Cargo.toml with all crates
  - ARCHITECTURE.md, PLAN.md
  - CI setup (cargo build, cargo test, cargo clippy)
  - .gitignore, LICENSE

- [x] **PR #2: C analysis documents**
  - `C_ANALYSIS/` directory with specs for every C module
  - These serve as contracts for Rust implementation

## Phase 1: Core Types (`tup-types`)

- [x] **PR #3: Node types, flags, and tupid**
  - `TUP_NODE_TYPE` enum (FILE, CMD, DIR, VAR, GENERATED, GHOST, GROUP, GENERATED_DIR, ROOT)
  - `TUP_LINK_TYPE` enum (NORMAL, STICKY, GROUP)
  - `TUP_FLAGS` enum (MODIFY, CREATE, CONFIG, VARIANT, TRANSIENT)
  - `tupid_t` newtype wrapper
  - Unit tests for all type conversions (i32 ↔ enum)

- [x] **PR #4: Access event types and version**
  - `ACCESS_EVENT` types
  - Version string constant
  - `TUP_CONFIG_*` constants
  - Error types (`TupError` enum)

## Phase 2: Database Layer (`tup-db`)

- [x] **PR #5: Database initialization and schema**
  - SQLite database creation
  - All CREATE TABLE and CREATE INDEX statements
  - Schema migration/version checking
  - `tup init` creates `.tup/db` with correct schema
  - Tests: create db, verify schema, reopen db

- [x] **PR #6: tup_entry and entry cache**
  - `TupEntry` struct (Rust equivalent of `struct tup_entry`)
  - Entry cache (in-memory lookup by tupid)
  - Entry creation, lookup, deletion
  - Parent-child relationships
  - Tests: CRUD operations on entries

- [x] **PR #7: Node CRUD operations**
  - `create_name_file` → insert node into DB
  - `delete_name_file` → remove node from DB
  - Node lookup by name and parent
  - Directory scanning and node creation
  - Tests: create files, directories, commands; verify DB state

- [x] **PR #8: Link/edge operations**
  - Create/delete links between nodes
  - NORMAL, STICKY, GROUP link types
  - Link queries (get inputs, get outputs)
  - Tests: link nodes, query dependencies

- [x] **PR #9: Flag operations** (completed in PRs #5 and #8)
  - Set/clear/query node flags (MODIFY, CREATE, etc.)
  - Flag propagation
  - Modified list queries
  - Tests: flag lifecycle

- [x] **PR #10: Variable database**
  - `vardb` — in-memory variable store
  - Variable CRUD in SQLite
  - @-variable resolution
  - Tests: set/get/delete variables

- [x] **PR #11: Configuration and options**
  - `tup.config` parsing
  - Option storage and retrieval
  - Default option values
  - Tests: config file parsing, option overrides

- [ ] **PR #12: Variant support**
  - Variant directory detection
  - Per-variant config
  - Variant creation and listing
  - Tests: multi-variant scenarios

## Phase 3: Graph Engine (`tup-graph`)

- [x] **PR #13: Graph data structures**
  - `Node`, `Edge`, `Graph` structs
  - Node state machine (INITIALIZED → PROCESSING → FINISHED)
  - Edge creation and traversal
  - Tests: build graph, verify structure

- [x] **PR #14: Topological sort and traversal** (completed in PR #13)
  - Topological ordering of DAG
  - BFS/DFS traversal
  - Parallel-ready traversal (identify independent nodes)
  - Tests: sort ordering, parallel groups

- [x] **PR #15: Cycle detection** (completed in PR #13)
  - Circular dependency detection
  - Group-based cycle checking
  - Error reporting with cycle path
  - Tests: detect cycles, verify error messages

- [ ] **PR #16: Graph pruning and incremental update**
  - Mark-and-sweep from flagged nodes
  - Prune unchanged subgraphs
  - Transient node handling
  - Tests: modify one file, verify minimal rebuild set

- [ ] **PR #17: Directory cache and bins**
  - Directory content caching
  - Output bin management
  - Path element groups (pel_group)
  - Tests: cache hits/misses, bin operations

## Phase 4: Platform Layer (`tup-platform`)

- [x] **PR #18: Platform detection and abstractions**
  - OS detection (Linux, macOS, Windows)
  - Platform-specific path handling
  - Terminal colors
  - Tests: platform detection on current OS

- [ ] **PR #19: File utilities**
  - `fslurp` → file reading
  - Environment manipulation
  - Debug and logging setup
  - Tests: file reading, env manipulation

- [ ] **PR #20: File locking**
  - Database file locking (flock)
  - Lock acquisition and release
  - Lock contention handling
  - Tests: concurrent lock attempts

- [x] **PR #21: Project initialization (`tup init`)**
  - Create `.tup/` directory structure
  - Initialize database
  - First working CLI command
  - Tests: `tup init` in empty directory, idempotency

## Phase 5: Tupfile Parser (`tup-parser`)

- [x] **PR #22: Basic Tupfile parsing**
  - Rule syntax: `: inputs |> command |> outputs`
  - Comment handling
  - Line continuation (`\`)
  - Tests: parse simple Tupfiles

- [x] **PR #23: Variable handling**
  - Variable assignment and reference (`$(VAR)`)
  - `@(VAR)` config variable references
  - `&(VAR)` group references
  - Append operator (`+=`)
  - Tests: variable substitution scenarios

- [x] **PR #24: Special variables and patterns**
  - `%f`, `%b`, `%B`, `%e`, `%d`, `%o`, `%g` substitution
  - `foreach` rules
  - Ordered/unordered outputs
  - Tests: pattern substitution

- [x] **PR #25: Includes and conditionals**
  - `include` directive
  - `include_rules` directive
  - `ifdef`/`ifndef`/`else`/`endif`
  - `ifeq`/`ifneq`
  - Tests: conditional compilation scenarios

- [ ] **PR #26: Globbing and path resolution**
  - Glob patterns in inputs (`*.c`)
  - Recursive globbing
  - Exclusion patterns
  - Path normalization
  - Tests: glob matching

- [ ] **PR #27: Macros and bins**
  - `!macro` definition and invocation
  - `{bin}` references
  - Chained rules
  - Tests: macro expansion

- [ ] **PR #28: Lua parser integration**
  - Lua Tupfile support (`Tupfile.lua`)
  - `tup.rule()`, `tup.foreach_rule()`
  - `tup.glob()`, `tup.getconfig()`
  - Lua helper functions
  - Tests: Lua Tupfile parsing

- [ ] **PR #29: varsed (variable substitution in files)**
  - `varsed` command handling
  - `@VAR@` substitution in file contents
  - Tests: varsed transformations

## Phase 6: Build Updater (`tup-updater`)

- [ ] **PR #30: Single-threaded updater**
  - Sequential command execution
  - Process spawning and output capture
  - Exit code handling
  - Tests: build simple projects

- [ ] **PR #31: Parallel execution**
  - Worker thread pool
  - Job scheduling from DAG
  - `-j N` parallelism option
  - Tests: parallel builds, dependency ordering

- [ ] **PR #32: Progress and timing**
  - Progress bar / status line
  - Per-command timing
  - Build summary
  - Tests: progress output format

- [ ] **PR #33: Error handling and partial builds**
  - Build failure handling
  - `--keep-going` support
  - Partial update rollback
  - Tests: build with failures

- [ ] **PR #34: Output verification**
  - Check expected outputs were created
  - Detect extra outputs (not declared)
  - Ghost node resolution
  - Tests: missing output detection

- [ ] **PR #35: ccache integration**
  - Compiler cache detection
  - Command wrapping
  - Tests: ccache-wrapped builds

## Phase 7: File Scanning (`tup-monitor` / `tup scan`)

- [ ] **PR #36: Filesystem scanner**
  - Full directory tree scan
  - Compare filesystem state to DB
  - Flag changed/new/deleted files
  - Tests: scan detects changes

- [ ] **PR #37: File monitor daemon**
  - `notify` crate integration
  - Continuous file watching
  - Event batching and deduplication
  - `tup monitor` / `tup stop` commands
  - Tests: monitor detects file changes

## Phase 8: Dependency Tracking Server (`tup-server`)

- [ ] **PR #38: Basic server (no FUSE)**
  - Process spawning with environment setup
  - Stdout/stderr capture
  - Working directory management
  - Tests: run commands through server

- [ ] **PR #39: LD_PRELOAD dependency tracking**
  - Build the C ldpreload shared library via `cc` crate
  - Inject into child processes
  - Parse dependency reports
  - Tests: detect file reads/writes

- [ ] **PR #40: FUSE server (Linux)**
  - FUSE filesystem mount
  - Intercept open/read/write/stat
  - Report dependencies back to tup
  - Tests: FUSE-based dependency detection

- [ ] **PR #41: FUSE server (macOS)**
  - macFUSE integration
  - Platform-specific adaptations
  - Tests: macOS FUSE dependency detection

## Phase 9: Integration and End-to-End Testing

- [ ] **PR #42: Test framework port**
  - Shell test harness helpers → Rust test utilities
  - Test directory setup/teardown
  - Common assertion functions
  - Parallel test execution

- [ ] **PR #43: Port t0xxx tests (initialization)**
  - Tests for `tup init`, basic project setup
  - ~20-30 tests

- [ ] **PR #44: Port t1xxx tests (basic creation/links)**
  - Tests for file creation, linking, basic DAG
  - ~50-100 tests

- [ ] **PR #45: Port t2xxx tests (parsing)**
  - Tests for Tupfile parsing, variables, rules
  - ~100-150 tests

- [ ] **PR #46: Port t3xxx tests (updates)**
  - Tests for incremental builds, updates
  - ~100-150 tests

- [ ] **PR #47: Port t4xxx-t5xxx tests (advanced)**
  - Tests for variants, groups, advanced features
  - ~100+ tests

- [ ] **PR #48: Port t6xxx-t9xxx tests (edge cases)**
  - Remaining test categories
  - Error handling tests
  - ~100+ tests

- [ ] **PR #49: Port benchmark tests**
  - Performance benchmarks (b*.sh)
  - Criterion-based benchmarks
  - Performance comparison with C version

## Phase 10: Polish and Parity

- [ ] **PR #50: CLI parity**
  - All subcommands working
  - Help text
  - Exit codes matching C version
  - Man page generation

- [ ] **PR #51: Graph visualization**
  - `tup graph` command
  - Graphviz DOT output
  - Same format as C version

- [ ] **PR #52: Windows support**
  - Cross-compilation setup
  - DLL injection (C FFI)
  - Windows-specific tests

- [ ] **PR #53: Full compatibility testing**
  - Run C tup test suite against Rust binary
  - Fix remaining incompatibilities
  - Document any intentional differences

---

## Workflow Per PR

Each PR follows this process:

1. **Read spec**: Agent reads `C_ANALYSIS/<module>.md`
2. **Implement**: Agent writes Rust code in the appropriate crate
3. **Test**: Agent writes unit tests and integration tests
4. **Validate**: Separate agent runs `cargo build`, `cargo test`, `cargo clippy`
5. **PR**: Create PR with clear description mapping to this plan
6. **Update PLAN.md**: Mark PR as done

## Guiding Principles

1. **Correctness over performance**: Match C behavior exactly first, optimize later
2. **Test everything**: No PR without tests
3. **Small PRs**: Each PR should be reviewable in isolation
4. **Use Rust idioms**: Don't write C-in-Rust. Use Result, enums, traits, iterators
5. **Document differences**: If Rust version intentionally differs from C, document why
6. **Keep compiling**: Every PR leaves the workspace in a buildable, test-passing state
