# Parser and Updater Specification (parser.c, luaparser.c, updater.c)

## Key Data Structures

### struct tupfile
State for parsing a single Tupfile.
```c
struct tupfile {
    struct tup_entry *tent;           // Directory containing Tupfile
    struct variant *variant;          // Variant context
    struct tup_entry *curtent;        // Current directory during parsing
    struct tup_entry *srctent;        // Source directory (variants)
    int cur_dfd, root_fd;            // File descriptors
    int refactoring;
    struct graph *g;                  // Dependency graph
    struct vardb node_db;             // &-variables
    struct bin_head bin_list;          // Output bins
    struct tupid_entries cmd_root;     // Command IDs
    struct tent_entries env_root;      // Environment variables
    struct string_entries bang_root;   // !-macros
    struct tent_entries input_root;    // Input dependencies
    FILE *f;                          // Output stream
    struct parser_server *ps;
    char ign;                         // Generate .gitignore
    struct lua_State *ls;
    int luaerror;
    int use_server, full_deps;
};
```

### struct rule
```c
struct rule {
    int foreach;
    struct bin *bin;
    const char *command;
    char *extra_command;
    int command_len;
    struct name_list inputs;
    struct name_list order_only_inputs;
    struct path_list_head outputs;
    struct path_list_head extra_outputs;
    int empty_input;
    int line_number;
};
```

### struct name_list_entry
```c
struct name_list_entry {
    char *path, *base;
    int len, extlesslen, baselen, extlessbaselen;
    int glob[MAX_GLOBS*2];           // (start, length) pairs
    int globcnt;
    int orderid;
    struct tup_entry *tent;
};
```

### struct worker_thread (updater)
```c
struct worker_thread {
    pthread_t pid;
    struct graph *g;
    worker_function fn;
    pthread_mutex_t lock;
    pthread_cond_t cond;
    struct node *n;                   // Node to process
    struct node *retn;               // Result node
    int rc;                          // Return code
    int quit;
};
```

## Parser Functions
```c
int parse(struct node *n, struct graph *g, struct timespan *ts, int refactoring, int use_server, int full_deps);
char *eval(struct tupfile *tf, const char *string, int allow_nodes);
void init_rule(struct rule *r);
int execute_rule(struct tupfile *tf, struct rule *r, struct name_list *output_nl);
int parser_include_file(struct tupfile *tf, const char *file);
int parser_include_rules(struct tupfile *tf, const char *tuprules);
int export(struct tupfile *tf, const char *cmdline);
int import(struct tupfile *tf, const char *cmdline, const char **retvar, const char **retval);
```

## Tupfile Syntax

### Rules
```
: INPUT |> COMMAND |> OUTPUT [| EXTRA_OUTPUTS]
: foreach INPUT |> COMMAND |> OUTPUT
: !macro INPUT |> |> OUTPUT
```

### Variables
```
VAR = value           # Set
VAR += value          # Append
$(VAR)                # Expand
@(CONFIG_VAR)         # Config variable (dependency tracked)
&(node_var)           # Node variable
$(TUP_CWD)            # Relative path
$(TUP_VARIANTDIR)     # Variant directory
```

### Percent Substitutions
```
%f  - all input paths         %b  - input basenames
%B  - basenames no extension  %o  - output paths
%O  - single output no ext    %d  - directory name
%e  - extension (foreach)     %g  - glob match
%i  - order-only inputs       %<group> - group expansion
%Nt - node variable           %'f / %"f - quoted versions
```

### Directives
```
include FILE              include_rules
preload DIR               run SCRIPT
export VAR                import VAR[=default]
ifdef/ifndef/ifeq/ifneq   else / endif
.gitignore                error MSG
```

### Bang Macros
```
!name = |> COMMAND |> OUTPUT
!name.EXT = |> COMMAND |> OUTPUT    # Extension-specific
```

## Lua Integration
```lua
tup.definerule{inputs={...}, outputs={...}, command="..."}
tup.glob(pattern)           tup.getconfig(name)
tup.getcwd()                tup.include(file)
tup.export(var)             tup.import(var)
tup.nodevariable(path)      tup.creategitignore()
```

## Updater
```c
int updater(int argc, char **argv, int phase);
int generate(int argc, char **argv);
int todo(int argc, char **argv);
```

### Update Phases
1. **CONFIG**: Process tup.config changes, update variables
2. **CREATE**: Create directories, parse all Tupfiles
3. **UPDATE**: Execute commands, update mtimes

### Parallel Execution (execute_graph)
- Thread pool with N workers (configurable via num_jobs)
- Work queue: dequeue nodes with no unfinished dependencies
- Workers signal completion via condition variable
- Main thread processes results, updates dependents
- Keep-going mode continues after failures
