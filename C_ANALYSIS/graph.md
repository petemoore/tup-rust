# Graph and Entry Specification (graph.c, graph.h, entry.c, entry.h)

## 1. Data Structures

### struct node
```c
struct node {
    TAILQ_ENTRY(node) list;
    struct edge_head edges;          // Outgoing edges
    struct edge_head incoming;       // Incoming edges
    struct tupid_tree tnode;         // RB-tree node (tnode.tupid = node ID)
    struct tup_entry *tent;          // Corresponding tup_entry
    struct node_head *active_list;   // Current list (node_list, plist, or removing_list)
    unsigned char state;             // 0=INITIALIZED, 1=PROCESSING, 2=FINISHED, 3=REMOVING
    unsigned char already_used;
    unsigned char expanded;
    unsigned char parsing;
    unsigned char marked;            // For pruning
    unsigned char skip;              // Initially 1, set to 0 if reachable
    unsigned char counted;
    unsigned char transient;         // 0=NONE, 1=PROCESSING, 2=DELETE
};
```

### struct edge
```c
struct edge {
    LIST_ENTRY(edge) list;           // In src->edges
    LIST_ENTRY(edge) destlist;       // In dest->incoming
    struct node *dest;
    struct node *src;
    int style;                       // TUP_LINK_NORMAL/STICKY/GROUP
};
```

### struct graph
```c
struct graph {
    struct node_head node_list;      // Finished nodes
    struct node_head plist;          // Pending expansion
    struct node_head removing_list;
    struct tent_entries transient_root;
    struct node *root;               // Virtual root (tupid=0)
    struct node *cur;                // Current node being processed
    int num_nodes;
    struct tupid_entries node_root;  // RB-tree of all nodes by tupid
    enum TUP_NODE_TYPE count_flags;
    time_t total_mtime;
    struct tent_entries gen_delete_root;
    struct tent_entries save_root;
    struct tent_entries cmd_delete_root;
    struct tent_entries normal_dir_root;
    struct tent_entries parse_gitignore_root;
    int style;
};
```

### struct tup_entry
```c
struct tup_entry {
    struct tupid_tree tnode;         // tnode.tupid = entry ID
    tupid_t dt;                     // Parent directory tupid
    struct tup_entry *parent;       // Resolved from dt
    enum TUP_NODE_TYPE type;
    struct timespec mtime;           // tv_sec=-1 means invalid
    tupid_t srcid;
    struct variant *variant;         // Lazily resolved
    struct string_tree name;         // Name in parent directory
    struct string_entries entries;   // Children (for directories)
    struct tent_entries stickies;
    struct tent_entries group_stickies;
    int retrieved_stickies;
    struct tup_entry *incoming;      // Legacy, unused
    _Atomic int refcount;
    pcre2_code *re;                  // Compiled regex (exclusion entries)
    char *flags;                     // 't' for transient, 'j' for compiledb
    int flagslen;
    char *display;
    int displaylen;
};
```

## 2. Enumerations

### Node States
```
STATE_INITIALIZED = 0
STATE_PROCESSING = 1
STATE_FINISHED = 2
STATE_REMOVING = 3
```

### Transient States
```
TRANSIENT_NONE = 0
TRANSIENT_PROCESSING = 1
TRANSIENT_DELETE = 2
```

### Graph Prune Type
```
GRAPH_PRUNE_GENERATED
GRAPH_PRUNE_ALL
```

## 3. Graph Functions

```c
int create_graph(struct graph *g, enum TUP_NODE_TYPE count_flags);
int destroy_graph(struct graph *g);
struct node *find_node(struct graph *g, tupid_t tupid);
struct node *create_node(struct graph *g, struct tup_entry *tent);
void remove_node(struct graph *g, struct node *n);
int node_insert_tail(struct node_head *head, struct node *n);
int node_insert_head(struct node_head *head, struct node *n);
int node_remove_list(struct node_head *head, struct node *n);
int create_edge(struct node *n1, struct node *n2, int style);
void remove_edge(struct edge *e);
int build_graph_cb(void *arg, struct tup_entry *tent);
int build_graph(struct graph *g);
int add_graph_stickies(struct graph *g);
int graph_empty(struct graph *g);
int nodes_are_connected(struct tup_entry *src, struct tent_entries *valid_root, int *connected);
int prune_graph(struct graph *g, int argc, char **argv, int *num_pruned, enum graph_prune_type gpt, int verbose);
void trim_graph(struct graph *g);
void dump_graph(struct graph *g, FILE *f, int show_dirs, int combine);
void save_graph(FILE *err, struct graph *g, const char *filename);
int group_need_circ_check(void);
int add_group_circ_check(struct tup_entry *tent);
int group_circ_check(void);
```

## 4. Entry Functions

```c
int tup_entry_add(tupid_t tupid, struct tup_entry **dest);
int tup_entry_add_all(tupid_t tupid, tupid_t dt, enum TUP_NODE_TYPE type, struct timespec mtime, tupid_t srcid, const char *name, const char *display, const char *flags, struct tup_entry **dest);
int tup_entry_add_to_dir(struct tup_entry *dtent, tupid_t tupid, const char *name, int len, const char *display, int displaylen, const char *flags, int flagslen, enum TUP_NODE_TYPE type, struct timespec mtime, tupid_t srcid, struct tup_entry **dest);
int tup_entry_rm(tupid_t tupid);
struct tup_entry *tup_entry_find(tupid_t tupid);
struct tup_entry *tup_entry_get(tupid_t tupid);  // Panics if not found
int tup_entry_find_name_in_dir(struct tup_entry *tent, const char *name, int len, struct tup_entry **dest);
int tup_entry_change_name_dt(tupid_t tupid, const char *new_name, tupid_t dt);
int tup_entry_change_display(struct tup_entry *tent, const char *display, int displaylen);
int tup_entry_change_flags(struct tup_entry *tent, const char *flags, int flagslen);
void tup_entry_add_ref(struct tup_entry *tent);
void tup_entry_del_ref(struct tup_entry *tent);
struct variant *tup_entry_variant(struct tup_entry *tent);
int tup_entry_open(struct tup_entry *tent);
int tup_entry_openat(int root_dfd, struct tup_entry *tent);
int tup_entry_resolve_dirs(void);
int tup_entry_clear(void);
int is_transient_tent(struct tup_entry *tent);
int is_compiledb_tent(struct tup_entry *tent);
int tup_entry_add_ghost_tree(struct tent_entries *root, struct tup_entry *tent);
int get_relative_dir(FILE *f, struct estring *e, tupid_t start, tupid_t end);
int exclusion_match(FILE *f, struct tent_entries *exclusion_root, const char *s, struct tup_entry **match);
void print_tup_entry(FILE *f, struct tup_entry *tent);
int snprint_tup_entry(char *dest, int len, struct tup_entry *tent);
```

## 5. Key Algorithms

### Graph Building (build_graph)
1. Process nodes from plist in LIFO order
2. For each INITIALIZED node: set PROCESSING, discover dependencies via DB callbacks, create edges
3. If PROCESSING node re-encountered → circular dependency error
4. Finished nodes move to node_list
5. Attach transient nodes (loop until stable)
6. Add sticky links for CMD nodes

### Cycle Detection
- Early: in make_edge(), if destination is STATE_PROCESSING
- Full: group_circ_check() trims leaves; remaining nodes form cycle

### Pruning (prune_graph)
- mark_nodes() traverses upward (dependencies) and downward (CMD outputs)
- Unmarked nodes are pruned
- Pruned CMDs added to modify_list for next build

### Trimming (trim_graph)
- Repeatedly remove nodes with no incoming OR no outgoing edges
- Remaining nodes form cycles

## 6. Memory Management
- Thread-local memory pools for nodes, edges, entries
- Atomic reference counting on tup_entry (refcount must be 0 at deletion)
- Edges owned by both src->edges and dest->incoming lists
- Graph cleanup removes all nodes and edges

## 7. Critical Invariants
1. Every node has exactly one active_list
2. No STATE_PROCESSING node can be re-encountered (cycle detection)
3. plist must be empty at end of build_graph()
4. tup_entry.refcount == 0 when rm_entry() called
5. n->tent->type determines node role in pruning/sticky operations
6. Root node (tupid=0) is never expanded
