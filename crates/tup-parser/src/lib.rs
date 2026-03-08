// tup-parser: Tupfile parser for the tup build system
//
// Parses Tupfile syntax including rules, variables, conditionals,
// includes, and bang macros.

mod errors;
mod lexer;
mod rule;
mod vardb;

pub use errors::ParseError;
pub use lexer::TupfileReader;
pub use rule::{Rule, RuleCommand};
pub use vardb::ParseVarDb;
