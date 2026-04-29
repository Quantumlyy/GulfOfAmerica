//! Expression parser.
//!
//! Pratt-style. The crucial twist is *whitespace-significant precedence* for
//! the standard arithmetic operators:
//!
//! ```text
//! 1 + 2*3   ->  1 + (2*3)   = 7  // `*` is tight, `+` is loose
//! 1+2 * 3   ->  (1+2) * 3   = 9  // `+` is tight, `*` is loose
//! ```
//!
//! Each binary operator's effective binding power is its base precedence plus
//! a "tightness bonus" derived from the whitespace surrounding the operator
//! token. Tight operators (no spaces) get +200; mixed/wide spacing gets less.

use crate::ast::{BinOp, BoolVal, Expr, FnBody, Param, StrPart, TimeKind, UnaryOp};
use crate::diagnostic::{Diagnostic, Label};
use crate::lexer;
use crate::source::{SourceFile, Span};
use crate::token::{StringPart, TokenKind};

use super::Parser;

/// Lowest binding power: don't terminate at any operator.
pub(crate) const BP_NONE: u32 = 0;

impl<'a> Parser<'a> {
    /// Parse a full expression including all operators.
    pub(crate) fn parse_expr(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_expr_inner(BP_NONE, true)
    }

    /// Parse an expression but stop before consuming a top-level `=` (so the
    /// caller can treat it as an assignment statement). Nested expressions
    /// inside parens, brackets, etc. always allow `=`.
    pub(crate) fn parse_expr_no_assign(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_expr_inner(BP_NONE, false)
    }

    pub(crate) fn parse_expr_bp(&mut self, min_bp: u32) -> Result<Expr, Diagnostic> {
        self.parse_expr_inner(min_bp, true)
    }

    fn parse_expr_inner(
        &mut self,
        min_bp: u32,
        allow_assign_eq: bool,
    ) -> Result<Expr, Diagnostic> {
        let mut lhs = self.parse_prefix()?;
        loop {
            // Postfix: member access, indexing, calls.
            if let Some(new_lhs) = self.try_postfix(lhs.clone())? {
                lhs = new_lhs;
                continue;
            }
            // Infix.
            let Some((op, bp_left, bp_right)) = self.peek_binop() else {
                break;
            };
            if !allow_assign_eq && matches!(op, BinOp::EqLoose1) {
                break;
            }
            if bp_left < min_bp {
                break;
            }
            // Consume the operator.
            self.bump();
            // Once we have descended into a sub-expression, `=` is fair game.
            let rhs = self.parse_expr_inner(bp_right, true)?;
            let span = lhs.span().merge(rhs.span());
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_prefix(&mut self) -> Result<Expr, Diagnostic> {
        let tok = self.peek().clone();
        match &tok.kind {
            TokenKind::Number(n) => {
                self.bump();
                let literal = self.file.slice(tok.span).to_string();
                Ok(Expr::Number {
                    value: *n,
                    literal,
                    span: tok.span,
                })
            }
            TokenKind::String(parts) => {
                let parts = parts.clone();
                let span = tok.span;
                let quote_count = self.string_open_quote_count(span);
                self.bump();
                let parts = self.lower_string_parts(parts)?;
                Ok(Expr::String {
                    parts,
                    quote_count,
                    span,
                })
            }
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.bump();
                Ok(Expr::Ident {
                    name,
                    span: tok.span,
                })
            }
            TokenKind::True => {
                self.bump();
                Ok(Expr::Bool {
                    value: BoolVal::True,
                    span: tok.span,
                })
            }
            TokenKind::False => {
                self.bump();
                Ok(Expr::Bool {
                    value: BoolVal::False,
                    span: tok.span,
                })
            }
            TokenKind::Maybe => {
                self.bump();
                Ok(Expr::Bool {
                    value: BoolVal::Maybe,
                    span: tok.span,
                })
            }
            TokenKind::Null => {
                self.bump();
                Ok(Expr::Null { span: tok.span })
            }
            TokenKind::Undefined => {
                self.bump();
                Ok(Expr::Undefined { span: tok.span })
            }
            // `;` is the not-prefix.
            TokenKind::Semi => {
                self.bump();
                let inner = self.parse_expr_bp(BP_PREFIX)?;
                let span = tok.span.merge(inner.span());
                Ok(Expr::Unary {
                    op: UnaryOp::Not,
                    operand: Box::new(inner),
                    span,
                })
            }
            TokenKind::Minus => {
                self.bump();
                let inner = self.parse_expr_bp(BP_PREFIX)?;
                let span = tok.span.merge(inner.span());
                Ok(Expr::Unary {
                    op: UnaryOp::Neg,
                    operand: Box::new(inner),
                    span,
                })
            }
            TokenKind::LParen => {
                // Parenthesised expression. Parens are otherwise transparent
                // here — the parser uses them for grouping.
                self.bump();
                // An empty `()` evaluates to `undefined` so that "parens are
                // whitespace" examples like `(add (3, 2))` work.
                if matches!(self.peek().kind, TokenKind::RParen) {
                    self.bump();
                    return Ok(Expr::Undefined { span: tok.span });
                }
                let inner = self.parse_expr()?;
                self.expect(|k| matches!(k, TokenKind::RParen), "`)`")?;
                Ok(inner)
            }
            TokenKind::LBracket => self.parse_array_lit(),
            TokenKind::LBrace => self.parse_object_lit(),
            TokenKind::New => self.parse_new(),
            TokenKind::Use => self.parse_use(),
            TokenKind::Previous | TokenKind::Next | TokenKind::Current => {
                self.parse_time_expr()
            }
            TokenKind::Await => {
                self.bump();
                let inner = self.parse_expr_bp(BP_PREFIX)?;
                let span = tok.span.merge(inner.span());
                Ok(Expr::Await {
                    target: Box::new(inner),
                    span,
                })
            }
            TokenKind::FnKeyword => self.parse_lambda(),
            TokenKind::Async => {
                self.bump();
                self.parse_lambda_async()
            }
            // Keywords that we accept as identifiers in expression position
            // for quirks like `const const greeting = Luke!` where `Luke` is a
            // bareword string.
            TokenKind::To => {
                self.bump();
                Ok(Expr::Ident {
                    name: "to".into(),
                    span: tok.span,
                })
            }
            other => {
                let actual = other.describe();
                Err(Diagnostic::error(format!(
                    "expected an expression, found {actual}"
                ))
                .with_code("E0101")
                .with_label(Label::primary(
                    tok.span,
                    "expected the start of an expression here",
                )))
            }
        }
    }

    fn try_postfix(&mut self, lhs: Expr) -> Result<Option<Expr>, Diagnostic> {
        let tok = self.peek().clone();
        match tok.kind {
            // `(args)`
            TokenKind::LParen => {
                self.bump();
                let args = self.parse_arg_list(|k| matches!(k, TokenKind::RParen))?;
                let close = self.expect(|k| matches!(k, TokenKind::RParen), "`)`")?;
                let span = lhs.span().merge(close.span);
                Ok(Some(Expr::Call {
                    callee: Box::new(lhs),
                    args,
                    span,
                }))
            }
            // `[index]`
            TokenKind::LBracket => {
                self.bump();
                let index = self.parse_expr()?;
                let close = self.expect(|k| matches!(k, TokenKind::RBracket), "`]`")?;
                let span = lhs.span().merge(close.span);
                Ok(Some(Expr::Index {
                    target: Box::new(lhs),
                    index: Box::new(index),
                    span,
                }))
            }
            // `.name`
            TokenKind::Dot => {
                self.bump();
                let name_tok = self.bump();
                let name = match &name_tok.kind {
                    TokenKind::Ident(s) => s.clone(),
                    TokenKind::Number(_) => self.file.slice(name_tok.span).to_string(),
                    other => {
                        return Err(Diagnostic::error(format!(
                            "expected member name after `.`, found {}",
                            other.describe()
                        ))
                        .with_code("E0102")
                        .with_label(Label::primary(name_tok.span, "expected a name here")));
                    }
                };
                let span = lhs.span().merge(name_tok.span);
                Ok(Some(Expr::Member {
                    target: Box::new(lhs),
                    name,
                    span,
                }))
            }
            _ => Ok(None),
        }
    }

    fn parse_arg_list(
        &mut self,
        is_close: impl Fn(&TokenKind) -> bool,
    ) -> Result<Vec<Expr>, Diagnostic> {
        let mut args = Vec::new();
        if is_close(&self.peek().kind) {
            return Ok(args);
        }
        loop {
            args.push(self.parse_expr()?);
            if !matches!(self.peek().kind, TokenKind::Comma) {
                break;
            }
            self.bump();
        }
        Ok(args)
    }

    fn parse_array_lit(&mut self) -> Result<Expr, Diagnostic> {
        let open = self.bump();
        let mut items = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RBracket) {
            loop {
                items.push(self.parse_expr()?);
                if !matches!(self.peek().kind, TokenKind::Comma) {
                    break;
                }
                self.bump();
                if matches!(self.peek().kind, TokenKind::RBracket) {
                    break;
                }
            }
        }
        let close = self.expect(|k| matches!(k, TokenKind::RBracket), "`]`")?;
        Ok(Expr::Array {
            items,
            span: open.span.merge(close.span),
        })
    }

    fn parse_object_lit(&mut self) -> Result<Expr, Diagnostic> {
        let open = self.bump();
        let mut entries: Vec<(String, Expr)> = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RBrace) {
            loop {
                let key_tok = self.bump();
                let key = match &key_tok.kind {
                    TokenKind::Ident(s) => s.clone(),
                    TokenKind::String(parts) => {
                        // Object keys are simple strings — interpolated keys
                        // are flattened to their literal portions joined.
                        parts
                            .iter()
                            .filter_map(|p| match p {
                                StringPart::Lit(s) => Some(s.clone()),
                                _ => None,
                            })
                            .collect()
                    }
                    other => {
                        return Err(Diagnostic::error(format!(
                            "expected object key, found {}",
                            other.describe()
                        ))
                        .with_code("E0103")
                        .with_label(Label::primary(key_tok.span, "expected a key here")));
                    }
                };
                self.expect(|k| matches!(k, TokenKind::Colon), "`:`")?;
                let value = self.parse_expr()?;
                entries.push((key, value));
                if !matches!(self.peek().kind, TokenKind::Comma) {
                    break;
                }
                self.bump();
                if matches!(self.peek().kind, TokenKind::RBrace) {
                    break;
                }
            }
        }
        let close = self.expect(|k| matches!(k, TokenKind::RBrace), "`}`")?;
        Ok(Expr::Object {
            entries,
            span: open.span.merge(close.span),
        })
    }

    fn parse_new(&mut self) -> Result<Expr, Diagnostic> {
        let new_tok = self.bump();
        let class_expr = self.parse_prefix()?;
        // Optional call.
        let (class_expr, args) = if matches!(self.peek().kind, TokenKind::LParen) {
            self.bump();
            let args = self.parse_arg_list(|k| matches!(k, TokenKind::RParen))?;
            self.expect(|k| matches!(k, TokenKind::RParen), "`)`")?;
            (class_expr, args)
        } else {
            (class_expr, Vec::new())
        };
        let span = new_tok.span.merge(class_expr.span());
        Ok(Expr::New {
            class: Box::new(class_expr),
            args,
            span,
        })
    }

    fn parse_use(&mut self) -> Result<Expr, Diagnostic> {
        let use_tok = self.bump();
        // `use(initial)` — we accept an optional paren wrapper.
        let had_paren = matches!(self.peek().kind, TokenKind::LParen);
        if had_paren {
            self.bump();
        }
        let initial = self.parse_expr()?;
        if had_paren {
            self.expect(|k| matches!(k, TokenKind::RParen), "`)`")?;
        }
        let span = use_tok.span.merge(initial.span());
        Ok(Expr::UseSignal {
            initial: Box::new(initial),
            span,
        })
    }

    fn parse_time_expr(&mut self) -> Result<Expr, Diagnostic> {
        let kw = self.bump();
        let when = match kw.kind {
            TokenKind::Previous => TimeKind::Previous,
            TokenKind::Next => TimeKind::Next,
            TokenKind::Current => TimeKind::Current,
            _ => unreachable!(),
        };
        let target = self.parse_expr_bp(BP_PREFIX)?;
        let span = kw.span.merge(target.span());
        Ok(Expr::Time {
            when,
            target: Box::new(target),
            span,
        })
    }

    fn parse_lambda(&mut self) -> Result<Expr, Diagnostic> {
        // `function (args) => body` — anonymous form. Often the README also
        // shows `(e) => body`, which is an arrow lambda; that is handled in
        // a dedicated parser path elsewhere.
        let kw = self.bump();
        // Parameter list.
        self.expect(|k| matches!(k, TokenKind::LParen), "`(`")?;
        let params = self.parse_param_list()?;
        self.expect(|k| matches!(k, TokenKind::RParen), "`)`")?;
        self.expect(|k| matches!(k, TokenKind::FatArrow), "`=>`")?;
        let body = self.parse_fn_body()?;
        let span = match &body {
            FnBody::Expr(e) => kw.span.merge(e.span()),
            FnBody::Block(b) => kw.span.merge(b.span),
        };
        Ok(Expr::Lambda {
            is_async: false,
            params,
            body: Box::new(body),
            span,
        })
    }

    fn parse_lambda_async(&mut self) -> Result<Expr, Diagnostic> {
        // `async` already consumed.
        // Expect a `function`-family keyword next.
        self.expect(|k| matches!(k, TokenKind::FnKeyword), "function keyword")?;
        self.expect(|k| matches!(k, TokenKind::LParen), "`(`")?;
        let params = self.parse_param_list()?;
        self.expect(|k| matches!(k, TokenKind::RParen), "`)`")?;
        self.expect(|k| matches!(k, TokenKind::FatArrow), "`=>`")?;
        let body = self.parse_fn_body()?;
        let span = match &body {
            FnBody::Expr(e) => e.span(),
            FnBody::Block(b) => b.span,
        };
        Ok(Expr::Lambda {
            is_async: true,
            params,
            body: Box::new(body),
            span,
        })
    }

    pub(crate) fn parse_param_list(&mut self) -> Result<Vec<Param>, Diagnostic> {
        let mut params = Vec::new();
        if matches!(self.peek().kind, TokenKind::RParen) {
            return Ok(params);
        }
        loop {
            let tok = self.bump();
            let name = match &tok.kind {
                TokenKind::Ident(s) => s.clone(),
                other => {
                    return Err(Diagnostic::error(format!(
                        "expected parameter name, found {}",
                        other.describe()
                    ))
                    .with_code("E0104")
                    .with_label(Label::primary(tok.span, "expected a name here")));
                }
            };
            // Optional `: type`.
            let ty = if matches!(self.peek().kind, TokenKind::Colon) {
                self.bump();
                Some(self.parse_type_ref()?)
            } else {
                None
            };
            params.push(Param {
                name,
                ty,
                span: tok.span,
            });
            if !matches!(self.peek().kind, TokenKind::Comma) {
                break;
            }
            self.bump();
        }
        Ok(params)
    }

    pub(crate) fn parse_fn_body(&mut self) -> Result<FnBody, Diagnostic> {
        if matches!(self.peek().kind, TokenKind::LBrace) {
            let block = self.parse_block()?;
            Ok(FnBody::Block(block))
        } else {
            let expr = self.parse_expr_no_assign()?;
            Ok(FnBody::Expr(expr))
        }
    }

    pub(crate) fn parse_type_ref(&mut self) -> Result<crate::ast::TypeRef, Diagnostic> {
        // We parse a small subset and otherwise accept anything that "looks
        // like a type": an identifier, optional `[]`, optional `<...>`.
        let start = self.peek().span;
        let mut end = start;
        // Identifier (name of the type).
        let name_tok = self.expect(
            |k| matches!(k, TokenKind::Ident(_) | TokenKind::To | TokenKind::Maybe),
            "type name",
        )?;
        end = end.merge(name_tok.span);
        // Optional `<...>` (e.g. `RegExp<...>`).
        if matches!(self.peek().kind, TokenKind::LAngle) {
            // Skip-balanced.
            let mut depth = 0i32;
            loop {
                let t = self.bump();
                end = end.merge(t.span);
                match t.kind {
                    TokenKind::LAngle => depth += 1,
                    TokenKind::RAngle => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    TokenKind::Eof => break,
                    _ => {}
                }
            }
        }
        // Optional `[]`.
        while matches!(self.peek().kind, TokenKind::LBracket)
            && matches!(self.peek_at(1).kind, TokenKind::RBracket)
        {
            self.bump();
            let close = self.bump();
            end = end.merge(close.span);
        }
        let span = start.merge(end);
        let source = self.file.slice(span).to_string();
        Ok(crate::ast::TypeRef { source, span })
    }

    fn lower_string_parts(
        &self,
        parts: Vec<StringPart>,
    ) -> Result<Vec<StrPart>, Diagnostic> {
        let mut out = Vec::with_capacity(parts.len());
        for p in parts {
            match p {
                StringPart::Lit(s) => out.push(StrPart::Lit(s)),
                StringPart::Expr { source, span } => {
                    let inner = self.parse_inner_expr(&source, span)?;
                    out.push(StrPart::Expr(inner));
                }
            }
        }
        Ok(out)
    }

    fn parse_inner_expr(&self, source: &str, span: Span) -> Result<Expr, Diagnostic> {
        // Re-lex/parse the interpolated source as an expression. We reuse the
        // outer file's name so diagnostics point to the right place.
        let inner_file = SourceFile::new(self.file.name.clone(), source.to_string());
        let tokens = lexer::lex(&inner_file).map_err(|d| shift_diag(d, span.start))?;
        let mut p = Parser::new(&inner_file, tokens);
        let expr = p.parse_expr().map_err(|d| shift_diag(d, span.start))?;
        // Ensure no trailing tokens.
        if !p.at_eof() {
            let extra = p.peek().span;
            let absolute = Span::new(extra.start + span.start, extra.end + span.start);
            return Err(Diagnostic::error("unexpected tokens in interpolation")
                .with_code("E0105")
                .with_label(Label::primary(absolute, "unexpected here")));
        }
        Ok(shift_expr(expr, span.start))
    }

    fn string_open_quote_count(&self, span: Span) -> usize {
        let s = self.file.slice(span);
        let first = s.chars().next();
        let Some(q) = first else { return 0 };
        s.chars().take_while(|c| *c == q).count()
    }

    /// Peek the next token as a binary operator, returning `(op, bp_left,
    /// bp_right)` for Pratt parsing. The left/right binding powers are equal
    /// for left-associative operators; right-associative operators bias one
    /// side.
    fn peek_binop(&self) -> Option<(BinOp, u32, u32)> {
        let tok = self.peek();
        let (op, base) = match &tok.kind {
            TokenKind::Plus => (BinOp::Add, 100),
            TokenKind::Minus => (BinOp::Sub, 100),
            TokenKind::Star => (BinOp::Mul, 110),
            TokenKind::Slash => (BinOp::Div, 110),
            TokenKind::Percent => (BinOp::Mod, 110),
            TokenKind::Eq(1) => (BinOp::EqLoose1, 50),
            TokenKind::Eq(2) => (BinOp::EqLoose2, 50),
            TokenKind::Eq(3) => (BinOp::EqStrict, 50),
            TokenKind::Eq(4) => (BinOp::EqExtreme, 50),
            TokenKind::NotEq => (BinOp::NotEq, 50),
            TokenKind::LAngle => (BinOp::Lt, 60),
            TokenKind::RAngle => (BinOp::Gt, 60),
            TokenKind::LtEq => (BinOp::LtEq, 60),
            TokenKind::GtEq => (BinOp::GtEq, 60),
            _ => return None,
        };
        // Whitespace tightness bonus: tighter operators bind tighter.
        let lspace = u32::from(tok.leading_space);
        let rspace = u32::from(tok.trailing_space);
        let max_space = lspace.max(rspace);
        // Tightness 0..=2:
        //   max_space == 0 -> tightness 2 (no whitespace either side)
        //   max_space == 1 -> tightness 0 (some whitespace)
        let tightness = if max_space == 0 { 2 } else { 0 };
        let effective = base + tightness * 200;
        // All operators here are left-associative; bp_right > bp_left to bind
        // left-to-right.
        Some((op, effective, effective + 1))
    }
}

/// Prefix unary operator binding power. Higher than any binop tightness so
/// that `;false`, `-x`, `previous x` always bind to the immediately-following
/// expression.
const BP_PREFIX: u32 = 1_000;

fn shift_diag(mut d: Diagnostic, by: usize) -> Diagnostic {
    for label in &mut d.labels {
        label.span.start += by;
        label.span.end += by;
    }
    d
}

fn shift_expr(expr: Expr, by: usize) -> Expr {
    fn shift_span(s: Span, by: usize) -> Span {
        Span::new(s.start + by, s.end + by)
    }
    match expr {
        Expr::Number {
            value,
            literal,
            span,
        } => Expr::Number {
            value,
            literal,
            span: shift_span(span, by),
        },
        Expr::String {
            parts,
            quote_count,
            span,
        } => Expr::String {
            parts: parts
                .into_iter()
                .map(|p| match p {
                    StrPart::Lit(s) => StrPart::Lit(s),
                    StrPart::Expr(e) => StrPart::Expr(shift_expr(e, by)),
                })
                .collect(),
            quote_count,
            span: shift_span(span, by),
        },
        Expr::Bool { value, span } => Expr::Bool {
            value,
            span: shift_span(span, by),
        },
        Expr::Undefined { span } => Expr::Undefined {
            span: shift_span(span, by),
        },
        Expr::Null { span } => Expr::Null {
            span: shift_span(span, by),
        },
        Expr::Ident { name, span } => Expr::Ident {
            name,
            span: shift_span(span, by),
        },
        Expr::Array { items, span } => Expr::Array {
            items: items.into_iter().map(|e| shift_expr(e, by)).collect(),
            span: shift_span(span, by),
        },
        Expr::Object { entries, span } => Expr::Object {
            entries: entries
                .into_iter()
                .map(|(k, e)| (k, shift_expr(e, by)))
                .collect(),
            span: shift_span(span, by),
        },
        Expr::Index {
            target,
            index,
            span,
        } => Expr::Index {
            target: Box::new(shift_expr(*target, by)),
            index: Box::new(shift_expr(*index, by)),
            span: shift_span(span, by),
        },
        Expr::Member {
            target,
            name,
            span,
        } => Expr::Member {
            target: Box::new(shift_expr(*target, by)),
            name,
            span: shift_span(span, by),
        },
        Expr::Call {
            callee,
            args,
            span,
        } => Expr::Call {
            callee: Box::new(shift_expr(*callee, by)),
            args: args.into_iter().map(|e| shift_expr(e, by)).collect(),
            span: shift_span(span, by),
        },
        Expr::Unary {
            op,
            operand,
            span,
        } => Expr::Unary {
            op,
            operand: Box::new(shift_expr(*operand, by)),
            span: shift_span(span, by),
        },
        Expr::Binary {
            op,
            lhs,
            rhs,
            span,
        } => Expr::Binary {
            op,
            lhs: Box::new(shift_expr(*lhs, by)),
            rhs: Box::new(shift_expr(*rhs, by)),
            span: shift_span(span, by),
        },
        Expr::Time {
            when,
            target,
            span,
        } => Expr::Time {
            when,
            target: Box::new(shift_expr(*target, by)),
            span: shift_span(span, by),
        },
        Expr::Await { target, span } => Expr::Await {
            target: Box::new(shift_expr(*target, by)),
            span: shift_span(span, by),
        },
        Expr::New { class, args, span } => Expr::New {
            class: Box::new(shift_expr(*class, by)),
            args: args.into_iter().map(|e| shift_expr(e, by)).collect(),
            span: shift_span(span, by),
        },
        Expr::Lambda {
            is_async,
            params,
            body,
            span,
        } => Expr::Lambda {
            is_async,
            params,
            body,
            span: shift_span(span, by),
        },
        Expr::UseSignal { initial, span } => Expr::UseSignal {
            initial: Box::new(shift_expr(*initial, by)),
            span: shift_span(span, by),
        },
    }
}

#[allow(unused)]
pub(crate) const BP_POSTFIX: u32 = 2_000;
