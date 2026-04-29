//! Statement parser. Implementation lands in subsequent commits.

use crate::ast::{Block, File, Program};
use crate::diagnostic::Diagnostic;
use crate::source::Span;
use crate::token::TokenKind;

use super::Parser;

impl<'a> Parser<'a> {
    pub(crate) fn parse_program(&mut self) -> Result<Program, Diagnostic> {
        // For now: produce an empty program. Real statement parsing follows.
        // We simply consume tokens until EOF without analysing them so that
        // the interpreter's "not yet implemented" error fires from a later
        // pass rather than the parser.
        while !self.at_eof() {
            self.bump();
        }
        Ok(Program {
            files: vec![File {
                name: None,
                stmts: Vec::new(),
            }],
        })
    }

    pub(crate) fn parse_block(&mut self) -> Result<Block, Diagnostic> {
        let open = self.expect(|k| matches!(k, TokenKind::LBrace), "`{`")?;
        let close = self.expect(|k| matches!(k, TokenKind::RBrace), "`}`")?;
        Ok(Block {
            stmts: Vec::new(),
            span: open.span.merge(close.span),
        })
    }

    #[allow(dead_code)]
    fn _placeholder_for_span(_span: Span) {}
}
