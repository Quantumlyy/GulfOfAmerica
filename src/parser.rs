//! Parser.
//!
//! A hand-written recursive-descent parser with a Pratt-style expression
//! engine. The expression engine implements *whitespace-significant
//! precedence*: operators that "hug" their operands (no spaces) bind tighter
//! than operators with spaces around them.

mod expr;
mod stmt;

use crate::ast::Program;
use crate::diagnostic::{Diagnostic, Label};
use crate::source::{SourceFile, Span};
use crate::token::{Token, TokenKind};

pub fn parse(file: &SourceFile, tokens: Vec<Token>) -> Result<Program, Diagnostic> {
    let mut p = Parser::new(file, tokens);
    p.parse_program()
}

pub(crate) struct Parser<'a> {
    pub(crate) file: &'a SourceFile,
    pub(crate) tokens: Vec<Token>,
    pub(crate) pos: usize,
}

impl<'a> Parser<'a> {
    pub(crate) fn new(file: &'a SourceFile, tokens: Vec<Token>) -> Self {
        Self {
            file,
            tokens,
            pos: 0,
        }
    }

    pub(crate) fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    pub(crate) fn peek_at(&self, off: usize) -> &Token {
        let idx = (self.pos + off).min(self.tokens.len() - 1);
        &self.tokens[idx]
    }

    pub(crate) fn bump(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if !matches!(tok.kind, TokenKind::Eof) {
            self.pos += 1;
        }
        tok
    }

    pub(crate) fn at_eof(&self) -> bool {
        matches!(self.peek().kind, TokenKind::Eof)
    }

    pub(crate) fn at(&self, kind_match: impl Fn(&TokenKind) -> bool) -> bool {
        kind_match(&self.peek().kind)
    }

    pub(crate) fn eat(&mut self, kind_match: impl Fn(&TokenKind) -> bool) -> Option<Token> {
        if kind_match(&self.peek().kind) {
            Some(self.bump())
        } else {
            None
        }
    }

    pub(crate) fn expect(
        &mut self,
        kind_match: impl Fn(&TokenKind) -> bool,
        what: &str,
    ) -> Result<Token, Diagnostic> {
        if kind_match(&self.peek().kind) {
            Ok(self.bump())
        } else {
            let actual = self.peek().kind.describe();
            let span = self.peek().span;
            Err(Diagnostic::error(format!("expected {what}, found {actual}"))
                .with_code("E0100")
                .with_label(Label::primary(span, format!("expected {what} here"))))
        }
    }

    pub(crate) fn span_to_here(&self, start: Span) -> Span {
        let here = if self.pos == 0 {
            self.peek().span
        } else {
            self.tokens[self.pos - 1].span
        };
        start.merge(here)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer;

    pub(crate) fn parse_str(s: &str) -> Result<Program, String> {
        let file = SourceFile::new("t.gom".into(), s.into());
        let tokens = lexer::lex(&file).map_err(|e| e.render(&file))?;
        parse(&file, tokens).map_err(|e| e.render(&file))
    }

    #[test]
    fn parses_empty_program() {
        let p = parse_str("").unwrap();
        assert_eq!(p.files.len(), 1);
        assert!(p.files[0].stmts.is_empty());
    }
}
