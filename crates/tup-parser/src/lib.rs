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
pub use percent::{expand_output_pattern, expand_percent, InputFile};
pub use rule::{Rule, RuleCommand};
pub use lua::parse_lua_tupfile;
pub use vardb::ParseVarDb;
pub use varsed::{varsed, varsed_file};
