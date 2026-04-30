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

/// Parse and return the first diagnostic on failure. Kept as a thin wrapper
/// over [`parse_recovering`] for callers that don't need multi-error
/// reporting.
pub fn parse(file: &SourceFile, tokens: Vec<Token>) -> Result<Program, Diagnostic> {
    let (program, mut diags) = parse_recovering(file, tokens);
    if diags.is_empty() {
        Ok(program)
    } else {
        Err(diags.remove(0))
    }
}

/// Parse with error recovery. Always returns a [`Program`] (possibly with
/// holes around recovery points) plus every diagnostic encountered. An
/// empty `Vec` means a clean parse.
pub fn parse_recovering(file: &SourceFile, tokens: Vec<Token>) -> (Program, Vec<Diagnostic>) {
    let mut p = Parser::new(file, tokens);
    p.parse_program_recovering()
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

    #[allow(dead_code)]
    pub(crate) fn at(&self, kind_match: impl Fn(&TokenKind) -> bool) -> bool {
        kind_match(&self.peek().kind)
    }

    #[allow(dead_code)]
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

    #[allow(dead_code)]
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

    #[test]
    fn parses_simple_call_with_bang() {
        let p = parse_str(r#"print("hi")!"#).unwrap();
        assert_eq!(p.files[0].stmts.len(), 1);
    }

    #[test]
    fn parses_question_mark_terminator() {
        let p = parse_str(r#"print("hi")?"#).unwrap();
        assert_eq!(p.files[0].stmts.len(), 1);
    }

    #[test]
    fn parses_assignment_statement() {
        let p = parse_str(r#"x = 5!"#).unwrap();
        assert_eq!(p.files[0].stmts.len(), 1);
        assert!(matches!(p.files[0].stmts[0], crate::ast::Stmt::Assign { .. }));
    }

    #[test]
    fn rejects_missing_terminator() {
        let err = parse_str("print 1\n").unwrap_err();
        assert!(err.contains("expected `!`"));
    }

    #[test]
    fn parses_const_const_declaration() {
        let p = parse_str(r#"const const x = 5!"#).unwrap();
        let s = &p.files[0].stmts[0];
        match s {
            crate::ast::Stmt::Let { decl, const_depth, priority, .. } => {
                assert_eq!(*decl, crate::ast::DeclKind::ConstConst);
                assert_eq!(*const_depth, 2);
                assert_eq!(*priority, 1);
            }
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn parses_const_const_const_declaration() {
        let p = parse_str(r#"const const const pi = 3.14!"#).unwrap();
        match &p.files[0].stmts[0] {
            crate::ast::Stmt::Let { const_depth, .. } => assert_eq!(*const_depth, 3),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_var_var_declaration() {
        let p = parse_str(r#"var var x = 5!"#).unwrap();
        match &p.files[0].stmts[0] {
            crate::ast::Stmt::Let { decl, .. } => {
                assert_eq!(*decl, crate::ast::DeclKind::VarVar);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_lifetime_lines() {
        let p = parse_str(r#"const const x<2> = 5!"#).unwrap();
        match &p.files[0].stmts[0] {
            crate::ast::Stmt::Let { lifetime, .. } => {
                assert!(matches!(lifetime, Some(crate::ast::Lifetime::Lines(2))));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_lifetime_negative_for_hoisting() {
        let p = parse_str(r#"const const x<-1> = 5!"#).unwrap();
        match &p.files[0].stmts[0] {
            crate::ast::Stmt::Let { lifetime, .. } => {
                assert!(matches!(lifetime, Some(crate::ast::Lifetime::Lines(-1))));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_lifetime_seconds() {
        let p = parse_str(r#"const const x<20s> = 5!"#).unwrap();
        match &p.files[0].stmts[0] {
            crate::ast::Stmt::Let { lifetime, .. } => match lifetime {
                Some(crate::ast::Lifetime::Seconds(s)) => assert!((*s - 20.0).abs() < 1e-9),
                other => panic!("expected seconds, got {other:?}"),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn parses_lifetime_infinity() {
        let p = parse_str(r#"const const x<Infinity> = 5!"#).unwrap();
        match &p.files[0].stmts[0] {
            crate::ast::Stmt::Let { lifetime, .. } => {
                assert!(matches!(lifetime, Some(crate::ast::Lifetime::Infinity)));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_priority_from_bang_count() {
        let p = parse_str(r#"const const x = 5!!!"#).unwrap();
        match &p.files[0].stmts[0] {
            crate::ast::Stmt::Let { priority, .. } => assert_eq!(*priority, 3),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_negative_priority_from_inverted_bang() {
        let p = parse_str(r#"const const x = 5¡"#).unwrap();
        match &p.files[0].stmts[0] {
            crate::ast::Stmt::Let { priority, .. } => assert_eq!(*priority, -1),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_redefining_a_number_literal_as_a_name() {
        let p = parse_str(r#"const const 5 = 4!"#).unwrap();
        match &p.files[0].stmts[0] {
            crate::ast::Stmt::Let { target, .. } => match target {
                crate::ast::BindingTarget::Ident { name, .. } => assert_eq!(name, "5"),
                _ => panic!(),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn parses_if_else() {
        let p = parse_str(r#"if (x = 1) { print(1)! } else { print(2)! }"#).unwrap();
        match &p.files[0].stmts[0] {
            crate::ast::Stmt::If {
                else_block: Some(_),
                ..
            } => {}
            other => panic!("expected if/else, got {other:?}"),
        }
    }

    #[test]
    fn parses_when_block() {
        let p = parse_str(r#"when (h = 0) { print("dead")! }"#).unwrap();
        assert!(matches!(p.files[0].stmts[0], crate::ast::Stmt::When { .. }));
    }

    #[test]
    fn parses_fn_decl_with_expr_body() {
        let p = parse_str(r#"function add(a, b) => a + b!"#).unwrap();
        match &p.files[0].stmts[0] {
            crate::ast::Stmt::FnDecl { name, params, .. } => {
                assert_eq!(name, "add");
                assert_eq!(params.len(), 2);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_fn_decl_with_block_body() {
        let p = parse_str(
            r#"function greet() => {
   print("hi")!
}!"#,
        )
        .unwrap();
        assert!(matches!(p.files[0].stmts[0], crate::ast::Stmt::FnDecl { .. }));
    }

    #[test]
    fn parses_class_decl_with_field_and_method() {
        let p = parse_str(
            r#"class Player {
   const var health = 10!
   function heal() => {
      health = 11!
   }!
}"#,
        )
        .unwrap();
        match &p.files[0].stmts[0] {
            crate::ast::Stmt::ClassDecl { members, .. } => assert_eq!(members.len(), 2),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_return_with_value() {
        let p = parse_str(r#"function five() => { return 5! }!"#).unwrap();
        assert!(matches!(p.files[0].stmts[0], crate::ast::Stmt::FnDecl { .. }));
    }

    #[test]
    fn parses_delete() {
        let p = parse_str(r#"delete 3!"#).unwrap();
        assert!(matches!(p.files[0].stmts[0], crate::ast::Stmt::Delete { .. }));
    }

    #[test]
    fn parses_export_to() {
        let p = parse_str(r#"export add to "main.gom"!"#).unwrap();
        match &p.files[0].stmts[0] {
            crate::ast::Stmt::Export {
                name,
                target_file,
                ..
            } => {
                assert_eq!(name, "add");
                assert_eq!(target_file, "main.gom");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_destructured_signal_binding() {
        let p = parse_str(r#"const var [getScore, setScore] = use(0)!"#).unwrap();
        match &p.files[0].stmts[0] {
            crate::ast::Stmt::Let { target, .. } => match target {
                crate::ast::BindingTarget::Destructure { .. } => {}
                _ => panic!(),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn parses_file_separator() {
        let src = r#"
print("a")!
=====================
print("b")!
"#;
        let p = parse_str(src).unwrap();
        assert_eq!(p.files.len(), 2);
        assert_eq!(p.files[0].stmts.len(), 1);
        assert_eq!(p.files[1].stmts.len(), 1);
    }
}
