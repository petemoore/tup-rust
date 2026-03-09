use mlua::prelude::*;
use std::path::Path;

use crate::rule::{Rule, RuleCommand};

/// Parse a Tupfile.lua and extract rules.
///
/// Provides a `tup` table with functions matching the C Lua API:
/// - tup.definerule{inputs={...}, command="...", outputs={...}}
/// - tup.getconfig(name)
/// - tup.getcwd()
/// - tup.glob(pattern)
pub fn parse_lua_tupfile(
    content: &str,
    filename: &str,
    work_dir: &Path,
) -> Result<Vec<Rule>, String> {
    let lua = Lua::new();
    let rules = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Rule>::new()));
    let work_dir_owned = work_dir.to_path_buf();

    // Create the tup table
    let tup_table = lua.create_table().map_err(lua_err)?;

    // tup.definerule
    let rules_clone = rules.clone();
    let definerule = lua.create_function(move |lua_ctx, table: LuaTable| {
        let inputs: Vec<String> = table.get::<_, LuaTable>("inputs")
            .map(|t| table_to_vec(&t))
            .unwrap_or_default();

        let command: String = table.get::<_, String>("command")
            .unwrap_or_default();

        let outputs: Vec<String> = table.get::<_, LuaTable>("outputs")
            .map(|t| table_to_vec(&t))
            .unwrap_or_default();

        let extra_inputs: Vec<String> = table.get::<_, LuaTable>("extra_inputs")
            .map(|t| table_to_vec(&t))
            .unwrap_or_default();

        let extra_outputs: Vec<String> = table.get::<_, LuaTable>("extra_outputs")
            .map(|t| table_to_vec(&t))
            .unwrap_or_default();

        let foreach: bool = table.get::<_, bool>("foreach").unwrap_or(false);

        let had_inputs = !inputs.is_empty();
        let rule = Rule {
            foreach,
            inputs,
            order_only_inputs: extra_inputs,
            command: RuleCommand {
                display: None,
                flags: None,
                command,
            },
            outputs: outputs.clone(),
            extra_outputs,
            line_number: 0,
            had_inputs,
        };

        rules_clone.lock().unwrap().push(rule);

        // Return output table (for chaining)
        let result = lua_ctx.create_table()?;
        for (i, out) in outputs.iter().enumerate() {
            result.set(i + 1, out.as_str())?;
        }
        Ok(result)
    }).map_err(lua_err)?;
    tup_table.set("definerule", definerule).map_err(lua_err)?;

    // tup.rule (alias for definerule with positional args)
    let rules_clone2 = rules.clone();
    let rule_fn = lua.create_function(move |_lua_ctx, table: LuaTable| {
        let inputs: Vec<String> = table.get::<_, LuaTable>(1)
            .map(|t| table_to_vec(&t))
            .unwrap_or_default();

        let command: String = table.get::<_, String>(2)
            .unwrap_or_default();

        let outputs: Vec<String> = table.get::<_, LuaTable>(3)
            .map(|t| table_to_vec(&t))
            .unwrap_or_default();

        let had_inputs = !inputs.is_empty();
        let rule = Rule {
            foreach: false,
            inputs,
            order_only_inputs: vec![],
            command: RuleCommand {
                display: None,
                flags: None,
                command,
            },
            outputs,
            extra_outputs: vec![],
            line_number: 0,
            had_inputs,
        };

        rules_clone2.lock().unwrap().push(rule);
        Ok(())
    }).map_err(lua_err)?;
    tup_table.set("rule", rule_fn).map_err(lua_err)?;

    // tup.getcwd()
    let cwd = work_dir.to_string_lossy().to_string();
    let getcwd = lua.create_function(move |_, ()| {
        Ok(cwd.clone())
    }).map_err(lua_err)?;
    tup_table.set("getcwd", getcwd).map_err(lua_err)?;

    // tup.getconfig(name)
    let getconfig = lua.create_function(|_, _name: String| {
        // Config integration would go here
        Ok(LuaValue::Nil)
    }).map_err(lua_err)?;
    tup_table.set("getconfig", getconfig).map_err(lua_err)?;

    // tup.glob(pattern)
    let wd = work_dir_owned.clone();
    let glob_fn = lua.create_function(move |lua_ctx, pattern: String| {
        let matches = crate::glob::expand_globs(
            &[pattern],
            &wd,
        ).unwrap_or_default();

        let result = lua_ctx.create_table()?;
        for (i, m) in matches.iter().enumerate() {
            result.set(i + 1, m.as_str())?;
        }
        Ok(result)
    }).map_err(lua_err)?;
    tup_table.set("glob", glob_fn).map_err(lua_err)?;

    // tup.export(varname)
    let export_fn = lua.create_function(|_, _name: String| {
        Ok(())
    }).map_err(lua_err)?;
    tup_table.set("export", export_fn).map_err(lua_err)?;

    // tup.creategitignore()
    let gitignore_fn = lua.create_function(|_, ()| {
        Ok(())
    }).map_err(lua_err)?;
    tup_table.set("creategitignore", gitignore_fn).map_err(lua_err)?;

    // tup.append_table(t1, t2)
    let append_fn = lua.create_function(|_, (t1, t2): (LuaTable, LuaTable)| {
        let len: i64 = t1.len()?;
        for pair in t2.pairs::<i64, LuaValue>() {
            let (_, val) = pair?;
            t1.set(len + 1, val)?;
        }
        Ok(())
    }).map_err(lua_err)?;
    tup_table.set("append_table", append_fn).map_err(lua_err)?;

    // Set tup as global
    lua.globals().set("tup", tup_table).map_err(lua_err)?;

    // Execute the Lua script
    lua.load(content)
        .set_name(filename)
        .exec()
        .map_err(|e| format!("Lua error in {filename}: {e}"))?;

    // Drop the Lua VM to release Arc references held by closures
    drop(lua);

    let result = std::sync::Arc::try_unwrap(rules)
        .map_err(|arc| format!("internal error: {} references to rules remain", std::sync::Arc::strong_count(&arc)))?
        .into_inner()
        .unwrap();

    Ok(result)
}

/// Convert a Lua table to a Vec<String>.
fn table_to_vec(table: &LuaTable) -> Vec<String> {
    let mut result = Vec::new();
    for (_, val) in table.clone().pairs::<i64, String>().flatten() {
        result.push(val);
    }
    result
}

fn lua_err(e: LuaError) -> String {
    format!("Lua error: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lua_definerule() {
        let content = r#"
tup.definerule{
    inputs = {"main.c"},
    command = "gcc -c main.c -o main.o",
    outputs = {"main.o"},
}
"#;
        let tmp = tempfile::tempdir().unwrap();
        let rules = parse_lua_tupfile(content, "Tupfile.lua", tmp.path()).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].inputs, vec!["main.c"]);
        assert_eq!(rules[0].command.command, "gcc -c main.c -o main.o");
        assert_eq!(rules[0].outputs, vec!["main.o"]);
    }

    #[test]
    fn test_lua_multiple_rules() {
        let content = r#"
tup.definerule{inputs={"a.c"}, command="gcc -c a.c -o a.o", outputs={"a.o"}}
tup.definerule{inputs={"b.c"}, command="gcc -c b.c -o b.o", outputs={"b.o"}}
"#;
        let tmp = tempfile::tempdir().unwrap();
        let rules = parse_lua_tupfile(content, "Tupfile.lua", tmp.path()).unwrap();
        assert_eq!(rules.len(), 2);
    }

    #[test]
    fn test_lua_foreach() {
        let content = r#"
tup.definerule{
    inputs = {"a.c", "b.c"},
    command = "gcc -c %f -o %o",
    outputs = {"%B.o"},
    foreach = true,
}
"#;
        let tmp = tempfile::tempdir().unwrap();
        let rules = parse_lua_tupfile(content, "Tupfile.lua", tmp.path()).unwrap();
        assert_eq!(rules.len(), 1);
        assert!(rules[0].foreach);
    }

    #[test]
    fn test_lua_getcwd() {
        let content = r#"
local cwd = tup.getcwd()
tup.definerule{inputs={}, command="echo " .. cwd, outputs={}}
"#;
        let tmp = tempfile::tempdir().unwrap();
        let rules = parse_lua_tupfile(content, "Tupfile.lua", tmp.path()).unwrap();
        assert_eq!(rules.len(), 1);
        assert!(rules[0].command.command.starts_with("echo "));
    }

    #[test]
    fn test_lua_glob() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("x.c"), "").unwrap();
        std::fs::write(tmp.path().join("y.c"), "").unwrap();
        std::fs::write(tmp.path().join("z.h"), "").unwrap();

        let content = r#"
local srcs = tup.glob("*.c")
for _, f in ipairs(srcs) do
    tup.definerule{inputs={f}, command="gcc -c " .. f, outputs={}}
end
"#;
        let rules = parse_lua_tupfile(content, "Tupfile.lua", tmp.path()).unwrap();
        assert_eq!(rules.len(), 2); // x.c and y.c, not z.h
    }

    #[test]
    fn test_lua_variables() {
        let content = r#"
CC = "gcc"
CFLAGS = "-Wall -O2"
tup.definerule{
    inputs = {"main.c"},
    command = CC .. " " .. CFLAGS .. " -c main.c -o main.o",
    outputs = {"main.o"},
}
"#;
        let tmp = tempfile::tempdir().unwrap();
        let rules = parse_lua_tupfile(content, "Tupfile.lua", tmp.path()).unwrap();
        assert_eq!(rules[0].command.command, "gcc -Wall -O2 -c main.c -o main.o");
    }

    #[test]
    fn test_lua_error() {
        let content = "this is not valid lua {{{";
        let tmp = tempfile::tempdir().unwrap();
        let result = parse_lua_tupfile(content, "Tupfile.lua", tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_lua_no_rules() {
        let content = "-- just a comment\nlocal x = 1\n";
        let tmp = tempfile::tempdir().unwrap();
        let rules = parse_lua_tupfile(content, "Tupfile.lua", tmp.path()).unwrap();
        assert!(rules.is_empty());
    }
}
