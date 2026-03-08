# Dependency Tracking Sub-Plan

This plan covers the OS-level dependency tracking features needed for
full parity with the C tup implementation.

## Overview

Tup's key innovation is **automatic dependency detection**: instead of
requiring users to declare every input file, tup intercepts file system
operations during command execution and automatically records which files
were read/written. This is done via three mechanisms:

1. **FUSE filesystem** — mount at `.tup/mnt`, redirect child processes through it
2. **LD_PRELOAD** — inject a shared library that wraps syscalls (Linux)
3. **DLL injection** — Windows equivalent of LD_PRELOAD

Without these, tup relies solely on Tupfile declarations (which is what
tup-rust currently does). With them, tup can detect undeclared dependencies
and warn about them.

## Phases

### Phase A: Dependency Protocol [PR #37a] — DONE
- [x] Define `AccessEvent` wire format
- [x] Depfile writer (binary format: type + len + len2 + path + path2)
- [x] Depfile reader/parser
- [x] FileAccessSummary with categorization and deduplication
- [x] Undeclared read/write detection
- [x] System path filtering (/dev, /proc, /usr, etc.)
- [x] 7 tests

### Phase B: Process Server [PR #37b] — DONE
- [x] `ProcessServer` struct with ServerMode (None, LdPreload, Fuse)
- [x] Command execution with environment setup
- [x] TUP_DEPFILE environment variable for LD_PRELOAD mode
- [x] LD_PRELOAD environment setup
- [x] Post-execution depfile reading
- [x] exec_and_verify() for dependency verification
- [x] 7 tests

### Phase C: File Monitor Daemon [PR #37c] — DONE
- [x] Monitor using `notify` crate (cross-platform: inotify/FSEvents/kqueue)
- [x] Recursive directory watching
- [x] Event deduplication
- [x] Filtering (.tup, .git, hidden files)
- [x] watch_for(duration) for timed monitoring
- [x] 5 tests

### Phase D: LD_PRELOAD Shared Library [PR #39]
- [ ] C source for ldpreload.so (port from original)
- [ ] Build via `cc` crate in build.rs
- [ ] Syscall wrapping: open, fopen, stat, rename, unlink, execve, chdir
- [ ] CWD tracking for relative path resolution
- [ ] Depfile output (ACCESS_EVENT_MAX_SIZE protocol)
- [ ] Fork safety (pthread_atfork)
- [ ] ccache path filtering
- [ ] Tests: inject into child, verify file accesses recorded

### Phase E: FUSE Server — Linux [PR #40]
- [ ] `fuser` crate integration
- [ ] Mount at `.tup/mnt`
- [ ] Path parsing: extract job ID from `@tupjob-N` prefix
- [ ] File operations: getattr, readdir, read, write, open, release, unlink, rename
- [ ] Thread-local file_info tracking per job
- [ ] Namespace isolation (CLONE_NEWUSER) for unprivileged FUSE
- [ ] Master fork process model
- [ ] Tests: FUSE mount, intercept file operations

### Phase F: FUSE Server — macOS [PR #41]
- [ ] macFUSE (osxfuse) integration
- [ ] Platform-specific mount/unmount
- [ ] Same operation set as Linux FUSE
- [ ] Tests: macOS FUSE operations

### Phase G: Windows DLL Injection [PR #52]
- [ ] Port dllinject.c to build via cc crate
- [ ] IAT patching for Windows API functions
- [ ] Hot patching for NTDLL functions
- [ ] CreateFile/DeleteFile/MoveFile/CopyFile interception
- [ ] CreateProcess injection propagation
- [ ] Tests: Windows file access tracking

### Phase H: Full Integration [PR #53]
- [ ] Wire FUSE/LD_PRELOAD into updater execution path
- [ ] Server mode selection (fuse, ldpreload, none)
- [ ] `--no-fuse` / `--server` flags
- [ ] Dependency verification: compare actual reads vs declared inputs
- [ ] Undeclared dependency warnings
- [ ] Extra output detection
- [ ] Run C test suite against Rust binary
- [ ] Performance benchmarks vs C version

## Priority Order

1. Phase A+B (dependency protocol + process server) — enables dep tracking without FUSE
2. Phase C (file monitor) — enables `tup monitor` for fast rebuilds
3. Phase D (LD_PRELOAD) — most common dep tracking method on Linux
4. Phase E (Linux FUSE) — full dep tracking on Linux
5. Phase F (macOS FUSE) — full dep tracking on macOS
6. Phase G (Windows DLL) — Windows support
7. Phase H (integration) — tie it all together
