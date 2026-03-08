mod harness;

use harness::TupTestEnv;

// ============================================================
// t0000: Initialization tests
// ============================================================

#[test]
fn t0000_init_creates_tup_dir() {
    let env = TupTestEnv::new();
    env.check_exist(".tup");
    env.check_exist(".tup/db");
}

#[test]
fn t0001_init_twice_warns() {
    let env = TupTestEnv::new();
    let result = env.run_tup(&["init"]);
    // Should warn about existing database
    assert!(!result.success || result.stderr.contains("already exists"));
}

// ============================================================
// t1000: Basic creation tests
// ============================================================

#[test]
fn t1000_basic_rule() {
    let env = TupTestEnv::new();
    env.write_tupfile(": |> echo hello > %o |> output.txt\n");
    let result = env.update();
    result.assert_success();
    env.check_exist("output.txt");
    assert!(env.read_file("output.txt").contains("hello"));
}

#[test]
fn t1001_rule_with_input() {
    let env = TupTestEnv::new();
    env.write_file("input.txt", "world");
    env.write_tupfile(": input.txt |> cp %f %o |> output.txt\n");
    let result = env.update();
    result.assert_success();
    env.check_exist("output.txt");
    assert_eq!(env.read_file("output.txt"), "world");
}

#[test]
fn t1002_multiple_rules() {
    let env = TupTestEnv::new();
    env.write_file("a.txt", "aaa");
    env.write_file("b.txt", "bbb");
    env.write_tupfile(
        ": a.txt |> cp %f %o |> a.out\n\
         : b.txt |> cp %f %o |> b.out\n"
    );
    let result = env.update();
    result.assert_success();
    env.check_exist("a.out");
    env.check_exist("b.out");
    assert_eq!(env.read_file("a.out"), "aaa");
    assert_eq!(env.read_file("b.out"), "bbb");
}

#[test]
fn t1003_version_command() {
    let env = TupTestEnv::new();
    let result = env.run_tup(&["version"]);
    result.assert_success();
    result.assert_stdout_contains("tup-rust");
    result.assert_stdout_contains("Platform:");
}

// ============================================================
// t2000: Tupfile parsing tests
// ============================================================

#[test]
fn t2000_variables() {
    let env = TupTestEnv::new();
    env.write_tupfile(
        "MSG = hello_from_var\n\
         : |> echo $(MSG) > %o |> output.txt\n"
    );
    let result = env.update();
    result.assert_success();
    assert!(env.read_file("output.txt").contains("hello_from_var"));
}

#[test]
fn t2001_variable_append() {
    let env = TupTestEnv::new();
    env.write_tupfile(
        "FLAGS = -Wall\n\
         FLAGS += -O2\n\
         : |> echo $(FLAGS) > %o |> output.txt\n"
    );
    let result = env.update();
    result.assert_success();
    let content = env.read_file("output.txt");
    assert!(content.contains("-Wall"));
    assert!(content.contains("-O2"));
}

#[test]
fn t2002_ifdef_true() {
    let env = TupTestEnv::new();
    env.write_tupfile(
        "ENABLE = yes\n\
         ifdef ENABLE\n\
         : |> echo enabled > %o |> output.txt\n\
         endif\n"
    );
    let result = env.update();
    result.assert_success();
    env.check_exist("output.txt");
}

#[test]
fn t2003_ifdef_false() {
    let env = TupTestEnv::new();
    env.write_tupfile(
        "ifdef MISSING\n\
         : |> echo should_not_run > %o |> output.txt\n\
         endif\n"
    );
    let result = env.update();
    result.assert_success();
    env.check_not_exist("output.txt");
}

#[test]
fn t2004_ifdef_else() {
    let env = TupTestEnv::new();
    env.write_tupfile(
        "ifdef MISSING\n\
         : |> echo wrong > %o |> output.txt\n\
         else\n\
         : |> echo correct > %o |> output.txt\n\
         endif\n"
    );
    let result = env.update();
    result.assert_success();
    assert!(env.read_file("output.txt").contains("correct"));
}

#[test]
fn t2005_ifeq() {
    let env = TupTestEnv::new();
    env.write_tupfile(
        "CC = gcc\n\
         ifeq ($(CC), gcc)\n\
         : |> echo is_gcc > %o |> output.txt\n\
         endif\n"
    );
    let result = env.update();
    result.assert_success();
    assert!(env.read_file("output.txt").contains("is_gcc"));
}

// ============================================================
// t2050: Glob and foreach tests
// ============================================================

#[test]
fn t2050_foreach_glob() {
    let env = TupTestEnv::new();
    env.write_file("a.txt", "aaa");
    env.write_file("b.txt", "bbb");
    env.write_file("c.dat", "ccc");
    env.write_tupfile(": foreach *.txt |> cp %f %o |> %B.out\n");
    let result = env.update();
    result.assert_success();
    env.check_exist("a.out");
    env.check_exist("b.out");
    env.check_not_exist("c.out"); // .dat not matched
}

#[test]
fn t2051_glob_all_inputs() {
    let env = TupTestEnv::new();
    env.write_file("x.c", "");
    env.write_file("y.c", "");
    env.write_tupfile(": *.c |> echo %f > %o |> files.txt\n");
    let result = env.update();
    result.assert_success();
    let content = env.read_file("files.txt");
    assert!(content.contains("x.c"));
    assert!(content.contains("y.c"));
}

// ============================================================
// t2100: Bang macros
// ============================================================

#[test]
fn t2100_bang_macro() {
    let env = TupTestEnv::new();
    env.write_file("input.txt", "data");
    env.write_tupfile(
        "!cp = |> cp %f %o |> %B.out\n\
         : input.txt |> !cp |>\n"
    );
    let result = env.update();
    result.assert_success();
    env.check_exist("input.out");
}

// ============================================================
// t3000: Multi-directory tests
// ============================================================

#[test]
fn t3000_subdirectory_tupfile() {
    let env = TupTestEnv::new();
    env.write_file("sub/input.txt", "hello");
    env.write_tupfile_in("sub", ": input.txt |> cp %f %o |> output.txt\n");
    let result = env.update();
    result.assert_success();
    env.check_exist("sub/output.txt");
    assert_eq!(env.read_file("sub/output.txt"), "hello");
}

#[test]
fn t3001_multiple_directories() {
    let env = TupTestEnv::new();
    env.write_file("a/in.txt", "aaa");
    env.write_file("b/in.txt", "bbb");
    env.write_tupfile_in("a", ": in.txt |> cp %f %o |> out.txt\n");
    env.write_tupfile_in("b", ": in.txt |> cp %f %o |> out.txt\n");
    let result = env.update();
    result.assert_success();
    env.check_exist("a/out.txt");
    env.check_exist("b/out.txt");
}

// ============================================================
// t4000: Error handling tests
// ============================================================

#[test]
fn t4000_command_failure() {
    let env = TupTestEnv::new();
    env.write_tupfile(": |> false |>\n");
    let result = env.update();
    result.assert_failure();
}

#[test]
fn t4001_keep_going() {
    let env = TupTestEnv::new();
    env.write_tupfile(
        ": |> echo first > first.txt |> first.txt\n\
         : |> false |>\n\
         : |> echo third > third.txt |> third.txt\n"
    );
    let _result = env.update_keep_going();
    // Should still create first.txt and third.txt
    env.check_exist("first.txt");
    env.check_exist("third.txt");
}

// ============================================================
// t5000: Parse command tests
// ============================================================

#[test]
fn t5000_parse_shows_rules() {
    let env = TupTestEnv::new();
    env.write_tupfile(": |> echo hello |> out.txt\n");
    let result = env.parse();
    result.assert_success();
    result.assert_stdout_contains("echo hello");
    result.assert_stdout_contains("out.txt");
}

#[test]
fn t5001_parse_multiple_tupfiles() {
    let env = TupTestEnv::new();
    env.write_tupfile(": |> echo root |> root.txt\n");
    env.write_tupfile_in("sub", ": |> echo sub |> sub.txt\n");
    let result = env.parse();
    result.assert_success();
    result.assert_stdout_contains("2 Tupfile(s)");
    result.assert_stdout_contains("2 rule(s)");
}

// ============================================================
// t6000: Graph command tests
// ============================================================

#[test]
fn t6000_graph_output() {
    let env = TupTestEnv::new();
    env.write_file("in.c", "");
    env.write_tupfile(": in.c |> gcc -c %f -o %o |> in.o\n");
    let result = env.graph();
    result.assert_success();
    result.assert_stdout_contains("digraph G");
    result.assert_stdout_contains("gcc");
    result.assert_stdout_contains("in.c");
    result.assert_stdout_contains("in.o");
}

// ============================================================
// t7000: Parallel execution tests
// ============================================================

#[test]
fn t7000_parallel_build() {
    let env = TupTestEnv::new();
    env.write_file("a.txt", "a");
    env.write_file("b.txt", "b");
    env.write_file("c.txt", "c");
    env.write_tupfile(": foreach *.txt |> cp %f %o |> %B.out\n");
    let result = env.update_parallel(4);
    result.assert_success();
    env.check_exist("a.out");
    env.check_exist("b.out");
    env.check_exist("c.out");
}
