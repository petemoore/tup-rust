# Foundational Data Structures

## Tree Structures (BSD RB-tree based)

### tupid_tree (tupid_tree.h/c)
RB-tree keyed by tupid_t.
```c
struct tupid_tree { RB_ENTRY(tupid_tree) linkage; tupid_t tupid; };
struct tupid_entries { struct tupid_tree *rbh_root; };

struct tupid_tree *tupid_tree_search(struct tupid_entries *root, tupid_t tupid);
int tupid_tree_insert(struct tupid_entries *root, struct tupid_tree *data);
int tupid_tree_add(struct tupid_entries *root, tupid_t tupid);
int tupid_tree_add_dup(struct tupid_entries *root, tupid_t tupid);  // silent on dup
void tupid_tree_remove(struct tupid_entries *root, tupid_t tupid);
void free_tupid_tree(struct tupid_entries *root);
```
**Rust equivalent:** `BTreeMap<i64, T>` or `BTreeSet<i64>`

### tent_tree (tent_tree.h/c)
RB-tree keyed by tup_entry (compared by tupid).
```c
struct tent_tree { RB_ENTRY(tent_tree) linkage; struct tup_entry *tent; };
struct tent_entries { struct tent_tree *rbh_root; int count; };

void tent_tree_init(struct tent_entries *root);
int tent_tree_add(struct tent_entries *root, struct tup_entry *tent);
int tent_tree_add_dup(struct tent_entries *root, struct tup_entry *tent);
struct tent_tree *tent_tree_search(struct tent_entries *root, struct tup_entry *tent);
int tent_tree_copy(struct tent_entries *dest, struct tent_entries *src);
void tent_tree_remove(struct tent_entries *root, struct tup_entry *tent);
void free_tent_tree(struct tent_entries *root);
```
**Rust equivalent:** `BTreeMap<i64, Arc<TupEntry>>` or `BTreeSet<TupId>`

### string_tree (string_tree.h/c)
RB-tree keyed by string name.
```c
struct string_tree { RB_ENTRY(string_tree) linkage; char *s; int len; };
struct string_entries { struct string_tree *rbh_root; };

int string_tree_insert(struct string_entries *root, struct string_tree *st);
struct string_tree *string_tree_search(struct string_entries *root, const char *s, int len);
void free_string_tree(struct string_entries *root);
```
**Rust equivalent:** `BTreeMap<String, T>` or `HashSet<String>`

### thread_tree (thread_tree.h/c)
RB-tree of thread IDs with mutex+condvar.
```c
struct thread_tree { RB_ENTRY(thread_tree) linkage; int id; };
struct thread_root { struct thread_entries root; pthread_mutex_t lock; pthread_cond_t cond; };

struct thread_tree *thread_tree_search(struct thread_root *troot, int id);
int thread_tree_insert(struct thread_root *troot, struct thread_tree *data);
void thread_tree_rm(struct thread_root *troot, struct thread_tree *data);
```
**Rust equivalent:** `Arc<Mutex<BTreeMap<i32, T>>>` with `Condvar`

## List Structures (BSD TAILQ based)

### tupid_list (tupid_list.h/c)
```c
struct tupid_list { TAILQ_ENTRY(tupid_list) list; tupid_t tupid; };
int tupid_list_add_tail(struct tupid_list_head *head, tupid_t tupid);
void free_tupid_list(struct tupid_list_head *head);
```
**Rust equivalent:** `Vec<i64>`

### tent_list (tent_list.h/c)
Reference-counted list of tup_entry pointers.
```c
struct tent_list { TAILQ_ENTRY(tent_list) list; struct tup_entry *tent; };
int tent_list_add_head(struct tent_list_head *head, struct tup_entry *tent);
int tent_list_add_tail(struct tent_list_head *head, struct tup_entry *tent);
void free_tent_list(struct tent_list_head *head);
```
Calls tup_entry_add_ref() on insert, tup_entry_del_ref() on delete.
**Rust equivalent:** `Vec<Arc<TupEntry>>`

## Composite Structures

### pel_group (pel_group.h/c)
Path element grouping. Splits path into segments.
```c
struct path_element { TAILQ_ENTRY(path_element) list; const char *path; int len; };
struct pel_group { struct path_element_head path_list; int pg_flags; int num_elements; };

#define PG_HIDDEN 1
#define PG_OUTSIDE_TUP 2
#define PG_ROOT 4
#define PG_GROUP 8

int get_path_elements(const char *dir, struct pel_group *pg);
```
Ignored paths: ".", "..", ".tup", ".git", ".hg", ".bzr", ".svn", ".ccache"

### bin (bin.h/c)
Output bins for collecting generated files.
```c
struct bin_entry { TAILQ_ENTRY(bin_entry) list; char *path; int len; struct tup_entry *tent; };
struct bin { LIST_ENTRY(bin) list; char *name; struct bin_entry_head entries; };
struct bin *bin_add(const char *name, struct bin_head *head);
struct bin *bin_find(const char *name, struct bin_head *head);
int bin_add_entry(struct bin *b, const char *path, int len, struct tup_entry *tent);
```

### dircache (dircache.h/c)
Bidirectional mapping: watch descriptor ↔ directory tupid.
```c
struct dircache { struct tupid_tree wd_node; struct tupid_tree dt_node; };
struct dircache_root { struct tupid_entries wd_root; struct tupid_entries dt_root; };
void dircache_add(struct dircache_root *droot, int wd, tupid_t dt);
struct dircache *dircache_lookup_wd(struct dircache_root *droot, int wd);
struct dircache *dircache_lookup_dt(struct dircache_root *droot, tupid_t dt);
```

### vardb (vardb.h/c)
Variable database using string_tree.
```c
struct vardb { struct string_entries root; int count; };
struct var_entry { struct string_tree var; char *value; int vallen; struct tup_entry *tent; };
int vardb_set(struct vardb *v, const char *var, const char *value, struct tup_entry *tent);
struct var_entry *vardb_get(struct vardb *v, const char *var, int varlen);
int vardb_append(struct vardb *v, const char *var, const char *value);
int vardb_compare(struct vardb *vdba, struct vardb *vdbb, ...callbacks...);
```

## Memory Patterns
- Direct malloc: tupid_tree, var_entry, string copies, bins
- Thread-local mempool: tent_tree, tent_list, tupid_list, path_element
- Reference counting: tup_entry (via tent_list/tent_tree)
- Container pattern: vardb embeds string_tree, dircache embeds two tupid_trees

## Error Handling
- Most return -1 on failure, 0 on success
- Insert returns NULL on success, non-NULL on duplicate
- Search returns pointer on found, NULL on not found
