# Architecture: C to Rust Module Mapping

This document maps the original C tup source code to the Rust crate structure.

## Crate Overview

```
tup-types      Core types, enums, constants (no dependencies)
tup-db         SQLite database layer (depends on tup-types)
tup-graph      DAG engine (depends on tup-types, tup-db)
tup-parser     Tupfile parser + Lua (depends on tup-types, tup-db, tup-graph)
tup-updater    Build executor (depends on tup-types, tup-db, tup-graph, tup-server)
tup-monitor    File system monitor (depends on tup-types, tup-db)
tup-server     FUSE/LD_PRELOAD server (depends on tup-types, tup-db)
tup-platform   OS abstraction (depends on tup-types)
tup-cli        Binary entry point (depends on all above)
```

## C → Rust Mapping

### tup-types (foundational types, zero dependencies)

| C Source | Rust Module | Description |
|----------|-------------|-------------|
| `db_types.h` | `types.rs` | TUP_NODE_TYPE, TUP_LINK_TYPE, TUP_FLAGS |
| `tupid.h` | `tupid.rs` | tupid_t type (i64 wrapper) |
| `access_event.h` | `access_event.rs` | Access event types |
| `version.h` | `version.rs` | Version string |
| `container.h` | `container.rs` | Container macros → Rust traits |
| `flist.h` | `flist.rs` | File list types |
| `array_size.h` | (not needed) | Compile-time array size macro |
| `compat.h` | (not needed) | C compat shims |

### tup-db (database layer)

| C Source | Rust Module | Description |
|----------|-------------|-------------|
| `db.c/h` | `db.rs` | Main database operations (~7.8K lines) |
| `entry.c/h` | `entry.rs` | tup_entry cache |
| `create_name_file.c` | `create.rs` | Node creation |
| `delete_name_file.c` | `delete.rs` | Node deletion |
| `tupid_tree.c/h` | (use BTreeMap) | tupid-keyed trees |
| `tent_tree.c/h` | (use BTreeMap) | tup_entry trees |
| `string_tree.c/h` | (use BTreeMap/HashSet) | String-keyed trees |
| `tupid_list.c/h` | (use Vec) | tupid lists |
| `tent_list.c/h` | (use Vec) | tup_entry lists |
| `vardb.c/h` | `vardb.rs` | Variable database |
| `variant.c/h` | `variant.rs` | Build variants |
| `option.c/h` | `option.rs` | Configuration options |
| `config.c/h` | `config.rs` | Runtime config |

### tup-graph (DAG engine)

| C Source | Rust Module | Description |
|----------|-------------|-------------|
| `graph.c/h` | `graph.rs` | Graph construction, traversal, pruning |
| `bin.c/h` | `bin.rs` | Output bins |
| `pel_group.c/h` | `pel_group.rs` | Path element groups |
| `dircache.c/h` | `dircache.rs` | Directory cache |
| `thread_tree.c/h` | (use HashMap) | Thread-keyed trees |

### tup-parser (Tupfile parsing)

| C Source | Rust Module | Description |
|----------|-------------|-------------|
| `parser.c/h` | `parser.rs` | Main Tupfile parser |
| `luaparser.c/h` | `luaparser.rs` | Lua-based parser |
| `if_stmt.c/h` | `if_stmt.rs` | Conditional processing |
| `varsed.c/h` | `varsed.rs` | Variable substitution in files |
| `vardict.c/h` | `vardict.rs` | Variable dictionary |
| `path.c/h` | `path.rs` | Path manipulation |
| `estring.c/h` | `estring.rs` | Extensible strings |

### tup-updater (build execution)

| C Source | Rust Module | Description |
|----------|-------------|-------------|
| `updater.c/h` | `updater.rs` | Build executor with parallel jobs |
| `progress.c/h` | `progress.rs` | Progress reporting |
| `timespan.c/h` | `timespan.rs` | Timing |
| `ccache.c/h` | `ccache.rs` | Compiler cache support |

### tup-monitor (file system monitoring)

| C Source | Rust Module | Description |
|----------|-------------|-------------|
| `monitor/inotify.c` | `inotify.rs` | Linux inotify monitor |
| `monitor/` | `monitor.rs` | Monitor daemon |
| `send_event.c` | `event.rs` | Event dispatch |

### tup-server (dependency tracking server)

| C Source | Rust Module | Description |
|----------|-------------|-------------|
| `server/fuse_server.c` | `fuse.rs` | FUSE filesystem server |
| `server/fuse_fs.c` | `fuse_fs.rs` | FUSE filesystem operations |
| `server/master_fork.c` | `master_fork.rs` | Fork management |
| `file.c/h` | `file.rs` | File access tracking |
| `src/ldpreload/` | (stays C) | LD_PRELOAD shared library |
| `src/dllinject/` | (stays C) | Windows DLL injection |

### tup-platform (OS abstraction)

| C Source | Rust Module | Description |
|----------|-------------|-------------|
| `platform.c/h` | `platform.rs` | Platform detection |
| `flock/` | `flock.rs` | File locking |
| `lock.c/h` | `lock.rs` | Database locking |
| `fslurp.c/h` | `fslurp.rs` | File reading utilities |
| `privs.h` | `privs.rs` | Privilege management |
| `environ.c/h` | `environ.rs` | Environment manipulation |
| `colors.c/h` | `colors.rs` | Terminal colors |
| `debug.c/h` | `debug.rs` | Debug utilities |
| `logging.c/h` | `logging.rs` | Log management |
| `init.c/h` | `init.rs` | Project initialization |
| `src/compat/` | (not needed) | C compat → Rust stdlib |

## C Data Structures → Rust Equivalents

| C Structure | Rust Equivalent |
|-------------|-----------------|
| `RB_TREE(tupid_tree)` | `BTreeMap<tupid_t, T>` or custom |
| `RB_TREE(tent_tree)` | `BTreeMap<tupid_t, TupEntry>` |
| `RB_TREE(string_tree)` | `BTreeMap<String, T>` or `HashSet<String>` |
| `TAILQ(tent_list)` | `Vec<TupEntry>` or `VecDeque<TupEntry>` |
| `TAILQ(tupid_list)` | `Vec<tupid_t>` |
| `BSD queue.h macros` | Rust standard collections |
| `BSD tree.h (red-black)` | `BTreeMap` / `BTreeSet` |
| `pthread_mutex_t` | `Mutex<T>` |
| `_Atomic int` | `AtomicI32` / `Arc<AtomicI32>` |
| `char *` strings | `String` / `&str` / `PathBuf` |

## External Dependency Mapping

| C Dependency | Rust Crate | Notes |
|-------------|------------|-------|
| SQLite3 (embedded) | `rusqlite` (bundled) | Same SQLite, Rust bindings |
| PCRE2 | `regex` | Rust-native regex engine |
| Lua 5.4 | `mlua` (vendored) | Full Lua 5.4 support |
| inih | `ini` or custom | Simple INI parser |
| FUSE/FUSE3 | `fuser` | When server support added |
| inotify | `notify` | Cross-platform file watching |
| pthreads | `std::thread` / `rayon` | Rust stdlib + work-stealing |
| BSD queue.h/tree.h | std collections | Not needed in Rust |
