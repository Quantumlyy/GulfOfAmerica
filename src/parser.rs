//! Parser (placeholder — full implementation follows in a later commit).

use crate::ast::Program;
use crate::diagnostic::Diagnostic;
use crate::source::SourceFile;
use crate::token::Token;

pub fn parse(_file: &SourceFile, _tokens: Vec<Token>) -> Result<Program, Diagnostic> {
    Err(Diagnostic::error("parser not yet implemented"))
}
