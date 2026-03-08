# Database Layer Specification (db.c, db.h, db_types.h)

## 1. Data Types and Enums

### Node Types (enum TUP_NODE_TYPE)
```
TUP_NODE_FILE = 0           // Source file
TUP_NODE_CMD = 1            // Command node
TUP_NODE_DIR = 2            // Directory
TUP_NODE_VAR = 3            // @-variable node
TUP_NODE_GENERATED = 4      // Generated/output file
TUP_NODE_GHOST = 5          // Placeholder for deleted/missing node
TUP_NODE_GROUP = 6          // Output group
TUP_NODE_GENERATED_DIR = 7  // Generated directory
TUP_NODE_ROOT = 8           // Virtual root node
```

### Link Types (enum TUP_LINK_TYPE)
```
TUP_LINK_NORMAL = 1  // Regular dependency link
TUP_LINK_STICKY = 2  // Sticky link (always in input list)
TUP_LINK_GROUP = 3   // Group link (output to group via cmdid)
```

### Flag Types (enum TUP_FLAGS_TYPE)
```
TUP_FLAGS_NONE = 0
TUP_FLAGS_MODIFY = 1
TUP_FLAGS_CREATE = 2
TUP_FLAGS_CONFIG = 4
TUP_FLAGS_VARIANT = 8
TUP_FLAGS_TRANSIENT = 16
```

### Special Constants
```
DOT_DT = 1              // Root directory tupid
DB_VERSION = 19          // Current schema version
PARSER_VERSION = 16      // Tupfile parser version
TUP_DIR = ".tup"
TUP_DB_FILE = ".tup/db"
```

### Virtual Directories (created under root DOT_DT)
```
"$" → env_dt()        // Environment variables
"/" → slash_dt()       // Full filesystem dependency tracking
"^" → exclusion_dt()   // Exclusion node tracking
```

## 2. SQLite Schema

### Table: node
```sql
CREATE TABLE node (
    id INTEGER PRIMARY KEY NOT NULL,
    dir INTEGER NOT NULL,
    type INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    mtime_ns INTEGER NOT NULL,
    srcid INTEGER NOT NULL,
    name VARCHAR(4096),
    display VARCHAR(4096),
    flags VARCHAR(256),
    UNIQUE(dir, name)
);
CREATE INDEX srcid_index ON node(srcid);
```

### Table: normal_link
```sql
CREATE TABLE normal_link (
    from_id INTEGER,
    to_id INTEGER,
    UNIQUE(from_id, to_id)
);
CREATE INDEX normal_index2 ON normal_link(to_id);
```

### Table: sticky_link
```sql
CREATE TABLE sticky_link (
    from_id INTEGER,
    to_id INTEGER,
    UNIQUE(from_id, to_id)
);
CREATE INDEX sticky_index2 ON sticky_link(to_id);
```

### Table: group_link
```sql
CREATE TABLE group_link (
    from_id INTEGER,
    to_id INTEGER,
    cmdid INTEGER,
    UNIQUE(from_id, to_id, cmdid)
);
CREATE INDEX group_index2 ON group_link(cmdid);
```

### Table: var
```sql
CREATE TABLE var (
    id INTEGER PRIMARY KEY NOT NULL,
    value VARCHAR(4096)
);
```

### Table: config
```sql
CREATE TABLE config (
    lval VARCHAR(256) UNIQUE,
    rval VARCHAR(256)
);
```

### Flag List Tables
```sql
CREATE TABLE config_list (id INTEGER PRIMARY KEY NOT NULL);
CREATE TABLE create_list (id INTEGER PRIMARY KEY NOT NULL);
CREATE TABLE modify_list (id INTEGER PRIMARY KEY NOT NULL);
CREATE TABLE variant_list (id INTEGER PRIMARY KEY NOT NULL);
CREATE TABLE transient_list (id INTEGER PRIMARY KEY NOT NULL);
```

## 3. Public Function Signatures

### Database Lifecycle
```c
int tup_db_open(void);
int tup_db_close(void);
int tup_db_create(int db_sync, int memory_db);
```

### Transaction Management
```c
int tup_db_begin(void);
int tup_db_commit(void);    // Also calls reclaim_ghosts()
int tup_db_rollback(void);
int tup_db_changes(void);
int tup_db_check_flags(int flags);
```

### Node Creation
```c
struct tup_entry *tup_db_create_node(struct tup_entry *dtent, const char *name, enum TUP_NODE_TYPE type);
struct tup_entry *tup_db_create_node_srcid(struct tup_entry *dtent, const char *name, enum TUP_NODE_TYPE type, tupid_t srcid, int *node_changed);
struct tup_entry *tup_db_create_node_part(struct tup_entry *dtent, const char *name, int len, enum TUP_NODE_TYPE type, tupid_t srcid, int *node_changed);
struct tup_entry *tup_db_create_node_part_display(struct tup_entry *dtent, const char *name, int namelen, const char *display, int displaylen, const char *flags, int flagslen, enum TUP_NODE_TYPE type, tupid_t srcid, int *node_changed);
int tup_db_node_insert_tent(struct tup_entry *dtent, const char *name, int namelen, enum TUP_NODE_TYPE type, struct timespec mtime, tupid_t srcid, struct tup_entry **entry);
int tup_db_fill_tup_entry(tupid_t tupid, struct tup_entry **dest);
int tup_db_select_tent(struct tup_entry *dtent, const char *name, struct tup_entry **entry);
int tup_db_select_tent_part(struct tup_entry *dtent, const char *name, int len, struct tup_entry **entry);
```

### Node Querying
```c
int tup_db_select_node_by_flags(int (*callback)(void *, struct tup_entry *), void *arg, int flags);
int tup_db_select_node_dir(int (*callback)(void *, struct tup_entry *), void *arg, tupid_t dt);
int tup_db_select_node_dir_glob(int (*callback)(void *, struct tup_entry *), void *arg, struct tup_entry *dtent, const char *glob, int len, struct tent_entries *delete_root, int include_directories);
```

### Node Modification
```c
int tup_db_set_name(tupid_t tupid, const char *new_name, tupid_t new_dt);
int tup_db_set_display(struct tup_entry *tent, const char *display, int displaylen);
int tup_db_set_flags(struct tup_entry *tent, const char *flags, int flagslen);
int tup_db_set_type(struct tup_entry *tent, enum TUP_NODE_TYPE type);
int tup_db_set_mtime(struct tup_entry *tent, struct timespec mtime);
int tup_db_set_srcid(struct tup_entry *tent, tupid_t srcid);
int tup_db_change_node(tupid_t tupid, const char *new_name, struct tup_entry *new_dtent);
```

### Node Deletion
```c
int tup_db_delete_node(tupid_t tupid);
int tup_db_delete_dir(tupid_t dt, int force);
int tup_db_delete_variant(struct tup_entry *tent, void *arg, int (*callback)(void *, struct tup_entry *));
```

### Link Operations
```c
int tup_db_create_link(tupid_t a, tupid_t b, int style);
int tup_db_create_unique_link(struct tup_entry *a, struct tup_entry *b);
int tup_db_link_exists(tupid_t a, tupid_t b, int style, int *exists);
int tup_db_get_incoming_link(struct tup_entry *tent, struct tup_entry **incoming);
int tup_db_delete_links(tupid_t tupid);
```

### Input/Output Management
```c
int tup_db_get_outputs(tupid_t cmdid, struct tent_entries *output_root, struct tent_entries *exclusion_root, struct tup_entry **group);
int tup_db_get_inputs(tupid_t cmdid, struct tent_entries *sticky_root, struct tent_entries *normal_root, struct tent_entries *group_sticky_root);
int tup_db_write_outputs(FILE *f, struct tup_entry *cmdtent, struct tent_entries *root, struct tent_entries *exclusion_root, struct tup_entry *group, struct tup_entry **old_group, int refactoring, int command_modified);
int tup_db_write_inputs(FILE *f, tupid_t cmdid, struct tent_entries *input_root, struct tent_entries *env_root, struct tup_entry *group, struct tup_entry *old_group, int refactoring);
```

### Flag List Operations
```c
int tup_db_add_config_list(tupid_t tupid);
int tup_db_add_create_list(tupid_t tupid);
int tup_db_add_modify_list(tupid_t tupid);
int tup_db_add_variant_list(tupid_t tupid);
int tup_db_add_transient_list(tupid_t tupid);
int tup_db_in_create_list(tupid_t tupid);    // Returns 0/1/-1
int tup_db_in_modify_list(tupid_t tupid);
int tup_db_unflag_config(tupid_t tupid);
int tup_db_unflag_create(tupid_t tupid);
int tup_db_unflag_modify(tupid_t tupid);
int tup_db_get_node_flags(tupid_t tupid);    // Returns bitmask
```

### Configuration
```c
int tup_db_config_set_int(const char *lval, int x);
int tup_db_config_get_int(const char *lval, int def, int *result);
```

### Variables
```c
int tup_db_set_var(tupid_t tupid, const char *value);
struct tup_entry *tup_db_get_var(struct variant *variant, const char *var, int varlen, struct estring *e);
int tup_db_read_vars(struct tup_entry *tent, struct tup_entry *vartent, const char *vardict_file);
```

### Environment
```c
int tup_db_check_env(int environ_check);
int tup_db_findenv(const char *var, int varlen, struct var_entry **ret);
int tup_db_get_environ(struct tent_entries *root, struct tent_entries *normal_root, struct tup_env *te);
tupid_t env_dt(void);
tupid_t slash_dt(void);
tupid_t exclusion_dt(void);
int is_virtual_tent(struct tup_entry *tent);
```

## 4. Key Behaviors and Invariants

### Ghost Node Behavior
- When a node is deleted but still referenced, it becomes TUP_NODE_GHOST
- Ghosts are reclaimed at transaction commit when no references remain
- Ghost reclamation may require multiple passes for nested ghosts

### Source ID (srcid) Semantics
- For generated files: srcid points to source directory
- For normal directories: srcid = -1
- Updates trigger re-parsing of source directory's Tupfile

### Link Semantics
- **Normal**: Standard input→output dependencies ("file A was read by command B")
- **Sticky**: Permanent, always in command inputs (from Tupfile declarations)
- **Group**: Associates outputs with output groups, unique per (output, group, command)

### Flag List Semantics
- Node can be in multiple lists simultaneously
- **create_list**: Directory Tupfiles need parsing
- **modify_list**: Commands need re-execution
- **config_list**: Config-related nodes
- **variant_list**: Variant-specific nodes
- **transient_list**: Outputs deleted after use
- Add operations use INSERT OR IGNORE (idempotent)

### Node Creation Semantics
1. If ghost with same name/dir exists → upgrade it
2. If non-ghost exists with different type → delete old, create new
3. If non-ghost with same type → update metadata
4. New commands go directly to modify_list
5. New directories go to create_list

### Transaction Semantics
- Must wrap all operations in begin/commit/rollback
- Cannot nest transactions
- At commit, ghosts are automatically reclaimed
- Sticky link cache invalidated globally when deleted

## 5. Global State
```c
static sqlite3 *tup_db;
static sqlite3_stmt *stmts[DB_NUM_STATEMENTS];
static struct tent_entries ghost_root;
static int tup_db_var_changed;
static int sql_debug;
static struct vardb envdb;
static int transaction;
static tupid_t local_env_dt;
static tupid_t local_exclusion_dt;
static tupid_t local_slash_dt;
static int sticky_count;
```

## 6. Error Handling
- Returns: 0 = success, -1 = error, 1 = status value
- All errors printed to stderr with context
- SQL errors include error message and full statement
- Callbacks return -1 on error, propagated immediately
