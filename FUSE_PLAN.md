# FUSE Implementation Plan

**Goal**: Port the C tup FUSE/server subsystem to Rust for automatic dependency tracking.

**Why this matters**: ~100+ of 989 compat tests require FUSE (or LD_PRELOAD) for file
access interception. Without it, tup relies solely on declared dependencies from Tupfiles.

## C Source Inventory

| C File | LOC | Purpose | Rust File | Ported |
|--------|-----|---------|-----------|--------|
| fuse_fs.c | 1550 | FUSE filesystem callbacks | tup_fuse.rs | ~15% (structs only) |
| fuse_server.c | 955 | Mount/unmount lifecycle, execution | fuse_server.rs + process.rs | ~10% |
| master_fork.c | 886 | Process forking, namespaces | — | 0% |
| file.c | 876 | File access tracking → DB | — | 0% |
| depfile.c | 477 | LD_PRELOAD integration | depfile.rs | ~60% (format only) |
| server.h + file.h | 191 | Data structures | tup_fuse.rs | ~40% |
| **Total** | **4935** | | | **~15%** |

## Work Packages

Each work package is a single PR. If during implementation a package turns out to be
larger than expected, split it into sub-packages and update this plan.

### WP1: File Access Tracking Core (file.c port) — ~200 LOC Rust

Port the core file access recording logic from file.c.

**C functions to port:**
- `handle_open_file()` (34 LOC) — add path to read/write/unlink/var list
- `handle_file()` (22 LOC) — main entry: resolve path, call handle_open_file
- `handle_file_dtent()` (26 LOC) — handle access on a tup_entry
- `handle_rename()` (30 LOC) — track rename events
- `init_file_info()` (21 LOC) — initialize file_info (already partial)
- `cleanup_file_info()` (9 LOC) — cleanup (Rust Drop)
- `check_unlink_list()` (10 LOC) — validate unlinks

**Does NOT include:** write_files() (DB integration, that's WP5).

**Test**: Unit tests for each function matching C behavior.

### WP2: FUSE Filesystem — Basic Operations (~400 LOC Rust)

Implement `fuser::Filesystem` trait for the core passthrough operations.

**NOTE**: `fuser` uses the low-level (inode-based) FUSE API, while C tup uses
the high-level (path-based) API. We need an inode→path mapping layer.

Split into sub-packages due to complexity:

#### WP2a: Inode Management + lookup + getattr (~200 LOC Rust)

- Inode table: inode → real filesystem path mapping
- `lookup()` — resolve name in parent directory, assign inode
- `getattr()` — stat file by inode, handle mappings/tmpdirs
- `context_check()` (39 LOC) — process group security check
- `get_finfo()` / `put_finfo()` (31 LOC) — job lookup with locking
- `tup_fuse_handle_file()` (22 LOC) — record access

#### WP2b: readdir (~150 LOC Rust)

- `readdir()` — list directory by inode, merge real+mapped+tmpdir
- Uses inode table for directory entry resolution

#### WP2c: open + read + release (~150 LOC Rust)

- `open()` — open file, track in read_list, return FH
- `read()` — pread from real/mapped file
- `release()` — close FD, decrement open_count

**Test**: Mount FUSE, read a file through it, verify access recorded.

### WP3: FUSE Filesystem — Write Operations (~300 LOC Rust)

Implement write-side FUSE operations with temporary file mappings.

**C functions to port:**
- `mknod_internal()` (76 LOC) — create file with mapping to .tup/tmp
- `tup_fs_mknod()` (5 LOC) — FUSE mknod callback
- `tup_fs_create()` (20 LOC) — FUSE create callback
- `tup_fs_mkdir()` (47 LOC) — virtual directory creation
- `tup_fs_unlink()` (35 LOC) — record unlink, remove mapping
- `tup_fs_rmdir()` (44 LOC) — remove virtual directory
- `tup_fs_symlink()` (19 LOC) — symlink in virtual dir
- `tup_fs_rename()` (82 LOC) — update mapping realname
- `tup_fs_link()` (7 LOC) — return EPERM (no hard links)
- `add_mapping_internal()` (50 LOC) — create virtual→temp mapping
- `add_mapping()` (12 LOC) — wrapper

**Test**: Create files through FUSE, verify they end up in .tup/tmp.

### WP4: FUSE Mount/Unmount Lifecycle (~200 LOC Rust)

Port the server initialization and teardown from fuse_server.c.

**C functions to port:**
- `server_init()` (175 LOC) — create .tup/mnt, .tup/tmp, start FUSE thread
- `server_quit()` (13 LOC) — unmount, cleanup
- `tup_unmount()` (16 LOC) — platform-specific unmount
- `os_unmount()` (14 LOC per platform) — fusermount / unmount syscall
- `fuse_thread()` (35 LOC) — background thread running fuse_main
- `tup_fuse_fs_init()` (20 LOC) — rlimit setup
- `sighandler()` (13 LOC) — signal handling

**Test**: Mount and unmount FUSE, verify .tup/mnt exists during mount.

### WP5: File Access → DB Integration (~250 LOC Rust)

Connect the file access tracking to the tup database.

**C functions to port:**
- `write_files()` (82 LOC) — main dependency writer: reads file_info lists,
  creates normal/sticky links in DB, handles ghost creation
- `add_config_files()` (8 LOC) — add config dependencies
- `add_parser_files()` (9 LOC) — add parser file dependencies
- `process_depfile()` (74 LOC) — parse LD_PRELOAD depfile, call write_files

**Dependencies**: Requires WP1 (file access tracking).

**Test**: Execute a command through FUSE, verify DB has correct dependency links.

### WP6: Command Execution with FUSE (~200 LOC Rust)

Wire FUSE into the command execution path in the updater.

**C functions to port:**
- `server_exec()` (17 LOC) — add FUSE group, exec, remove group
- `exec_internal()` (102 LOC) — core exec: wait for open_count, collect output
- `virt_tup_open()` (27 LOC) — open virtual job directory
- `virt_tup_close()` (12 LOC) — close virtual job directory
- `finfo_wait_open_count()` (30 LOC) — wait for all FDs to close

**Dependencies**: Requires WP2-WP5.

**Test**: Full `tup upd` with FUSE, verify dependency tracking works end-to-end.

### WP7: Master Fork Process (~400 LOC Rust)

Port the process supervision from master_fork.c.

**C functions to port:**
- `server_pre_init()` (112 LOC) — fork master, setup namespaces
- `server_post_exit()` (32 LOC) — wait for children
- `master_fork_loop()` (237 LOC) — main loop: receive exec messages, fork
- `master_fork_exec()` (44 LOC) — send exec message via socket
- `child_waiter()` (55 LOC) — wait thread for child processes
- `write_all()` / `read_all_internal()` (31 LOC) — IPC helpers

**Note**: This is optional for initial functionality. WP6 can use direct
fork+exec instead of the master_fork model. Master fork adds namespace
isolation (full_deps mode).

### WP8: Linux Namespace Support (~150 LOC Rust)

Port the namespace setup from master_fork.c (Linux only).

**C functions to port:**
- `deny_setgroups()` (26 LOC) — write to /proc/PID/setgroups
- `update_map()` (25 LOC) — write uid/gid maps
- Namespace flags in `server_pre_init()` — CLONE_NEWUSER, CLONE_NEWNS

**Note**: macOS doesn't use namespaces; it relies on FUSE path isolation.

### WP9: Privilege Management (~100 LOC Rust)

Port the privilege dropping from fuse_server.c.

**C functions to port:**
- `tup_privileged()` (6 LOC)
- `tup_drop_privs()` (23 LOC)
- `tup_temporarily_drop_privs()` (17 LOC)
- `tup_restore_privs()` (14 LOC)

### WP10: Parser Mode FUSE (~100 LOC Rust)

Port the parser-mode FUSE support for run-scripts.

**C functions to port:**
- `server_parser_start()` (19 LOC)
- `server_parser_stop()` (24 LOC)
- `tup_fuse_server_get_dir_entries()` (32 LOC)
- `readdir_parser()` (12 LOC)

**Dependencies**: Requires WP2, WP4.

### WP11: Additional FUSE Operations (~150 LOC Rust)

Port remaining FUSE operations.

**C functions to port:**
- `tup_fs_access()` (59 LOC)
- `tup_fs_readlink()` (43 LOC)
- `tup_fs_chmod()` / `tup_fs_chown()` (~20 LOC)
- `tup_fs_truncate()` (25 LOC)
- `tup_fs_utimens()` (20 LOC)
- `tup_fs_flush()` (7 LOC)
- `tup_fs_statfs()` (10 LOC)
- `get_virtual_var()` (25 LOC) — @tup@ variable handling

## Execution Order

**Phase 1 — Core (WP1 → WP2 → WP3)**: File tracking + basic FUSE ops.
Get FUSE mounted and intercepting reads/writes.

**Phase 2 — Integration (WP4 → WP5 → WP6)**: Mount lifecycle + DB writes + updater.
Run `tup upd` with FUSE and see dependencies recorded.

**Phase 3 — Hardening (WP7 → WP8 → WP9 → WP10 → WP11)**: Process model +
namespaces + parser mode + remaining ops.

## Recursive Decomposition Rule

If any WP takes more than ~400 LOC of Rust or touches more than 3 files,
split it before starting. Update this plan with the sub-packages. The plan
is a living document.

## Build Requirements

macOS: `PKG_CONFIG_PATH=/usr/local/lib/pkgconfig` (macFUSE installed)
Linux: `libfuse-dev` or `libfuse3-dev`
Feature flag: `--features fuse`

## Progress

| WP | Description | Status | PR |
|----|-------------|--------|-----|
| WP1 | File access tracking core | DONE | #88 |
| WP2a | Filesystem: lookup, getattr, readdir, open, read | DONE | #89 |
| WP3 | Filesystem: write operations | DONE | #90 |
| WP4 | Mount/unmount lifecycle | DONE | #91 |
| WP5 | File access → DB integration | DONE | #92 |
| WP6 | Wire into command execution | DONE | #93 |
| WP7 | Master fork process | NOT STARTED | — |
| WP8 | Linux namespace support | NOT STARTED | — |
| WP9 | Privilege management | NOT STARTED | — |
| WP10 | Parser mode FUSE | NOT STARTED | — |
| WP11 | Additional FUSE operations | NOT STARTED | — |

## What's Working

The FUSE infrastructure is in place:
- File access tracking (read/write/unlink/var lists, mappings)
- Full Filesystem trait (lookup, getattr, readdir, open, read, write, create, mknod, mkdir, unlink, rmdir, symlink, rename, link)
- Mount/unmount lifecycle with macOS support
- DB integration (write_files connects tracking to dependency links)
- Wired into tup upd execution path

## What's Next

Per-command job registration needs to connect each command's execution
to a FileInfo in the FUSE filesystem, so that file accesses during
execution are recorded to the correct job. This requires modifying
the updater to use FUSE job paths (@tupjob-N) for command execution.
