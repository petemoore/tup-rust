# Server, Monitor, and Platform Specification

## Server Architecture

### File Tracking (file.c)
```c
struct file_info {
    pthread_mutex_t lock;
    struct file_entry_head read_list, write_list, unlink_list, var_list;
    struct mapping_head mapping_list;
    struct tent_entries sticky_root, normal_root, group_sticky_root;
    struct tent_entries output_root, exclusion_root;
    int server_fail, open_count, do_unlink;
};

int init_file_info(struct file_info *info, int do_unlink);
int handle_file(enum access_type at, const char *filename, const char *file2, struct file_info *info);
int write_files(FILE *f, tupid_t cmdid, struct file_info *info, int *warnings, ...);
```

### Server Structure
```c
struct server {
    struct file_info finfo;
    int id, exited, signalled, exit_status, exit_sig;
    int output_fd, error_fd;
    int need_namespacing, run_in_bash, streaming_mode;
};

int server_init(enum server_mode mode);  // CONFIG, PARSER, UPDATER
int server_exec(struct server *s, int dfd, const char *cmd, struct tup_env *newenv, struct tup_entry *dtent);
int server_postexec(struct server *s);
```

### FUSE Server
- Mounts at `.tup/mnt`
- Single-threaded, foreground mode
- Intercepts: open, read, write, stat, unlink, rename, mkdir, rmdir, symlink, chmod, chown
- Path parsing extracts job ID from `@tupjob-N` prefix
- Thread-local tracking via thread_tree keyed by job ID

### LD_PRELOAD (stays as C)
- Shared library intercepting syscalls via dlsym(RTLD_NEXT, ...)
- Intercepts: open, fopen, creat, stat, rename, unlink, execve, chdir, realpath
- Writes access_event structs to TUP_DEPFILE
- Fork-safe via pthread_atfork()

## Monitor (inotify.c)

### Love-Trowbridge Algorithm
1. Add watch on directory
2. Set up handlers for CREATE_SUBDIR
3. Read directory contents
4. Recursively watch subdirectories
5. Handle CREATE_SUBDIR for new directories

### Event Processing
- Events queued in event_list with deduplication
- Waits for quiet period before processing
- Database updated in batches

### Tri-Lock System (lock.c)
Three lock files coordinate monitor and updater:
- `.tup/shared` — mutex for object lock access
- `.tup/object` — database serialization
- `.tup/tri` — priority for monitor

## Platform Layer

### Platform Detection (platform.c)
```c
extern const char *tup_platform;  // "linux", "macosx", "win32", etc.
extern const char *tup_arch;      // "x86_64", "arm64", etc.
```

### Configuration (config.c)
```c
int find_tup_dir(void);
const char *get_tup_top(void);
int tup_top_fd(void);
tupid_t get_sub_dir_dt(void);
```

### Initialization (init.c)
```c
int tup_init(int argc, char **argv);
int init_command(int argc, char **argv);
```

### Options (option.c)
```c
int tup_option_get_int(const char *opt);
int tup_option_get_flag(const char *opt);
const char *tup_option_get_string(const char *opt);
```

### Privilege Management (privs.h)
```c
int tup_privileged(void);
int tup_drop_privs(void);
int tup_temporarily_drop_privs(void);
int tup_restore_privs(void);
```

### Main Entry Point (main.c)
Commands: init, version, monitor, stop, upd, scan, refactor, variant, graph, compiledb, options, todo
