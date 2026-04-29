//! Statement parser.
//!
//! Implemented incrementally:
//!
//! 1. **Terminators & expression statements** — every statement ends with a
//!    run of `!` (with overload priority equal to the run length) or a run
//!    of `?` (debug print). At top level, a bare identifier-or-path followed
//!    by `=` is an assignment statement; otherwise it is an expression
//!    statement.
//! 2. *Declarations* and *control flow* land in subsequent passes.

use crate::ast::{
    BindingTarget, Block, DeclKind, Expr, File, Lifetime, Param, Program, Stmt,
};
use crate::diagnostic::{Diagnostic, Label};
use crate::source::Span;
use crate::token::TokenKind;

use super::Parser;

/// Result of parsing a statement-ending token run.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Terminator {
    pub bangs: Option<u8>,
    pub questions: Option<u8>,
    /// Negative-priority count from `¡` runs.
    pub inverted_bangs: u8,
    pub span: Span,
}

impl Terminator {
    /// Resolve the overload priority of a declaration that ends with this
    /// terminator: a `!!` run is +2, `¡¡` is -2, `?` and unmarked are 0.
    pub fn priority(self) -> i32 {
        let pos = self.bangs.unwrap_or(0) as i32;
        let neg = self.inverted_bangs as i32;
        pos - neg
    }
}

impl<'a> Parser<'a> {
    pub(crate) fn parse_program(&mut self) -> Result<Program, Diagnostic> {
        let mut files = Vec::new();
        let mut current_name: Option<String> = None;
        loop {
            // Optional file separator at the very start (with optional name).
            if matches!(self.peek().kind, TokenKind::FileSeparator) {
                let name = self.parse_file_separator_name()?;
                if !files.is_empty() || current_name.is_some() {
                    // Already inside a file; close it before starting a new one.
                }
                current_name = name;
                continue;
            }
            if self.at_eof() {
                break;
            }
            let mut stmts = Vec::new();
            while !self.at_eof()
                && !matches!(self.peek().kind, TokenKind::FileSeparator)
            {
                stmts.push(self.parse_stmt()?);
            }
            files.push(File {
                name: current_name.take(),
                stmts,
            });
        }
        if files.is_empty() {
            files.push(File {
                name: None,
                stmts: Vec::new(),
            });
        }
        Ok(Program { files })
    }

    /// Consume a `=====` file-separator token plus any optional `name.gom`
    /// label that may follow it (e.g. `===== add.gom ===`).
    fn parse_file_separator_name(&mut self) -> Result<Option<String>, Diagnostic> {
        // Consume the leading FileSeparator token.
        self.bump();
        // Optional identifier-with-dot like `add.gom`.
        let mut name: Option<String> = None;
        if let TokenKind::Ident(s) = &self.peek().kind {
            let mut buf = s.clone();
            self.bump();
            // Allow `name.gom` extension.
            if matches!(self.peek().kind, TokenKind::Dot) {
                self.bump();
                if let TokenKind::Ident(ext) = &self.peek().kind {
                    buf.push('.');
                    buf.push_str(ext);
                    self.bump();
                }
            }
            name = Some(buf);
        }
        // Optional trailing FileSeparator (e.g. `===== name.gom =====`).
        if matches!(self.peek().kind, TokenKind::FileSeparator) {
            self.bump();
        }
        Ok(name)
    }

    pub(crate) fn parse_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        let start = self.peek().span;
        match self.peek().kind.clone() {
            TokenKind::Const | TokenKind::Var => self.parse_let(start),
            TokenKind::If => self.parse_if(start),
            TokenKind::When => self.parse_when(start),
            TokenKind::Class | TokenKind::ClassName => self.parse_class_decl(start),
            TokenKind::FnKeyword => self.parse_fn_decl(start, false),
            TokenKind::Async => {
                self.bump();
                self.parse_fn_decl(start, true)
            }
            TokenKind::Return => self.parse_return(start),
            TokenKind::Delete => self.parse_delete(start),
            TokenKind::Export => self.parse_export(start),
            TokenKind::Ident(ref name) if name == "import" => {
                self.parse_import(start)
            }
            TokenKind::Ident(ref name) if name == "reverse" => {
                let span = self.bump().span;
                let _ = self.parse_terminator()?;
                Ok(Stmt::Reverse { span })
            }
            _ => self.parse_expr_or_assign(start),
        }
    }

    /// Parse the run of `!` / `?` / `¡` at the end of a statement and return
    /// it as a single [`Terminator`]. Multiple kinds in sequence (e.g. `!!?`)
    /// are accepted and merged.
    pub(crate) fn parse_terminator(&mut self) -> Result<Terminator, Diagnostic> {
        let mut bangs: Option<u8> = None;
        let mut questions: Option<u8> = None;
        let mut inverted: u8 = 0;
        let start = self.peek().span;
        let mut last = start;
        let mut consumed_anything = false;
        loop {
            match self.peek().kind {
                TokenKind::Bang(n) => {
                    last = self.bump().span;
                    bangs = Some(bangs.unwrap_or(0).saturating_add(n));
                    consumed_anything = true;
                }
                TokenKind::Question(n) => {
                    last = self.bump().span;
                    questions = Some(questions.unwrap_or(0).saturating_add(n));
                    consumed_anything = true;
                }
                TokenKind::InvertedBang(n) => {
                    last = self.bump().span;
                    inverted = inverted.saturating_add(n);
                    consumed_anything = true;
                }
                _ => break,
            }
        }
        if !consumed_anything {
            let actual = self.peek().kind.describe();
            let span = self.peek().span;
            return Err(Diagnostic::error(format!(
                "expected `!` or `?` at end of statement, found {actual}"
            ))
            .with_code("E0110")
            .with_label(Label::primary(span, "expected `!` here"))
            .with_note(
                "every statement in Gulf of Mexico ends with at least one `!` (or `?` for \
                 a debug print). The number of `!`s controls overload priority.",
            ));
        }
        Ok(Terminator {
            bangs,
            questions,
            inverted_bangs: inverted,
            span: start.merge(last),
        })
    }

    fn parse_expr_or_assign(&mut self, start: Span) -> Result<Stmt, Diagnostic> {
        let lhs = self.parse_expr_no_assign()?;
        if matches!(self.peek().kind, TokenKind::Eq(1)) {
            self.bump();
            let value = self.parse_expr()?;
            let term = self.parse_terminator()?;
            let span = start.merge(term.span);
            Ok(Stmt::Assign {
                target: lhs,
                value,
                priority: term.priority(),
                span,
            })
        } else if matches!(
            self.peek().kind,
            TokenKind::PlusEq | TokenKind::MinusEq | TokenKind::StarEq | TokenKind::SlashEq
        ) {
            // Compound assignment desugars to `target = target OP value`.
            let op_tok = self.bump();
            let value = self.parse_expr()?;
            let term = self.parse_terminator()?;
            let op = match op_tok.kind {
                TokenKind::PlusEq => crate::ast::BinOp::Add,
                TokenKind::MinusEq => crate::ast::BinOp::Sub,
                TokenKind::StarEq => crate::ast::BinOp::Mul,
                TokenKind::SlashEq => crate::ast::BinOp::Div,
                _ => unreachable!(),
            };
            let bin_span = lhs.span().merge(value.span());
            let new_value = Expr::Binary {
                op,
                lhs: Box::new(lhs.clone()),
                rhs: Box::new(value),
                span: bin_span,
            };
            let span = start.merge(term.span);
            Ok(Stmt::Assign {
                target: lhs,
                value: new_value,
                priority: term.priority(),
                span,
            })
        } else {
            let term = self.parse_terminator()?;
            let span = start.merge(term.span);
            Ok(Stmt::Expr {
                expr: lhs,
                bangs: term.bangs,
                questions: term.questions,
                span,
            })
        }
    }

    pub(crate) fn parse_block(&mut self) -> Result<Block, Diagnostic> {
        let open = self.expect(|k| matches!(k, TokenKind::LBrace), "`{`")?;
        let mut stmts = Vec::new();
        while !matches!(self.peek().kind, TokenKind::RBrace | TokenKind::Eof) {
            stmts.push(self.parse_stmt()?);
        }
        let close = self.expect(|k| matches!(k, TokenKind::RBrace), "`}`")?;
        Ok(Block {
            stmts,
            span: open.span.merge(close.span),
        })
    }

    // -----------------------------------------------------------------
    // Stubs filled in by later commits — keep the compiler happy.
    // -----------------------------------------------------------------

    fn parse_let(&mut self, start: Span) -> Result<Stmt, Diagnostic> {
        // Count leading `const` / `var` keywords. Two are required (const var,
        // var const, etc.); a third `const` (only) makes a globally-eternal
        // binding.
        let first = self.bump();
        let second = match self.peek().kind {
            TokenKind::Const | TokenKind::Var => self.bump(),
            ref other => {
                let span = self.peek().span;
                return Err(Diagnostic::error(format!(
                    "expected `const` or `var` after `{}`, found {}",
                    if matches!(first.kind, TokenKind::Const) { "const" } else { "var" },
                    other.describe()
                ))
                .with_code("E0120")
                .with_label(Label::primary(span, "expected `const` or `var` here"))
                .with_note(
                    "declarations come in pairs: `const const`, `const var`, `var const`, or \
                     `var var`.",
                ));
            }
        };
        // Optional third `const` (only legal as `const const const`).
        let mut const_depth: u8 = 0;
        if matches!(first.kind, TokenKind::Const) {
            const_depth += 1;
        }
        if matches!(second.kind, TokenKind::Const) {
            const_depth += 1;
        }
        let mut third_const_span: Option<Span> = None;
        if matches!(self.peek().kind, TokenKind::Const)
            && matches!(first.kind, TokenKind::Const)
            && matches!(second.kind, TokenKind::Const)
        {
            third_const_span = Some(self.peek().span);
            self.bump();
            const_depth = 3;
        }
        let decl = match (&first.kind, &second.kind) {
            (TokenKind::Const, TokenKind::Const) => DeclKind::ConstConst,
            (TokenKind::Const, TokenKind::Var) => DeclKind::ConstVar,
            (TokenKind::Var, TokenKind::Const) => DeclKind::VarConst,
            (TokenKind::Var, TokenKind::Var) => DeclKind::VarVar,
            _ => unreachable!(),
        };

        // Binding target: identifier, number-as-name, or `[ ... ]` destructure.
        let target = self.parse_binding_target()?;
        // Optional lifetime annotation.
        let lifetime = if matches!(self.peek().kind, TokenKind::LAngle) {
            Some(self.parse_lifetime()?)
        } else {
            None
        };
        // Optional type annotation.
        let ty = if matches!(self.peek().kind, TokenKind::Colon) {
            self.bump();
            Some(self.parse_type_ref()?)
        } else {
            None
        };
        self.expect(|k| matches!(k, TokenKind::Eq(1)), "`=`")?;
        let value = self.parse_expr()?;
        let term = self.parse_terminator()?;
        let span = start.merge(term.span);
        let _ = third_const_span;
        Ok(Stmt::Let {
            decl,
            const_depth,
            target,
            ty,
            lifetime,
            value,
            priority: term.priority(),
            span,
        })
    }

    fn parse_binding_target(&mut self) -> Result<BindingTarget, Diagnostic> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::Ident(name) => {
                self.bump();
                Ok(BindingTarget::Ident {
                    name,
                    span: tok.span,
                })
            }
            // `const const 5 = 4!`
            TokenKind::Number(_) => {
                self.bump();
                let name = self.file.slice(tok.span).to_string();
                Ok(BindingTarget::Ident {
                    name,
                    span: tok.span,
                })
            }
            TokenKind::LBracket => {
                let pat = self.parse_destructure_pattern()?;
                Ok(BindingTarget::Destructure {
                    pattern: pat,
                    span: tok.span,
                })
            }
            other => {
                Err(Diagnostic::error(format!(
                    "expected a binding name, found {}",
                    other.describe()
                ))
                .with_code("E0121")
                .with_label(Label::primary(tok.span, "expected an identifier here")))
            }
        }
    }

    fn parse_destructure_pattern(
        &mut self,
    ) -> Result<crate::ast::DestructurePattern, Diagnostic> {
        let open = self.expect(|k| matches!(k, TokenKind::LBracket), "`[`")?;
        let mut items = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RBracket) {
            loop {
                let item = match self.peek().kind {
                    TokenKind::LBracket => self.parse_destructure_pattern()?,
                    TokenKind::Ident(_) => {
                        let tok = self.bump();
                        let TokenKind::Ident(name) = tok.kind else {
                            unreachable!()
                        };
                        crate::ast::DestructurePattern::Ident(name, tok.span)
                    }
                    ref other => {
                        return Err(Diagnostic::error(format!(
                            "expected a name in destructuring pattern, found {}",
                            other.describe()
                        ))
                        .with_code("E0122")
                        .with_label(Label::primary(self.peek().span, "expected a name here")));
                    }
                };
                items.push(item);
                if !matches!(self.peek().kind, TokenKind::Comma) {
                    break;
                }
                self.bump();
            }
        }
        let close = self.expect(|k| matches!(k, TokenKind::RBracket), "`]`")?;
        Ok(crate::ast::DestructurePattern::List(
            items,
            open.span.merge(close.span),
        ))
    }

    fn parse_lifetime(&mut self) -> Result<Lifetime, Diagnostic> {
        let _open = self.expect(|k| matches!(k, TokenKind::LAngle), "`<`")?;
        // `<-N>`, `<N>`, `<Ns>`, `<Infinity>`.
        let negative = matches!(self.peek().kind, TokenKind::Minus);
        if negative {
            self.bump();
        }
        let lifetime = match self.peek().kind.clone() {
            TokenKind::Number(n) => {
                let tok = self.bump();
                // Optional `s` for seconds.
                if let TokenKind::Ident(s) = &self.peek().kind {
                    if s == "s" || s == "sec" || s == "secs" || s == "seconds" {
                        self.bump();
                        return self.finish_lifetime(Lifetime::Seconds(if negative {
                            -n
                        } else {
                            n
                        }));
                    }
                }
                let _ = tok;
                let lines = if negative { -(n as i64) } else { n as i64 };
                self.finish_lifetime(Lifetime::Lines(lines))?
            }
            TokenKind::Ident(name) if name == "Infinity" || name == "infinity" => {
                self.bump();
                self.finish_lifetime(Lifetime::Infinity)?
            }
            ref other => {
                return Err(Diagnostic::error(format!(
                    "expected a lifetime (a number, `Ns`, or `Infinity`), found {}",
                    other.describe()
                ))
                .with_code("E0123")
                .with_label(Label::primary(self.peek().span, "expected a lifetime here")));
            }
        };
        Ok(lifetime)
    }

    fn finish_lifetime(&mut self, lt: Lifetime) -> Result<Lifetime, Diagnostic> {
        self.expect(|k| matches!(k, TokenKind::RAngle), "`>`")?;
        Ok(lt)
    }
    fn parse_if(&mut self, start: Span) -> Result<Stmt, Diagnostic> {
        self.bump(); // `if`
        // The condition's parens are optional; we accept either form. Inside
        // the parens, `=` is comparison (not assignment) — and that is what
        // the parens-aware expression parser already does.
        let had_paren = matches!(self.peek().kind, TokenKind::LParen);
        if had_paren {
            self.bump();
        }
        let cond = self.parse_expr()?;
        if had_paren {
            self.expect(|k| matches!(k, TokenKind::RParen), "`)`")?;
        }
        let then_block = self.parse_block()?;
        let else_block = if matches!(self.peek().kind, TokenKind::Else) {
            self.bump();
            if matches!(self.peek().kind, TokenKind::If) {
                // `else if` — wrap as a single-statement else block.
                let nested_start = self.peek().span;
                let nested = self.parse_if(nested_start)?;
                let span = nested.span_helper();
                Some(Block {
                    stmts: vec![nested],
                    span,
                })
            } else {
                Some(self.parse_block()?)
            }
        } else {
            None
        };
        let span = start.merge(
            else_block
                .as_ref()
                .map(|b| b.span)
                .unwrap_or(then_block.span),
        );
        Ok(Stmt::If {
            cond,
            then_block,
            else_block,
            span,
        })
    }

    fn parse_when(&mut self, start: Span) -> Result<Stmt, Diagnostic> {
        self.bump(); // `when`
        let had_paren = matches!(self.peek().kind, TokenKind::LParen);
        if had_paren {
            self.bump();
        }
        let cond = self.parse_expr()?;
        if had_paren {
            self.expect(|k| matches!(k, TokenKind::RParen), "`)`")?;
        }
        let block = self.parse_block()?;
        Ok(Stmt::When {
            cond,
            block: block.clone(),
            span: start.merge(block.span),
        })
    }

    fn parse_return(&mut self, start: Span) -> Result<Stmt, Diagnostic> {
        self.bump(); // `return`
        // A bare `return!` returns undefined.
        let value = if matches!(
            self.peek().kind,
            TokenKind::Bang(_) | TokenKind::Question(_) | TokenKind::InvertedBang(_)
        ) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        let term = self.parse_terminator()?;
        Ok(Stmt::Return {
            value,
            span: start.merge(term.span),
        })
    }

    fn parse_delete(&mut self, start: Span) -> Result<Stmt, Diagnostic> {
        self.bump(); // `delete`
        let target = self.parse_expr()?;
        let term = self.parse_terminator()?;
        Ok(Stmt::Delete {
            target,
            span: start.merge(term.span),
        })
    }

    fn parse_export(&mut self, start: Span) -> Result<Stmt, Diagnostic> {
        self.bump(); // `export`
        let name_tok = self.expect(
            |k| matches!(k, TokenKind::Ident(_)),
            "name to export",
        )?;
        let TokenKind::Ident(name) = name_tok.kind else {
            unreachable!()
        };
        self.expect(|k| matches!(k, TokenKind::To), "`to`")?;
        let path_tok = self.bump();
        let target_file = match path_tok.kind {
            TokenKind::String(parts) => parts
                .into_iter()
                .filter_map(|p| match p {
                    crate::token::StringPart::Lit(s) => Some(s),
                    _ => None,
                })
                .collect::<String>(),
            ref other => {
                return Err(Diagnostic::error(format!(
                    "expected a string filename after `to`, found {}",
                    other.describe()
                ))
                .with_code("E0130")
                .with_label(Label::primary(path_tok.span, "expected a filename here")));
            }
        };
        let term = self.parse_terminator()?;
        Ok(Stmt::Export {
            name,
            target_file,
            span: start.merge(term.span),
        })
    }

    fn parse_import(&mut self, start: Span) -> Result<Stmt, Diagnostic> {
        self.bump(); // `import`
        let name_tok = self.expect(
            |k| matches!(k, TokenKind::Ident(_)),
            "name to import",
        )?;
        let TokenKind::Ident(name) = name_tok.kind else {
            unreachable!()
        };
        let term = self.parse_terminator()?;
        Ok(Stmt::Import {
            name,
            span: start.merge(term.span),
        })
    }

    fn parse_fn_decl(&mut self, start: Span, is_async: bool) -> Result<Stmt, Diagnostic> {
        self.bump(); // function-keyword
        let name_tok = self.expect(
            |k| matches!(k, TokenKind::Ident(_)),
            "function name",
        )?;
        let TokenKind::Ident(name) = name_tok.kind else {
            unreachable!()
        };
        // Parens around params are optional (parens are whitespace).
        let had_paren = matches!(self.peek().kind, TokenKind::LParen);
        if had_paren {
            self.bump();
        }
        let params = self.parse_param_list()?;
        if had_paren {
            self.expect(|k| matches!(k, TokenKind::RParen), "`)`")?;
        }
        self.expect(|k| matches!(k, TokenKind::FatArrow), "`=>`")?;
        let body = self.parse_fn_body()?;
        // Body may have its own terminator if it was an `expr-bodied` function.
        // In that case parse_fn_body already left us pointing at `!`.
        let term = self.parse_terminator()?;
        let span = start.merge(term.span);
        Ok(Stmt::FnDecl {
            is_async,
            name,
            params,
            body,
            priority: term.priority(),
            span,
        })
    }

    fn parse_class_decl(&mut self, start: Span) -> Result<Stmt, Diagnostic> {
        self.bump(); // `class` or `className`
        let name_tok = self.expect(
            |k| matches!(k, TokenKind::Ident(_)),
            "class name",
        )?;
        let TokenKind::Ident(name) = name_tok.kind else {
            unreachable!()
        };
        let body_open = self.expect(|k| matches!(k, TokenKind::LBrace), "`{`")?;
        let mut members = Vec::new();
        while !matches!(self.peek().kind, TokenKind::RBrace | TokenKind::Eof) {
            members.push(self.parse_class_member()?);
        }
        let body_close = self.expect(|k| matches!(k, TokenKind::RBrace), "`}`")?;
        Ok(Stmt::ClassDecl {
            name,
            members,
            span: start.merge(body_open.span.merge(body_close.span)),
        })
    }

    fn parse_class_member(&mut self) -> Result<crate::ast::ClassMember, Diagnostic> {
        match self.peek().kind {
            // `function method(...) => body!` (any "function" prefix).
            TokenKind::FnKeyword => {
                let start = self.peek().span;
                self.bump();
                let name_tok = self.expect(
                    |k| matches!(k, TokenKind::Ident(_)),
                    "method name",
                )?;
                let TokenKind::Ident(name) = name_tok.kind else {
                    unreachable!()
                };
                let had_paren = matches!(self.peek().kind, TokenKind::LParen);
                if had_paren {
                    self.bump();
                }
                let params = self.parse_param_list()?;
                if had_paren {
                    self.expect(|k| matches!(k, TokenKind::RParen), "`)`")?;
                }
                self.expect(|k| matches!(k, TokenKind::FatArrow), "`=>`")?;
                let body = self.parse_fn_body()?;
                let term = self.parse_terminator()?;
                let span = start.merge(term.span);
                Ok(crate::ast::ClassMember::Method {
                    is_async: false,
                    name,
                    params,
                    body,
                    span,
                })
            }
            TokenKind::Async => {
                let start = self.peek().span;
                self.bump();
                self.expect(|k| matches!(k, TokenKind::FnKeyword), "function keyword")?;
                let name_tok = self.expect(
                    |k| matches!(k, TokenKind::Ident(_)),
                    "method name",
                )?;
                let TokenKind::Ident(name) = name_tok.kind else {
                    unreachable!()
                };
                let had_paren = matches!(self.peek().kind, TokenKind::LParen);
                if had_paren {
                    self.bump();
                }
                let params = self.parse_param_list()?;
                if had_paren {
                    self.expect(|k| matches!(k, TokenKind::RParen), "`)`")?;
                }
                self.expect(|k| matches!(k, TokenKind::FatArrow), "`=>`")?;
                let body = self.parse_fn_body()?;
                let term = self.parse_terminator()?;
                let span = start.merge(term.span);
                Ok(crate::ast::ClassMember::Method {
                    is_async: true,
                    name,
                    params,
                    body,
                    span,
                })
            }
            // `const var name = value!` field declaration.
            TokenKind::Const | TokenKind::Var => {
                let start = self.peek().span;
                let stmt = self.parse_let(start)?;
                match stmt {
                    Stmt::Let {
                        decl,
                        target,
                        value,
                        span,
                        ..
                    } => match target {
                        BindingTarget::Ident { name, .. } => {
                            Ok(crate::ast::ClassMember::Field {
                                decl,
                                name,
                                value,
                                span,
                            })
                        }
                        _ => Err(Diagnostic::error(
                            "destructuring patterns are not allowed for class fields",
                        )
                        .with_code("E0140")
                        .with_label(Label::primary(span, "destructure not allowed here"))),
                    },
                    _ => unreachable!(),
                }
            }
            ref other => {
                let span = self.peek().span;
                Err(Diagnostic::error(format!(
                    "expected a class member (field or method), found {}",
                    other.describe()
                ))
                .with_code("E0141")
                .with_label(Label::primary(span, "expected a member here")))
            }
        }
    }
}

/// Trait helper: every `Stmt` knows its own span.
impl Stmt {
    fn span_helper(&self) -> Span {
        match self {
            Stmt::Let { span, .. }
            | Stmt::Expr { span, .. }
            | Stmt::Assign { span, .. }
            | Stmt::If { span, .. }
            | Stmt::When { span, .. }
            | Stmt::FnDecl { span, .. }
            | Stmt::ClassDecl { span, .. }
            | Stmt::Return { span, .. }
            | Stmt::Delete { span, .. }
            | Stmt::Export { span, .. }
            | Stmt::Import { span, .. }
            | Stmt::Reverse { span } => *span,
        }
    }
}

// Suppress unused-warnings for items used by later commits.
#[allow(dead_code)]
fn _used_later(_: BindingTarget, _: DeclKind, _: Lifetime, _: Param) {}
