# Utility Module Specifications

## Types

### tupid_t
```c
typedef sqlite3_int64 tupid_t;  // 64-bit integer
```

### access_event
```c
enum access_type { ACCESS_READ, ACCESS_WRITE, ACCESS_RENAME, ACCESS_UNLINK, ACCESS_VAR };
struct access_event { enum access_type at; int len; int len2; };
```

## Utility Modules

### estring (estring.h/c) — Extensible string
```c
struct estring { int len; int maxlen; char *s; };
int estring_init(struct estring *e);           // Default size 4096
int estring_append(struct estring *e, const char *src, int len);
int estring_append_escape(struct estring *e, const char *src, int len, char escape);
```

### fslurp (fslurp.h/c) — Read entire file
```c
struct buf { char *s; int len; };
int fslurp(int fd, struct buf *b);
int fslurp_null(int fd, struct buf *b);
```

### timespan (timespan.h/c) — Timing
```c
struct timespan { struct timeval start; struct timeval end; };
void timespan_start(struct timespan *ts);
void timespan_end(struct timespan *ts);
float timespan_seconds(struct timespan *ts);
```

### colors (colors.h/c) — Terminal colors
```c
void color_init(void);
const char *color_type(enum TUP_NODE_TYPE type);
const char *color_end(void);
const char *color_error_mode(void);
```

### progress (progress.h/c) — Build progress
```c
void tup_show_message(const char *s);
void start_progress(int total, int total_time, int max_jobs);
void show_result(struct tup_entry *tent, int is_error, struct timespan *ts, const char *extra, int always);
void show_progress(int active, enum TUP_NODE_TYPE type);
```

### environ (environ.h/c) — Environment
```c
struct tup_env { char *envblock; int block_size; int num_entries; };
int environ_add_defaults(struct tent_entries *root);
```

### mempool (mempool.h/c) — Thread-local memory pools
```c
struct mempool { struct mementry_head free_list; unsigned int item_size; ... };
void *mempool_alloc(struct mempool *pool);
void mempool_free(struct mempool *pool, void *item);
```

### variant (variant.h/c) — Build variants
```c
struct variant {
    struct tup_entry *tent;
    struct vardb vdb;
    int enabled, root_variant;
    char variant_dir[PATH_MAX];
};
int variant_load(void);
int variant_add(struct tup_entry *tent, int enabled, struct variant **dest);
struct variant *variant_search(tupid_t dt);
```

### if_stmt (if_stmt.h/c) — Conditional processing
```c
struct if_stmt { unsigned char ifness; unsigned char level; };
void if_init(struct if_stmt *ifs);
int if_add(struct if_stmt *ifs, int is_true);
int if_else(struct if_stmt *ifs);
int if_endif(struct if_stmt *ifs);
int if_true(struct if_stmt *ifs);
```

### create_name_file.c — Node creation
```c
int create_name_file(tupid_t dt, const char *file, struct timespec mtime, struct tup_entry **entry);
tupid_t tup_file_mod(tupid_t dt, const char *file, int *modified);
int tup_file_del(tupid_t dt, const char *file, int len, int *modified);
struct tup_entry *get_tent_dt(tupid_t dt, const char *path);
tupid_t find_dir_tupid(const char *dir);
```

### delete_name_file.c — Node deletion
```c
int delete_name_file(tupid_t tupid);
int delete_file(struct tup_entry *tent);
```

### varsed (varsed.h/c) — Variable substitution in files
```c
int varsed(int argc, char **argv);
```

### vardict (vardict.h/c) — Variable dictionary
```c
int tup_vardict_init(void);
const char *tup_config_var(const char *key, int keylen);
```
