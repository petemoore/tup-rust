// tup-parser: Tupfile parser for the tup build system
//
// Parses Tupfile syntax including rules, variables, conditionals,
// includes, and bang macros.

mod bang;
mod errors;
pub mod glob;
mod lexer;
mod lua;
mod percent;
mod rule;
mod vardb;
mod varsed;

pub use bang::BangDb;
pub use errors::ParseError;
pub use glob::{expand_globs, is_glob};
pub use lexer::TupfileReader;
pub use lua::parse_lua_tupfile;
pub use percent::{expand_output_pattern, expand_percent, validate_output_path, InputFile};
pub use rule::{Rule, RuleCommand};
pub use vardb::ParseVarDb;
pub use varsed::{
    cmd_varsed, load_vardict, parse_vardict_binary, parse_vardict_text, varsed, varsed_binary,
    varsed_file,
};
