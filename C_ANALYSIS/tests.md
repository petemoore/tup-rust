# Test Suite Specification

## Framework (test/tup.sh)

### Initialization
- Sets PATH to include local tup binary
- Creates isolated test directory: `tuptesttmp-<testname>`
- Runs `tup init --no-sync --force`
- Each test sources `./tup.sh`

### Key Test Functions

**Build Operations:**
- `update()` — full tup update (fails on error)
- `update_partial()` — update without flag checking
- `update_fail()` — expect update to fail
- `update_fail_msg()` — expect failure with specific message
- `update_null()` — expect no changes
- `parse()` / `parse_fail_msg()` — parse-only operations
- `refactor()` / `refactor_fail_msg()`
- `generate()` / `generate_fail_msg()`

**Assertions:**
- `check_exist()` / `check_not_exist()` — file existence
- `tup_object_exist()` / `tup_object_no_exist()` — database node existence
- `tup_dep_exist()` / `tup_dep_no_exist()` — normal dependency
- `tup_sticky_exist()` / `tup_sticky_no_exist()` — sticky dependency

**Monitor:**
- `monitor()` / `stop_monitor()` / `wait_monitor()`

**Config:**
- `varsetall()` — set tup.config variables
- `single_threaded()` — set num_jobs=1
- `set_full_deps()` / `clear_full_deps()`

**Platform Checks:**
- `check_windows()` / `check_no_windows()` / `check_no_osx()`
- `check_monitor_supported()` / `check_tup_suid()`

## Test Categories (986 functional + 8 benchmark)

| Range | Category | Description |
|-------|----------|-------------|
| t0000-t0005 | Init/Setup | Database initialization |
| t1000-t1012 | Basic Commands | tup create, graph, version |
| t2000-t2217 | Core Tupfile | Rules, foreach, variables, includes, globs |
| t3000-t3093 | Variables & Groups | Variable substitution, config vars, groups |
| t4000+ | Advanced | Compilation, subprocess, refactor, monitors |

## Test Runner (test/test.sh)
```bash
./test.sh                    # Run all tests
./test.sh --keep-going       # Don't stop on failure
./test.sh t0000-init.sh      # Run specific test
```

## Benchmark Tests (test/bench.sh)
8 benchmarks parametrized by count (default 100): init, create, link, update, reupdate, delete, create-select, link-select.

## Porting Strategy
1. Create Rust test harness library replicating tup.sh functions
2. Each t*.sh → Rust test function
3. Keep shell tests runnable alongside Rust tests
4. Add parallel test execution (shell version is sequential)
