// tup-parser: Tupfile parser for the tup build system
//
// Parses Tupfile syntax including rules, variables, conditionals,
// includes, and bang macros.

mod bang;
mod errors;
mod glob;
mod lexer;
mod percent;
mod rule;
mod vardb;

pub use bang::BangDb;
pub use errors::ParseError;
pub use glob::{expand_globs, is_glob};
pub use lexer::TupfileReader;
pub use percent::{expand_output_pattern, expand_percent, InputFile};
pub use rule::{Rule, RuleCommand};
pub use vardb::ParseVarDb;
