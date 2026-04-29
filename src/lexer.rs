//! Lexer (placeholder — full implementation follows in a later commit).

use crate::diagnostic::Diagnostic;
use crate::source::SourceFile;
use crate::token::Token;

pub fn lex(_file: &SourceFile) -> Result<Vec<Token>, Diagnostic> {
    Err(Diagnostic::error("lexer not yet implemented"))
}
