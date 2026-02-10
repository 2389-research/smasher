// ABOUTME: Module root for DOT graph parsing, exposing AST types and the parse function.
// ABOUTME: Provides lexer, parser, and AST submodules for DOT language processing.

pub mod ast;
pub mod lexer;
pub mod parser;

pub use ast::*;
pub use parser::parse;
