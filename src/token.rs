//! Token definitions.
//!
//! A few notable tokens that are not entirely standard:
//!
//! * `Bang(n)` carries the run-length of `!` so that overload priority works.
//! * `Question(n)` similarly tracks `?` count for debug-printing precedence.
//! * `Eq(n)` is `=` (n=1, "least precise"), `==` (n=2), `===` (n=3),
//!   `====` (n=4). Five or more in a row is a `FileSeparator`.
//! * `Semicolon` is the **not** prefix in this language, not a terminator.
//! * `LtAngle`/`GtAngle` are `<` `>` used for lifetime annotations; we do not
//!   distinguish them at lex time from comparison operators — the parser
//!   decides based on context.

use crate::source::Span;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Literals
    Number(f64),
    /// A string literal that may contain interpolated segments.
    /// Each `StringPart` is either a literal chunk or an embedded expression
    /// (which the parser will re-lex/parse as an expression).
    String(Vec<StringPart>),
    Ident(String),

    // Keywords (most are still parsed as identifiers; only the genuinely
    // syntax-defining ones get reserved here).
    Const,
    Var,
    If,
    Else,
    When,
    /// Any prefix of "function": `f`, `fn`, `fun`, `func`, `funct`, `functi`,
    /// `functio`, `function`. We accept all of them here.
    FnKeyword,
    Class,
    ClassName,
    Return,
    New,
    True,
    False,
    Maybe,
    Delete,
    Previous,
    Next,
    Current,
    Await,
    Async,
    Export,
    To,
    Noop,
    Use,
    Null,
    Undefined,

    // Punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    /// `=>` for arrow functions / lambda bodies.
    FatArrow,
    /// `<` (used for both comparison and lifetime brackets).
    LAngle,
    /// `>` (used for both comparison and lifetime brackets).
    RAngle,
    LtEq,
    GtEq,
    NotEq,
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `%`
    Percent,
    /// `;` — the **not** prefix.
    Semi,
    /// `=` repeated `n` times, where `1 <= n <= 4`. n=1 is also assignment.
    Eq(u8),
    /// 5 or more `=` characters; doubles as a "file separator".
    FileSeparator,
    /// `!` repeated `n` times, n >= 1. End-of-statement plus priority hint.
    Bang(u8),
    /// `?` repeated `n` times, n >= 1. End-of-statement debug print.
    Question(u8),
    /// `¡` — inverted bang, signals negative priority overload.
    InvertedBang(u8),

    /// Newline, only emitted in places where it matters (statement level).
    /// We emit one newline token per *logical* line break and collapse runs.
    Newline,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StringPart {
    /// A literal piece of text.
    Lit(String),
    /// An interpolated expression as raw source text plus its span,
    /// to be parsed when the parser handles the enclosing string literal.
    Expr { source: String, span: Span },
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// True if at least one whitespace character precedes this token on the
    /// same line. Used by the parser for whitespace-significant precedence.
    pub leading_space: bool,
    /// True if at least one whitespace character follows this token on the
    /// same line.
    pub trailing_space: bool,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Self {
            kind,
            span,
            leading_space: false,
            trailing_space: false,
        }
    }
}

impl TokenKind {
    /// Human description for diagnostics.
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Number(n) => format!("number `{n}`"),
            TokenKind::String(_) => "string literal".to_string(),
            TokenKind::Ident(s) => format!("identifier `{s}`"),
            TokenKind::Const => "`const`".into(),
            TokenKind::Var => "`var`".into(),
            TokenKind::If => "`if`".into(),
            TokenKind::Else => "`else`".into(),
            TokenKind::When => "`when`".into(),
            TokenKind::FnKeyword => "function keyword".into(),
            TokenKind::Class => "`class`".into(),
            TokenKind::ClassName => "`className`".into(),
            TokenKind::Return => "`return`".into(),
            TokenKind::New => "`new`".into(),
            TokenKind::True => "`true`".into(),
            TokenKind::False => "`false`".into(),
            TokenKind::Maybe => "`maybe`".into(),
            TokenKind::Delete => "`delete`".into(),
            TokenKind::Previous => "`previous`".into(),
            TokenKind::Next => "`next`".into(),
            TokenKind::Current => "`current`".into(),
            TokenKind::Await => "`await`".into(),
            TokenKind::Async => "`async`".into(),
            TokenKind::Export => "`export`".into(),
            TokenKind::To => "`to`".into(),
            TokenKind::Noop => "`noop`".into(),
            TokenKind::Use => "`use`".into(),
            TokenKind::Null => "`null`".into(),
            TokenKind::Undefined => "`undefined`".into(),
            TokenKind::LParen => "`(`".into(),
            TokenKind::RParen => "`)`".into(),
            TokenKind::LBrace => "`{`".into(),
            TokenKind::RBrace => "`}`".into(),
            TokenKind::LBracket => "`[`".into(),
            TokenKind::RBracket => "`]`".into(),
            TokenKind::Comma => "`,`".into(),
            TokenKind::Colon => "`:`".into(),
            TokenKind::FatArrow => "`=>`".into(),
            TokenKind::LAngle => "`<`".into(),
            TokenKind::RAngle => "`>`".into(),
            TokenKind::LtEq => "`<=`".into(),
            TokenKind::GtEq => "`>=`".into(),
            TokenKind::NotEq => "`!=`".into(),
            TokenKind::Plus => "`+`".into(),
            TokenKind::Minus => "`-`".into(),
            TokenKind::Star => "`*`".into(),
            TokenKind::Slash => "`/`".into(),
            TokenKind::Percent => "`%`".into(),
            TokenKind::Semi => "`;` (the not prefix)".into(),
            TokenKind::Eq(n) => format!("`{}`", "=".repeat(*n as usize)),
            TokenKind::FileSeparator => "file separator (`=====`+)".into(),
            TokenKind::Bang(n) => format!("`{}`", "!".repeat(*n as usize)),
            TokenKind::Question(n) => format!("`{}`", "?".repeat(*n as usize)),
            TokenKind::InvertedBang(n) => format!("`{}`", "¡".repeat(*n as usize)),
            TokenKind::Newline => "end of line".into(),
            TokenKind::Eof => "end of file".into(),
        }
    }
}
