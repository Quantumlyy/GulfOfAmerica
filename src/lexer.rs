//! Lexer.
//!
//! Notable behaviours:
//!
//! * Run-length tokens. `!`, `?`, `=`, and `¡` collapse runs of the same
//!   character into a single token whose payload is the run length. Five or
//!   more `=`s become a [`TokenKind::FileSeparator`].
//! * Multi-quote strings. The opening quote run length is recorded, and the
//!   string ends at the first matching run of the same character.
//! * Currency interpolation. Inside a string, `${expr}`, `£{expr}`, `¥{expr}`,
//!   `{expr}€`, and the Cape Verdean escudo form `{a$b}` all introduce
//!   interpolated expressions. Interpolated source is captured verbatim and
//!   re-parsed by the parser later.
//! * Whitespace tracking. Each token records whether at least one whitespace
//!   character appears immediately before/after it on the same line, which
//!   the parser uses for whitespace-significant arithmetic precedence.
//! * Parentheses are emitted as ordinary tokens. The parser is responsible
//!   for the language's "parens are whitespace" semantics.

use crate::diagnostic::{Diagnostic, Label};
use crate::source::{SourceFile, Span};
use crate::token::{StringPart, Token, TokenKind};

pub fn lex(file: &SourceFile) -> Result<Vec<Token>, Diagnostic> {
    let mut lx = Lexer::new(&file.text);
    lx.run()?;
    Ok(lx.tokens)
}

struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    tokens: Vec<Token>,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            tokens: Vec::new(),
        }
    }

    fn run(&mut self) -> Result<(), Diagnostic> {
        loop {
            let leading_space = self.skip_trivia();
            if self.pos >= self.bytes.len() {
                let span = Span::point(self.pos);
                self.tokens.push(Token {
                    kind: TokenKind::Eof,
                    span,
                    leading_space,
                    trailing_space: false,
                });
                break;
            }
            let start = self.pos;
            let kind = self.lex_one()?;
            let span = Span::new(start, self.pos);
            // A token has trailing space if the next byte is a whitespace
            // character (excluding newline). The next iteration will set the
            // following token's leading_space too.
            let trailing_space = self.peek_byte().is_some_and(is_inline_space);
            self.tokens.push(Token {
                kind,
                span,
                leading_space,
                trailing_space,
            });
            // EOF reached?
            if self.peek_byte().is_none() {
                let span = Span::point(self.pos);
                self.tokens.push(Token {
                    kind: TokenKind::Eof,
                    span,
                    leading_space: false,
                    trailing_space: false,
                });
                break;
            }
        }
        Ok(())
    }

    fn lex_one(&mut self) -> Result<TokenKind, Diagnostic> {
        let c = self.peek_char().expect("non-empty checked above");
        match c {
            '(' => {
                self.advance(c);
                Ok(TokenKind::LParen)
            }
            ')' => {
                self.advance(c);
                Ok(TokenKind::RParen)
            }
            '{' => {
                self.advance(c);
                Ok(TokenKind::LBrace)
            }
            '}' => {
                self.advance(c);
                Ok(TokenKind::RBrace)
            }
            '[' => {
                self.advance(c);
                Ok(TokenKind::LBracket)
            }
            ']' => {
                self.advance(c);
                Ok(TokenKind::RBracket)
            }
            ',' => {
                self.advance(c);
                Ok(TokenKind::Comma)
            }
            ':' => {
                self.advance(c);
                Ok(TokenKind::Colon)
            }
            ';' => {
                self.advance(c);
                Ok(TokenKind::Semi)
            }
            '+' => {
                self.advance(c);
                if self.peek_char() == Some('+') {
                    self.advance('+');
                    Ok(TokenKind::PlusPlus)
                } else if self.peek_char() == Some('=') {
                    self.advance('=');
                    Ok(TokenKind::PlusEq)
                } else {
                    Ok(TokenKind::Plus)
                }
            }
            '-' => {
                self.advance(c);
                if self.peek_char() == Some('-') {
                    self.advance('-');
                    Ok(TokenKind::MinusMinus)
                } else if self.peek_char() == Some('=') {
                    self.advance('=');
                    Ok(TokenKind::MinusEq)
                } else {
                    Ok(TokenKind::Minus)
                }
            }
            '*' => {
                self.advance(c);
                if self.peek_char() == Some('=') {
                    self.advance('=');
                    Ok(TokenKind::StarEq)
                } else {
                    Ok(TokenKind::Star)
                }
            }
            '/' => {
                self.advance(c);
                if self.peek_char() == Some('=') {
                    self.advance('=');
                    Ok(TokenKind::SlashEq)
                } else {
                    Ok(TokenKind::Slash)
                }
            }
            '%' => {
                self.advance(c);
                Ok(TokenKind::Percent)
            }
            '^' => {
                self.advance(c);
                Ok(TokenKind::Caret)
            }
            '.' if !matches!(self.peek_byte_at(1), Some(b'0'..=b'9')) => {
                self.advance(c);
                Ok(TokenKind::Dot)
            }
            '<' => {
                self.advance(c);
                if self.peek_char() == Some('=') {
                    self.advance('=');
                    Ok(TokenKind::LtEq)
                } else {
                    Ok(TokenKind::LAngle)
                }
            }
            '>' => {
                self.advance(c);
                if self.peek_char() == Some('=') {
                    self.advance('=');
                    Ok(TokenKind::GtEq)
                } else {
                    Ok(TokenKind::RAngle)
                }
            }
            '=' => self.lex_equals(),
            '!' => self.lex_run('!'),
            '?' => self.lex_run('?'),
            '¡' => self.lex_inverted_bang(),
            '"' | '\'' => self.lex_string(c),
            '0'..='9' | '.' => self.lex_number(),
            _ if is_ident_start(c) => self.lex_ident(),
            _ => {
                let start = self.pos;
                self.advance(c);
                let span = Span::new(start, self.pos);
                Err(Diagnostic::error(format!("unexpected character `{c}`"))
                    .with_code("E0001")
                    .with_label(Label::primary(span, "I don't know what to do with this")))
            }
        }
    }

    fn lex_equals(&mut self) -> Result<TokenKind, Diagnostic> {
        let mut count = 0u32;
        while self.peek_char() == Some('=') {
            self.advance('=');
            count += 1;
        }
        // `=>` is the fat arrow. If we consumed exactly one `=` and the next
        // char is `>`, return FatArrow.
        if count == 1 && self.peek_char() == Some('>') {
            self.advance('>');
            return Ok(TokenKind::FatArrow);
        }
        match count {
            0 => unreachable!(),
            1..=4 => Ok(TokenKind::Eq(count as u8)),
            _ => Ok(TokenKind::FileSeparator),
        }
    }

    fn lex_run(&mut self, ch: char) -> Result<TokenKind, Diagnostic> {
        let mut count = 0u32;
        while self.peek_char() == Some(ch) {
            self.advance(ch);
            count += 1;
        }
        let count = count.min(255) as u8;
        match ch {
            '!' => {
                // Distinguish `!=`: a single `!` followed immediately by `=`.
                // (Note we already consumed the `!`s; if we consumed exactly
                // one and the next char is `=` we return NotEq.)
                if count == 1 && self.peek_char() == Some('=') {
                    self.advance('=');
                    return Ok(TokenKind::NotEq);
                }
                Ok(TokenKind::Bang(count))
            }
            '?' => Ok(TokenKind::Question(count)),
            _ => unreachable!(),
        }
    }

    fn lex_inverted_bang(&mut self) -> Result<TokenKind, Diagnostic> {
        let mut count = 0u32;
        while self.peek_char() == Some('¡') {
            self.advance('¡');
            count += 1;
        }
        Ok(TokenKind::InvertedBang(count.min(255) as u8))
    }

    fn lex_number(&mut self) -> Result<TokenKind, Diagnostic> {
        let start = self.pos;
        // Integer part.
        while matches!(self.peek_byte(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        // Fractional part.
        if self.peek_byte() == Some(b'.')
            && matches!(self.peek_byte_at(1), Some(b'0'..=b'9'))
        {
            self.pos += 1;
            while matches!(self.peek_byte(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        // Exponent.
        if matches!(self.peek_byte(), Some(b'e' | b'E')) {
            let save = self.pos;
            self.pos += 1;
            if matches!(self.peek_byte(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if matches!(self.peek_byte(), Some(b'0'..=b'9')) {
                while matches!(self.peek_byte(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            } else {
                // Not actually an exponent — back off.
                self.pos = save;
            }
        }
        let text = &self.src[start..self.pos];
        let value: f64 = text.parse().map_err(|_| {
            Diagnostic::error(format!("invalid number literal `{text}`"))
                .with_code("E0002")
                .with_label(Label::primary(
                    Span::new(start, self.pos),
                    "could not parse this as a number",
                ))
        })?;
        Ok(TokenKind::Number(value))
    }

    fn lex_ident(&mut self) -> Result<TokenKind, Diagnostic> {
        let start = self.pos;
        while let Some(c) = self.peek_char() {
            if is_ident_continue(c) {
                self.advance(c);
            } else {
                break;
            }
        }
        let text = &self.src[start..self.pos];
        Ok(keyword_or_ident(text))
    }

    fn lex_string(&mut self, quote: char) -> Result<TokenKind, Diagnostic> {
        let open_start = self.pos;
        let mut quote_count = 0usize;
        while self.peek_char() == Some(quote) {
            self.advance(quote);
            quote_count += 1;
        }
        let mut parts: Vec<StringPart> = Vec::new();
        let mut buf = String::new();

        loop {
            let Some(c) = self.peek_char() else {
                let span = Span::new(open_start, self.pos);
                return Err(Diagnostic::error("unterminated string literal")
                    .with_code("E0010")
                    .with_label(Label::primary(
                        Span::new(open_start, open_start + quote_count),
                        format!(
                            "this string was opened with {} {quote}{} but never closed",
                            quote_count,
                            if quote_count == 1 { "" } else { "s" }
                        ),
                    ))
                    .with_label(Label::secondary(
                        span,
                        "string content begins here",
                    ))
                    .with_note(
                        "string literals can use any number of matching opening and closing \
                         quotes — but they do have to match.",
                    ));
            };
            // Closing run? Match exactly `quote_count` consecutive `quote`s.
            if c == quote {
                let saved = self.pos;
                let mut closing = 0usize;
                while self.peek_char() == Some(quote) {
                    self.advance(quote);
                    closing += 1;
                    if closing == quote_count {
                        break;
                    }
                }
                if closing == quote_count {
                    if !buf.is_empty() {
                        parts.push(StringPart::Lit(std::mem::take(&mut buf)));
                    }
                    return Ok(TokenKind::String(parts));
                }
                // Not enough — append the consumed quotes as content.
                for _ in 0..closing {
                    buf.push(quote);
                }
                let _ = saved;
                continue;
            }
            // Currency-prefixed interpolation: `$`, `£`, `¥` followed by `{`.
            if matches!(c, '$' | '£' | '¥') && self.peek_char_at(1) == Some('{') {
                self.advance(c); // consume the currency
                let expr = self.lex_string_interpolation_braces()?;
                if !buf.is_empty() {
                    parts.push(StringPart::Lit(std::mem::take(&mut buf)));
                }
                parts.push(StringPart::Expr {
                    source: expr.0,
                    span: expr.1,
                });
                continue;
            }
            // Trailing-currency interpolation: `{...}` immediately followed by
            // `€` (the Euro sign goes after the value as per local norms).
            // Also covers the bare `{...}` form when followed by a currency
            // prefix character on either side — keep it simple: a `{` that is
            // followed by an identifier-shaped expression and then `}` is an
            // interpolation if and only if it is immediately followed by a
            // currency symbol or preceded by one.
            if c == '{' {
                // Look ahead: must contain `}` before quote/newline. Decide
                // whether to treat as interpolation or literal `{`.
                if let Some((expr_text, expr_span, post_pos)) = self.try_brace_interp(quote) {
                    // Check if the closing `}` is followed by a currency char.
                    // If so, consume the currency and treat as interp.
                    let after = self.src[post_pos..]
                        .chars()
                        .next();
                    let is_trailing_currency = matches!(after, Some('€'));
                    // Cape Verdean escudo form: identifier`$`identifier inside braces.
                    let is_cape_verde = expr_text.contains('$')
                        && expr_text.chars().all(|ch| ch == '$' || is_ident_continue(ch));
                    if is_trailing_currency || is_cape_verde {
                        // Commit.
                        self.pos = post_pos;
                        if is_trailing_currency {
                            self.advance('€');
                        }
                        if !buf.is_empty() {
                            parts.push(StringPart::Lit(std::mem::take(&mut buf)));
                        }
                        if is_cape_verde {
                            // `a$b` -> member access `a.b`.
                            let dotted = expr_text.replace('$', ".");
                            parts.push(StringPart::Expr {
                                source: dotted,
                                span: expr_span,
                            });
                        } else {
                            parts.push(StringPart::Expr {
                                source: expr_text,
                                span: expr_span,
                            });
                        }
                        continue;
                    }
                }
                // Treat as a literal `{`.
                self.advance('{');
                buf.push('{');
                continue;
            }
            // Backslash escapes. We support a small set of common escapes;
            // the README does not specify them, but a perfect language ought
            // to allow you to put a `\n` in a string.
            if c == '\\' {
                self.advance('\\');
                let next = self.peek_char();
                match next {
                    Some('n') => {
                        self.advance('n');
                        buf.push('\n');
                    }
                    Some('t') => {
                        self.advance('t');
                        buf.push('\t');
                    }
                    Some('r') => {
                        self.advance('r');
                        buf.push('\r');
                    }
                    Some('\\') => {
                        self.advance('\\');
                        buf.push('\\');
                    }
                    Some(q) if q == quote => {
                        self.advance(q);
                        buf.push(q);
                    }
                    Some(other) => {
                        self.advance(other);
                        buf.push('\\');
                        buf.push(other);
                    }
                    None => {
                        return Err(Diagnostic::error("unterminated string literal")
                            .with_code("E0010")
                            .with_label(Label::primary(
                                Span::new(open_start, self.pos),
                                "trailing backslash before end of input",
                            )));
                    }
                }
                continue;
            }
            // Plain character.
            self.advance(c);
            buf.push(c);
        }
    }

    /// Lex the body of an interpolation starting at `{` (already at `{`).
    /// Returns `(source, span)` of the inner expression text.
    fn lex_string_interpolation_braces(&mut self) -> Result<(String, Span), Diagnostic> {
        let brace_start = self.pos;
        debug_assert_eq!(self.peek_char(), Some('{'));
        self.advance('{');
        let inner_start = self.pos;
        let mut depth = 1usize;
        while let Some(c) = self.peek_char() {
            match c {
                '{' => {
                    depth += 1;
                    self.advance('{');
                }
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let inner_end = self.pos;
                        self.advance('}');
                        let text = self.src[inner_start..inner_end].to_string();
                        return Ok((text, Span::new(inner_start, inner_end)));
                    }
                    self.advance('}');
                }
                '\n' => {
                    return Err(Diagnostic::error(
                        "unterminated interpolation in string literal",
                    )
                    .with_code("E0011")
                    .with_label(Label::primary(
                        Span::new(brace_start, self.pos),
                        "this `{` was never closed",
                    )));
                }
                _ => self.advance(c),
            }
        }
        Err(Diagnostic::error(
            "unterminated interpolation in string literal",
        )
        .with_code("E0011")
        .with_label(Label::primary(
            Span::new(brace_start, self.pos),
            "this `{` was never closed",
        )))
    }

    /// Try to parse a `{ ... }` interpolation (without consuming) and return
    /// its inner text plus the position right after the `}`. Used to decide
    /// whether a `{` should be treated as interpolation (when followed by
    /// `€` etc.) or as a literal brace.
    fn try_brace_interp(&self, quote: char) -> Option<(String, Span, usize)> {
        let mut p = self.pos;
        if self.byte_at(p) != Some(b'{') {
            return None;
        }
        p += 1;
        let inner_start = p;
        let mut depth = 1usize;
        while let Some(c) = self.char_at(p) {
            if c == quote || c == '\n' {
                return None;
            }
            match c {
                '{' => {
                    depth += 1;
                    p += c.len_utf8();
                }
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let text = self.src[inner_start..p].to_string();
                        let span = Span::new(inner_start, p);
                        return Some((text, span, p + 1));
                    }
                    p += 1;
                }
                _ => p += c.len_utf8(),
            }
        }
        None
    }

    /// Skip whitespace and comments, returning whether anything was skipped
    /// (used to set the next token's `leading_space`).
    fn skip_trivia(&mut self) -> bool {
        let start = self.pos;
        loop {
            match self.peek_byte() {
                Some(b' ' | b'\t' | b'\r' | b'\n') => self.pos += 1,
                Some(b'/') if self.peek_byte_at(1) == Some(b'/') => {
                    while let Some(b) = self.peek_byte() {
                        if b == b'\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                Some(b'/') if self.peek_byte_at(1) == Some(b'*') => {
                    self.pos += 2;
                    while let Some(b) = self.peek_byte() {
                        if b == b'*' && self.peek_byte_at(1) == Some(b'/') {
                            self.pos += 2;
                            break;
                        }
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
        self.pos > start
    }

    // ---------- low-level helpers ----------

    fn peek_byte(&self) -> Option<u8> {
        self.byte_at(self.pos)
    }

    fn peek_byte_at(&self, off: usize) -> Option<u8> {
        self.byte_at(self.pos + off)
    }

    fn byte_at(&self, p: usize) -> Option<u8> {
        self.bytes.get(p).copied()
    }

    fn peek_char(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek_char_at(&self, n: usize) -> Option<char> {
        self.src[self.pos..].chars().nth(n)
    }

    fn char_at(&self, p: usize) -> Option<char> {
        self.src.get(p..)?.chars().next()
    }

    fn advance(&mut self, c: char) {
        self.pos += c.len_utf8();
    }
}

fn is_inline_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t')
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic() || (!c.is_ascii() && !c.is_whitespace() && !is_punct(c))
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_alphanumeric() || (!c.is_ascii() && !c.is_whitespace() && !is_punct(c))
}

fn is_punct(c: char) -> bool {
    matches!(
        c,
        '(' | ')'
            | '{'
            | '}'
            | '['
            | ']'
            | ','
            | ':'
            | ';'
            | '+'
            | '-'
            | '*'
            | '/'
            | '%'
            | '^'
            | '.'
            | '<'
            | '>'
            | '='
            | '!'
            | '?'
            | '"'
            | '\''
            | '\\'
            | '$'
            | '£'
            | '¥'
            | '€'
            | '¡'
    )
}

fn keyword_or_ident(text: &str) -> TokenKind {
    match text {
        "const" => TokenKind::Const,
        "var" => TokenKind::Var,
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "when" => TokenKind::When,
        "class" => TokenKind::Class,
        "className" => TokenKind::ClassName,
        "return" => TokenKind::Return,
        "new" => TokenKind::New,
        "true" | "True" => TokenKind::True,
        "false" | "False" => TokenKind::False,
        "maybe" | "Maybe" => TokenKind::Maybe,
        "delete" => TokenKind::Delete,
        "previous" => TokenKind::Previous,
        "next" => TokenKind::Next,
        "current" => TokenKind::Current,
        "await" => TokenKind::Await,
        "async" => TokenKind::Async,
        "export" => TokenKind::Export,
        "to" => TokenKind::To,
        "noop" => TokenKind::Noop,
        "use" => TokenKind::Use,
        "null" => TokenKind::Null,
        "undefined" => TokenKind::Undefined,
        // Any prefix of "function" with at least 1 letter, in order.
        "f" | "fn" | "fun" | "func" | "funct" | "functi" | "functio" | "function" => {
            TokenKind::FnKeyword
        }
        // Allow "funct" in DBX (we don't implement DBX but accept the keyword).
        _ => TokenKind::Ident(text.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex_str(s: &str) -> Vec<TokenKind> {
        let file = SourceFile::new("t.gom".into(), s.into());
        lex(&file)
            .unwrap()
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    #[test]
    fn bang_runs_collapse() {
        let kinds = lex_str("a!!!");
        assert!(matches!(kinds[1], TokenKind::Bang(3)));
    }

    #[test]
    fn equals_runs_become_eq_or_separator() {
        assert!(matches!(lex_str("=")[0], TokenKind::Eq(1)));
        assert!(matches!(lex_str("==")[0], TokenKind::Eq(2)));
        assert!(matches!(lex_str("===")[0], TokenKind::Eq(3)));
        assert!(matches!(lex_str("====")[0], TokenKind::Eq(4)));
        assert!(matches!(lex_str("=====")[0], TokenKind::FileSeparator));
        assert!(matches!(lex_str("=========")[0], TokenKind::FileSeparator));
    }

    #[test]
    fn fat_arrow() {
        assert!(matches!(lex_str("=>")[0], TokenKind::FatArrow));
    }

    #[test]
    fn multi_quote_strings() {
        // Triple quotes contain "Lu".
        let toks = lex_str("'''Lu'''");
        let TokenKind::String(parts) = &toks[0] else {
            panic!("expected string, got {:?}", toks[0]);
        };
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            StringPart::Lit(s) => assert_eq!(s, "Lu"),
            other => panic!("expected literal, got {other:?}"),
        }
    }

    #[test]
    fn quad_quotes() {
        let toks = lex_str(r#""""x""""#);
        let TokenKind::String(parts) = &toks[0] else {
            panic!("expected string");
        };
        match &parts[0] {
            StringPart::Lit(s) => assert_eq!(s, "x"),
            _ => panic!(),
        }
    }

    #[test]
    fn currency_interpolation_us_dollar() {
        let toks = lex_str("\"Hello ${name}!\"");
        let TokenKind::String(parts) = &toks[0] else {
            panic!("expected string");
        };
        assert_eq!(parts.len(), 3, "{parts:?}");
    }

    #[test]
    fn function_keyword_prefixes() {
        for kw in ["f", "fn", "fun", "func", "functi", "function"] {
            assert!(
                matches!(lex_str(kw)[0], TokenKind::FnKeyword),
                "{kw} should lex as FnKeyword"
            );
        }
    }

    #[test]
    fn unterminated_string_errors() {
        let file = SourceFile::new("t.gom".into(), r#"print("Hello)!"#.into());
        let err = lex(&file).unwrap_err();
        let rendered = err.render(&file);
        assert!(rendered.contains("string"));
        assert!(rendered.contains("t.gom:"));
    }
}
