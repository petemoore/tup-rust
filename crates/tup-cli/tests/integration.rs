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

// ============================================================
// t1100: Percent substitution tests
// ============================================================

#[test]
fn t1100_percent_f_single() {
    let env = TupTestEnv::new();
    env.write_file("src.txt", "content");
    env.write_tupfile(": src.txt |> echo %f > %o |> out.txt\n");
    let result = env.update();
    result.assert_success();
    assert!(env.read_file("out.txt").contains("src.txt"));
}

#[test]
fn t1101_percent_f_multiple() {
    let env = TupTestEnv::new();
    env.write_file("a.c", "");
    env.write_file("b.c", "");
    env.write_tupfile(": a.c b.c |> echo %f > %o |> out.txt\n");
    let result = env.update();
    result.assert_success();
    let content = env.read_file("out.txt");
    assert!(content.contains("a.c"));
    assert!(content.contains("b.c"));
}

#[test]
fn t1102_percent_b_basename() {
    let env = TupTestEnv::new();
    env.write_file("src/main.c", "");
    env.write_tupfile_in("src", ": main.c |> echo %b > %o |> out.txt\n");
    let result = env.update();
    result.assert_success();
    assert!(env.read_file("src/out.txt").contains("main.c"));
}

#[test]
#[allow(non_snake_case)]
fn t1103_percent_B_no_ext() {
    let env = TupTestEnv::new();
    env.write_file("hello.c", "");
    env.write_tupfile(": hello.c |> echo %B > %o |> out.txt\n");
    let result = env.update();
    result.assert_success();
    assert!(env.read_file("out.txt").contains("hello"));
    // Should NOT contain the .c extension
    assert!(!env.read_file("out.txt").contains("hello.c"));
}

#[test]
fn t1104_percent_o_output() {
    let env = TupTestEnv::new();
    env.write_tupfile(": |> echo written > %o |> result.txt\n");
    let result = env.update();
    result.assert_success();
    assert!(env.read_file("result.txt").contains("written"));
}

#[test]
fn t1105_percent_e_extension() {
    let env = TupTestEnv::new();
    env.write_file("test.cpp", "");
    env.write_tupfile(": foreach test.cpp |> echo %e > %o |> %B.ext\n");
    let result = env.update();
    result.assert_success();
    assert!(env.read_file("test.ext").contains("cpp"));
}

#[test]
fn t1106_percent_literal() {
    let env = TupTestEnv::new();
    env.write_tupfile(": |> echo 100%% > %o |> out.txt\n");
    let result = env.update();
    result.assert_success();
    assert!(env.read_file("out.txt").contains("100%"));
}

// ============================================================
// t2200: Advanced variable tests
// ============================================================

#[test]
fn t2200_nested_variable_expansion() {
    let env = TupTestEnv::new();
    env.write_tupfile(
        "A = hello\n\
         B = $(A) world\n\
         : |> echo $(B) > %o |> out.txt\n"
    );
    let result = env.update();
    result.assert_success();
    let content = env.read_file("out.txt");
    assert!(content.contains("hello"));
    assert!(content.contains("world"));
}

#[test]
fn t2201_variable_in_output() {
    let env = TupTestEnv::new();
    env.write_tupfile(
        "OUT = result.txt\n\
         : |> echo done > %o |> $(OUT)\n"
    );
    let result = env.update();
    result.assert_success();
    env.check_exist("result.txt");
}

#[test]
fn t2202_variable_in_input() {
    let env = TupTestEnv::new();
    env.write_file("data.txt", "test_data");
    env.write_tupfile(
        "SRC = data.txt\n\
         : $(SRC) |> cp %f %o |> copy.txt\n"
    );
    let result = env.update();
    result.assert_success();
    assert_eq!(env.read_file("copy.txt"), "test_data");
}

#[test]
fn t2203_ifdef_nested() {
    let env = TupTestEnv::new();
    env.write_tupfile(
        "A = yes\n\
         B = yes\n\
         ifdef A\n\
         ifdef B\n\
         : |> echo both > %o |> out.txt\n\
         endif\n\
         endif\n"
    );
    let result = env.update();
    result.assert_success();
    env.check_exist("out.txt");
    assert!(env.read_file("out.txt").contains("both"));
}

#[test]
fn t2204_ifneq() {
    let env = TupTestEnv::new();
    env.write_tupfile(
        "CC = clang\n\
         ifneq ($(CC), gcc)\n\
         : |> echo not_gcc > %o |> out.txt\n\
         endif\n"
    );
    let result = env.update();
    result.assert_success();
    assert!(env.read_file("out.txt").contains("not_gcc"));
}

// ============================================================
// t2300: Comment and whitespace tests
// ============================================================

#[test]
fn t2300_comments_ignored() {
    let env = TupTestEnv::new();
    env.write_tupfile(
        "# This is a comment\n\
         : |> echo ok > %o |> out.txt\n\
         # Another comment\n"
    );
    let result = env.update();
    result.assert_success();
    env.check_exist("out.txt");
}

#[test]
fn t2301_line_continuation() {
    let env = TupTestEnv::new();
    env.write_tupfile(
        "MSG = hello \\\n\
         world\n\
         : |> echo $(MSG) > %o |> out.txt\n"
    );
    let result = env.update();
    result.assert_success();
    let content = env.read_file("out.txt");
    assert!(content.contains("hello"));
    assert!(content.contains("world"));
}

// ============================================================
// t2400: Bang macro advanced tests
// ============================================================

#[test]
fn t2400_bang_with_variable() {
    let env = TupTestEnv::new();
    env.write_file("input.txt", "data");
    env.write_tupfile(
        "CMD = cp\n\
         !copy = |> $(CMD) %f %o |> %B.out\n\
         : input.txt |> !copy |>\n"
    );
    let result = env.update();
    result.assert_success();
    env.check_exist("input.out");
    assert_eq!(env.read_file("input.out"), "data");
}

#[test]
fn t2401_bang_foreach() {
    let env = TupTestEnv::new();
    env.write_file("x.txt", "xx");
    env.write_file("y.txt", "yy");
    env.write_tupfile(
        "!cp = foreach |> cp %f %o |> %B.out\n\
         : *.txt |> !cp |>\n"
    );
    let result = env.update();
    result.assert_success();
    env.check_exist("x.out");
    env.check_exist("y.out");
}

// ============================================================
// t3100: Deep directory tests
// ============================================================

#[test]
fn t3100_deeply_nested() {
    let env = TupTestEnv::new();
    env.write_file("a/b/c/input.txt", "deep");
    env.write_tupfile_in("a/b/c", ": input.txt |> cp %f %o |> output.txt\n");
    let result = env.update();
    result.assert_success();
    env.check_exist("a/b/c/output.txt");
    assert_eq!(env.read_file("a/b/c/output.txt"), "deep");
}

#[test]
fn t3101_multiple_tupfiles_different_levels() {
    let env = TupTestEnv::new();
    env.write_tupfile(": |> echo root > %o |> root.txt\n");
    env.write_file("sub/data.txt", "sub_data");
    env.write_tupfile_in("sub", ": data.txt |> cp %f %o |> copy.txt\n");
    env.write_file("sub/deep/item.txt", "deep_item");
    env.write_tupfile_in("sub/deep", ": item.txt |> cp %f %o |> result.txt\n");

    let result = env.update();
    result.assert_success();
    env.check_exist("root.txt");
    env.check_exist("sub/copy.txt");
    env.check_exist("sub/deep/result.txt");
}

// ============================================================
// t4100: Error message tests
// ============================================================

#[test]
fn t4100_no_tupfile_no_error() {
    let env = TupTestEnv::new();
    // No Tupfile — should succeed with "No Tupfiles found"
    let result = env.update();
    result.assert_success();
}

#[test]
fn t4101_failed_command_shows_stderr() {
    let env = TupTestEnv::new();
    env.write_tupfile(": |> echo error_msg >&2 && false |>\n");
    let result = env.update();
    result.assert_failure();
    result.assert_stderr_contains("error_msg");
}

// ============================================================
// t5100: Scan command tests
// ============================================================

#[test]
fn t5100_scan_shows_counts() {
    let env = TupTestEnv::new();
    env.write_file("a.c", "");
    env.write_file("b.c", "");
    env.write_tupfile(": |> echo |>\n");
    let result = env.run_tup(&["scan"]);
    result.assert_success();
    // Scan output goes to stderr
    result.assert_stderr_contains("Scan:");
}

// ============================================================
// t8000: Display string tests
// ============================================================

#[test]
fn t8000_display_string() {
    let env = TupTestEnv::new();
    env.write_tupfile(": |> ^Creating output^ echo hello > %o |> out.txt\n");
    let result = env.update();
    result.assert_success();
    result.assert_stderr_contains("Creating output");
    env.check_exist("out.txt");
}

#[test]
fn t8001_options_command() {
    let env = TupTestEnv::new();
    let result = env.run_tup(&["options"]);
    result.assert_success();
    result.assert_stdout_contains("updater.num_jobs");
    result.assert_stdout_contains("db.sync");
}

// ============================================================
// t9000: Lua Tupfile tests
// ============================================================

#[test]
fn t9000_lua_basic_rule() {
    let env = TupTestEnv::new();
    env.write_file("input.txt", "lua_data");
    env.write_file("Tupfile.lua", r#"
tup.definerule{
    inputs = {"input.txt"},
    command = "cp %f %o",
    outputs = {"output.txt"},
}
"#);
    let result = env.update();
    result.assert_success();
    env.check_exist("output.txt");
    assert_eq!(env.read_file("output.txt"), "lua_data");
}

#[test]
fn t9001_lua_multiple_rules() {
    let env = TupTestEnv::new();
    env.write_file("Tupfile.lua", r#"
tup.definerule{inputs={}, command="echo one > one.txt", outputs={"one.txt"}}
tup.definerule{inputs={}, command="echo two > two.txt", outputs={"two.txt"}}
"#);
    let result = env.update();
    result.assert_success();
    env.check_exist("one.txt");
    env.check_exist("two.txt");
}

#[test]
fn t9002_lua_variables() {
    let env = TupTestEnv::new();
    env.write_file("Tupfile.lua", r#"
local msg = "from_lua"
tup.definerule{
    inputs = {},
    command = "echo " .. msg .. " > output.txt",
    outputs = {"output.txt"},
}
"#);
    let result = env.update();
    result.assert_success();
    assert!(env.read_file("output.txt").contains("from_lua"));
}

#[test]
fn t9003_lua_loop() {
    let env = TupTestEnv::new();
    env.write_file("a.txt", "a");
    env.write_file("b.txt", "b");
    env.write_file("c.txt", "c");
    env.write_file("Tupfile.lua", r#"
local files = {"a.txt", "b.txt", "c.txt"}
for _, f in ipairs(files) do
    local out = f:gsub("%.txt", ".out")
    tup.definerule{
        inputs = {f},
        command = "cp %f " .. out,
        outputs = {out},
    }
end
"#);
    let result = env.update();
    result.assert_success();
    env.check_exist("a.out");
    env.check_exist("b.out");
    env.check_exist("c.out");
}

#[test]
fn t9004_lua_glob() {
    let env = TupTestEnv::new();
    env.write_file("x.src", "xx");
    env.write_file("y.src", "yy");
    env.write_file("z.dat", "zz");
    env.write_file("Tupfile.lua", r#"
local srcs = tup.glob("*.src")
for _, f in ipairs(srcs) do
    local out = f:gsub("%.src", ".dst")
    tup.definerule{
        inputs = {f},
        command = "cp " .. f .. " " .. out,
        outputs = {out},
    }
end
"#);
    let result = env.update();
    result.assert_success();
    env.check_exist("x.dst");
    env.check_exist("y.dst");
    env.check_not_exist("z.dst"); // .dat not matched
}

#[test]
fn t9005_lua_parse() {
    let env = TupTestEnv::new();
    env.write_file("Tupfile.lua", r#"
tup.definerule{inputs={}, command="echo hi", outputs={"out.txt"}}
"#);
    let result = env.parse();
    result.assert_success();
    result.assert_stdout_contains("echo hi");
    result.assert_stdout_contains("out.txt");
}

// ============================================================
// t9100: Mixed Tupfile and Tupfile.lua tests
// ============================================================

#[test]
fn t9100_mixed_standard_and_lua() {
    let env = TupTestEnv::new();
    // Root uses standard Tupfile
    env.write_tupfile(": |> echo root > %o |> root.txt\n");
    // Subdirectory uses Lua
    env.write_file("sub/data.txt", "subdata");
    env.write_file("sub/Tupfile.lua", r#"
tup.definerule{
    inputs = {"data.txt"},
    command = "cp data.txt output.txt",
    outputs = {"output.txt"},
}
"#);
    let result = env.update();
    result.assert_success();
    env.check_exist("root.txt");
    env.check_exist("sub/output.txt");
}

// ============================================================
// t9200: Stress / edge case tests
// ============================================================

#[test]
fn t9200_many_rules() {
    let env = TupTestEnv::new();
    let mut tupfile = String::new();
    for i in 0..20 {
        env.write_file(&format!("input_{i}.txt"), &format!("data_{i}"));
        tupfile.push_str(&format!(
            ": input_{i}.txt |> cp %f %o |> output_{i}.txt\n"
        ));
    }
    env.write_tupfile(&tupfile);
    let result = env.update();
    result.assert_success();
    for i in 0..20 {
        env.check_exist(&format!("output_{i}.txt"));
    }
}

#[test]
fn t9201_empty_tupfile() {
    let env = TupTestEnv::new();
    env.write_tupfile("# Just a comment\n");
    let result = env.update();
    result.assert_success();
}

#[test]
fn t9202_special_characters_in_output() {
    let env = TupTestEnv::new();
    env.write_tupfile(": |> echo hello > %o |> out-put_file.2.txt\n");
    let result = env.update();
    result.assert_success();
    env.check_exist("out-put_file.2.txt");
}

#[test]
fn t9203_long_command() {
    let env = TupTestEnv::new();
    // Test with a long command line
    let long_args: String = (0..50).map(|i| format!("-DVAR{i}={i}")).collect::<Vec<_>>().join(" ");
    env.write_tupfile(&format!(": |> echo {long_args} > %o |> out.txt\n"));
    let result = env.update();
    result.assert_success();
    env.check_exist("out.txt");
}
