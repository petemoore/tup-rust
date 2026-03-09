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
    let definerule = lua
        .create_function(move |lua_ctx, table: LuaTable| {
            let inputs: Vec<String> = table
                .get::<_, LuaTable>("inputs")
                .map(|t| table_to_vec(&t))
                .unwrap_or_default();

            let command: String = table.get::<_, String>("command").unwrap_or_default();

            let outputs: Vec<String> = table
                .get::<_, LuaTable>("outputs")
                .map(|t| table_to_vec(&t))
                .unwrap_or_default();

            let extra_inputs: Vec<String> = table
                .get::<_, LuaTable>("extra_inputs")
                .map(|t| table_to_vec(&t))
                .unwrap_or_default();

            let extra_outputs: Vec<String> = table
                .get::<_, LuaTable>("extra_outputs")
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
                vars_snapshot: None,
                bin: None,
            };

            rules_clone.lock().unwrap().push(rule);

            // Return output table (for chaining)
            let result = lua_ctx.create_table()?;
            for (i, out) in outputs.iter().enumerate() {
                result.set(i + 1, out.as_str())?;
            }
            Ok(result)
        })
        .map_err(lua_err)?;
    tup_table.set("definerule", definerule).map_err(lua_err)?;

    // tup.rule (alias for definerule with positional args)
    // Supports multiple calling conventions:
    //   tup.rule('command')
    //   tup.rule({'inputs'}, 'command')
    //   tup.rule({'inputs'}, 'command', {'outputs'})
    //   tup.rule('command', {'outputs'})
    let rules_clone2 = rules.clone();
    let rule_fn = lua
        .create_function(move |_lua_ctx, args: LuaMultiValue| {
            let mut inputs = Vec::new();
            let mut command = String::new();
            let mut outputs = Vec::new();

            let arg_list: Vec<LuaValue> = args.into_iter().collect();
            match arg_list.len() {
                1 => {
                    // tup.rule('command') or tup.rule({inputs, command, outputs})
                    match &arg_list[0] {
                        LuaValue::String(s) => {
                            command = s.to_str().unwrap_or("").to_string();
                        }
                        LuaValue::Table(t) => {
                            inputs = t
                                .get::<_, LuaTable>(1)
                                .map(|t| table_to_vec(&t))
                                .unwrap_or_default();
                            command = t.get::<_, String>(2).unwrap_or_default();
                            outputs = t
                                .get::<_, LuaTable>(3)
                                .map(|t| table_to_vec(&t))
                                .unwrap_or_default();
                        }
                        _ => {}
                    }
                }
                2 => {
                    // tup.rule({'inputs'}, 'command') or tup.rule('command', {'outputs'})
                    match (&arg_list[0], &arg_list[1]) {
                        (LuaValue::Table(t), LuaValue::String(s)) => {
                            inputs = table_to_vec(t);
                            command = s.to_str().unwrap_or("").to_string();
                        }
                        (LuaValue::String(s), LuaValue::Table(t)) => {
                            command = s.to_str().unwrap_or("").to_string();
                            outputs = table_to_vec(t);
                        }
                        _ => {}
                    }
                }
                3 => {
                    // tup.rule(inputs, 'command', outputs)
                    // inputs/outputs can be strings or tables
                    command = match &arg_list[1] {
                        LuaValue::String(s) => s.to_str().unwrap_or("").to_string(),
                        _ => String::new(),
                    };
                    inputs = lua_value_to_string_vec(&arg_list[0]);
                    outputs = lua_value_to_string_vec(&arg_list[2]);
                }
                _ => {}
            }

            let had_inputs = !inputs.is_empty();
            let output_copy = outputs.clone();
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
                vars_snapshot: None,
                bin: None,
            };

            rules_clone2.lock().unwrap().push(rule);

            // Return outputs as a table (for chaining)
            let result = _lua_ctx.create_table()?;
            for (i, out) in output_copy.iter().enumerate() {
                result.set(i + 1, out.as_str())?;
            }
            Ok(result)
        })
        .map_err(lua_err)?;
    tup_table.set("rule", rule_fn).map_err(lua_err)?;

    // tup.foreach_rule (like tup.rule but with foreach=true)
    let rules_clone3 = rules.clone();
    let foreach_rule_fn = lua
        .create_function(move |_lua_ctx, args: LuaMultiValue| {
            let mut inputs = Vec::new();
            let mut command = String::new();
            let mut outputs = Vec::new();

            let arg_list: Vec<LuaValue> = args.into_iter().collect();
            match arg_list.len() {
                2 => {
                    inputs = lua_value_to_string_vec(&arg_list[0]);
                    command = match &arg_list[1] {
                        LuaValue::String(s) => s.to_str().unwrap_or("").to_string(),
                        _ => String::new(),
                    };
                }
                3 => {
                    inputs = lua_value_to_string_vec(&arg_list[0]);
                    command = match &arg_list[1] {
                        LuaValue::String(s) => s.to_str().unwrap_or("").to_string(),
                        _ => String::new(),
                    };
                    outputs = lua_value_to_string_vec(&arg_list[2]);
                }
                _ => {}
            }

            let had_inputs = !inputs.is_empty();
            let output_copy = outputs.clone();
            let rule = Rule {
                foreach: true,
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
                vars_snapshot: None,
                bin: None,
            };

            rules_clone3.lock().unwrap().push(rule);

            // Return outputs as a table (for chaining)
            let result = _lua_ctx.create_table()?;
            for (i, out) in output_copy.iter().enumerate() {
                result.set(i + 1, out.as_str())?;
            }
            Ok(result)
        })
        .map_err(lua_err)?;
    tup_table
        .set("foreach_rule", foreach_rule_fn)
        .map_err(lua_err)?;

    // tup.getcwd()
    let cwd = work_dir.to_string_lossy().to_string();
    let getcwd = lua
        .create_function(move |_, ()| Ok(cwd.clone()))
        .map_err(lua_err)?;
    tup_table.set("getcwd", getcwd).map_err(lua_err)?;

    // tup.getconfig(name)
    let getconfig = lua
        .create_function(|_, _name: String| {
            // Config integration would go here
            Ok(LuaValue::Nil)
        })
        .map_err(lua_err)?;
    tup_table.set("getconfig", getconfig).map_err(lua_err)?;

    // tup.glob(pattern)
    let wd = work_dir_owned.clone();
    let glob_fn = lua
        .create_function(move |lua_ctx, pattern: String| {
            let matches = crate::glob::expand_globs(&[pattern], &wd).unwrap_or_default();

            let result = lua_ctx.create_table()?;
            for (i, m) in matches.iter().enumerate() {
                result.set(i + 1, m.as_str())?;
            }
            Ok(result)
        })
        .map_err(lua_err)?;
    tup_table.set("glob", glob_fn).map_err(lua_err)?;

    // tup.include(filename) — load and execute a Lua file
    let wd_for_include = work_dir_owned.clone();
    let include_fn = lua
        .create_function(move |lua_ctx, filename: String| {
            let path = wd_for_include.join(&filename);
            let content = std::fs::read_to_string(&path).map_err(|e| {
                mlua::Error::RuntimeError(format!("Cannot include '{}': {}", filename, e))
            })?;
            let processed = preprocess_lua_plus_equals(&content);
            lua_ctx
                .load(&processed)
                .set_name(&filename)
                .exec()
                .map_err(|e| {
                    mlua::Error::RuntimeError(format!(
                        "Error in included file '{}': {}",
                        filename, e
                    ))
                })?;
            Ok(())
        })
        .map_err(lua_err)?;
    tup_table.set("include", include_fn).map_err(lua_err)?;

    // tup.import(varname) — import an environment variable as a Lua global
    let import_fn = lua
        .create_function(|lua_ctx, name: String| {
            match std::env::var(&name) {
                Ok(val) => {
                    lua_ctx.globals().set(name, val)?;
                }
                Err(_) => {
                    lua_ctx.globals().set(name, mlua::Value::Nil)?;
                }
            }
            Ok(())
        })
        .map_err(lua_err)?;
    tup_table.set("import", import_fn).map_err(lua_err)?;

    // tup.export(varname)
    let export_fn = lua
        .create_function(|_, _name: String| Ok(()))
        .map_err(lua_err)?;
    tup_table.set("export", export_fn).map_err(lua_err)?;

    // tup.creategitignore()
    let gitignore_fn = lua.create_function(|_, ()| Ok(())).map_err(lua_err)?;
    tup_table
        .set("creategitignore", gitignore_fn)
        .map_err(lua_err)?;

    // tup.append_table(t1, t2)
    let append_fn = lua
        .create_function(|_, (t1, t2): (LuaTable, LuaTable)| {
            let len: i64 = t1.len()?;
            for pair in t2.pairs::<i64, LuaValue>() {
                let (_, val) = pair?;
                t1.set(len + 1, val)?;
            }
            Ok(())
        })
        .map_err(lua_err)?;
    tup_table.set("append_table", append_fn).map_err(lua_err)?;

    // tup.getrelativedir(dir) — stub for now
    let getreldir_fn = lua
        .create_function(|_, _dir: String| Ok("".to_string()))
        .map_err(lua_err)?;
    tup_table
        .set("getrelativedir", getreldir_fn)
        .map_err(lua_err)?;

    // Set tup as global
    lua.globals().set("tup", tup_table).map_err(lua_err)?;

    // Register tup_append_assignment (C tup's += operator support)
    // This function appends a value (string or table) to an existing value.
    let builtin_lua = r#"
tup_table_meta = {}
tup_table_meta.__tostring = function(t)
    return table.concat(t, ' ')
end
tup_table_meta.__concat = function(a, b)
    if type(a) == 'table' then a = tostring(a) end
    if type(b) == 'table' then b = tostring(b) end
    return a .. b
end

tup_append_assignment = function(a, b)
    local result
    if type(a) == 'string' then
        result = {a}
    elseif type(a) == 'table' then
        result = a
    elseif type(a) == 'nil' then
        result = {}
    else
        error('+= operator only works when the source is a table or string')
    end
    if type(b) == 'string' then
        result[#result+1] = b
    elseif type(b) == 'table' then
        for _, v in ipairs(b) do
            result[#result+1] = v
        end
    elseif type(b) ~= 'nil' then
        error('+= operator only works when the value is a table or string')
    end
    setmetatable(result, tup_table_meta)
    return result
end

-- Helper: convert string to table
local function tableize(t)
    if type(t) == 'string' then return {t} end
    if type(t) == 'table' then return t end
    return {}
end

-- tup.frule(arguments) — normalize and call definerule
tup.frule = function(arguments)
    if arguments.inputs then
        if type(arguments.inputs) == 'table' then
            if arguments.inputs.extra_inputs then
                arguments.extra_inputs = tableize(arguments.inputs.extra_inputs)
                arguments.inputs['extra_inputs'] = nil
            end
        end
        arguments.inputs = tableize(arguments.inputs)
    end
    if arguments.outputs then
        if type(arguments.outputs) == 'table' then
            if arguments.outputs.extra_outputs then
                arguments.extra_outputs = tableize(arguments.outputs.extra_outputs)
                arguments.outputs['extra_outputs'] = nil
            end
        end
        arguments.outputs = tableize(arguments.outputs)
    end
    return tup.definerule(arguments)
end

-- tup.rule(inputs, command, outputs) — shorthand
tup.rule = function(a, b, c)
    if c then
        -- 3-arg form: inputs, command, outputs
        local inputs = type(a) == 'string' and {a} or (a or {})
        local outputs = type(c) == 'string' and {c} or (c or {})
        return tup.definerule{inputs=inputs, command=b, outputs=outputs}
    elseif b then
        -- 2-arg form: command + outputs or inputs + command
        if type(a) == 'table' and type(b) == 'string' then
            return tup.definerule{inputs=a, command=b, outputs={}}
        elseif type(a) == 'string' and type(b) == 'table' then
            return tup.definerule{inputs={}, command=a, outputs=b}
        elseif type(a) == 'string' and type(b) == 'string' then
            return tup.definerule{inputs={}, command=a, outputs={b}}
        end
    else
        -- 1-arg form: just command
        return tup.definerule{inputs={}, command=a, outputs={}}
    end
end
"#;
    lua.load(builtin_lua)
        .set_name("builtin")
        .exec()
        .map_err(lua_err)?;

    // Preprocess content: convert `var += value` into `var = tup_append_assignment(var, value)`
    let processed = preprocess_lua_plus_equals(content);

    // Execute the Lua script
    lua.load(&processed)
        .set_name(filename)
        .exec()
        .map_err(|e| format!("Lua error in {filename}: {e}"))?;

    // Drop the Lua VM to release Arc references held by closures
    drop(lua);

    let result = std::sync::Arc::try_unwrap(rules)
        .map_err(|arc| {
            format!(
                "internal error: {} references to rules remain",
                std::sync::Arc::strong_count(&arc)
            )
        })?
        .into_inner()
        .unwrap();

    Ok(result)
}

/// Preprocess Lua content to convert `var += value` into `var = tup_append_assignment(var, value)`.
///
/// This matches C tup's patched Lua parser which natively supports +=.
/// We do it as a text transformation since we use unpatched mlua.
fn preprocess_lua_plus_equals(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    for line in content.lines() {
        let trimmed = line.trim();
        // Match lines like `varname += value` (not inside strings or comments)
        if !trimmed.starts_with("--") {
            if let Some(pos) = line.find("+=") {
                // Check this isn't inside a string (simple heuristic: no quotes before +=)
                let before = &line[..pos];
                let quote_count = before.chars().filter(|c| *c == '"' || *c == '\'').count();
                if quote_count % 2 == 0 {
                    // This is a real += operator
                    let var_name = before.trim();
                    let value = line[pos + 2..].trim();
                    result.push_str(var_name);
                    result.push_str(" = tup_append_assignment(");
                    result.push_str(var_name);
                    result.push_str(", ");
                    result.push_str(value);
                    result.push_str(")\n");
                    continue;
                }
            }
        }
        result.push_str(line);
        result.push('\n');
    }
    result
}

/// Convert a Lua table to a Vec<String>.
fn table_to_vec(table: &LuaTable) -> Vec<String> {
    let mut result = Vec::new();
    for (_, val) in table.clone().pairs::<i64, String>().flatten() {
        result.push(val);
    }
    result
}

/// Convert a Lua value (string or table) to a Vec<String>.
fn lua_value_to_string_vec(val: &LuaValue) -> Vec<String> {
    match val {
        LuaValue::String(s) => {
            let s = s.to_str().unwrap_or("").to_string();
            if s.is_empty() {
                vec![]
            } else {
                vec![s]
            }
        }
        LuaValue::Table(t) => table_to_vec(t),
        _ => vec![],
    }
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
        assert_eq!(
            rules[0].command.command,
            "gcc -Wall -O2 -c main.c -o main.o"
        );
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
