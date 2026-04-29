//! Abstract syntax tree.
//!
//! The AST tries to faithfully encode every quirk of the language so that
//! later passes can act on intent rather than syntax.

use crate::source::Span;

#[derive(Debug, Clone)]
pub struct Program {
    /// One entry per "file" in the source, separated by `=====` lines.
    pub files: Vec<File>,
}

#[derive(Debug, Clone)]
pub struct File {
    pub name: Option<String>,
    pub stmts: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        decl: DeclKind,
        /// Number of `const`s in the declaration (2 = normal, 3 = "globally
        /// forever immutable" const-const-const).
        const_depth: u8,
        target: BindingTarget,
        /// Optional type annotation, parsed and ignored.
        ty: Option<TypeRef>,
        lifetime: Option<Lifetime>,
        value: Expr,
        /// Overload priority. `!`s are positive, `¡`s are negative.
        priority: i32,
        span: Span,
    },
    Expr {
        expr: Expr,
        /// `Some(n)` if the statement ended with `n` `!`s. None means `?`.
        bangs: Option<u8>,
        /// `Some(n)` if the statement ended with `n` `?`s.
        questions: Option<u8>,
        span: Span,
    },
    Assign {
        target: Expr,
        value: Expr,
        priority: i32,
        span: Span,
    },
    If {
        cond: Expr,
        then_block: Block,
        else_block: Option<Block>,
        span: Span,
    },
    When {
        cond: Expr,
        block: Block,
        span: Span,
    },
    FnDecl {
        is_async: bool,
        name: String,
        params: Vec<Param>,
        body: FnBody,
        priority: i32,
        span: Span,
    },
    ClassDecl {
        name: String,
        members: Vec<ClassMember>,
        span: Span,
    },
    Return {
        value: Option<Expr>,
        span: Span,
    },
    Delete {
        target: Expr,
        span: Span,
    },
    Export {
        name: String,
        target_file: String,
        span: Span,
    },
    Import {
        name: String,
        span: Span,
    },
    /// Aesthetic `reverse!` statement. We accept and warn on it.
    Reverse {
        span: Span,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclKind {
    /// `const const`
    ConstConst,
    /// `const var`
    ConstVar,
    /// `var const`
    VarConst,
    /// `var var`
    VarVar,
}

impl DeclKind {
    /// Is this binding allowed to be **reassigned** (`x = y`)?
    pub fn reassignable(self) -> bool {
        matches!(self, DeclKind::VarConst | DeclKind::VarVar)
    }

    /// Is the **inner value** allowed to be mutated (e.g. `x.pop()`)?
    pub fn mutable(self) -> bool {
        matches!(self, DeclKind::ConstVar | DeclKind::VarVar)
    }

    pub fn label(self) -> &'static str {
        match self {
            DeclKind::ConstConst => "const const",
            DeclKind::ConstVar => "const var",
            DeclKind::VarConst => "var const",
            DeclKind::VarVar => "var var",
        }
    }
}

#[derive(Debug, Clone)]
pub enum BindingTarget {
    Ident { name: String, span: Span },
    /// Destructured signal-like: `[a, b]` or `[[a, b], b]` etc.
    Destructure { pattern: DestructurePattern, span: Span },
}

#[derive(Debug, Clone)]
pub enum DestructurePattern {
    Ident(String, Span),
    /// `[a, b]` with arbitrary nesting.
    List(Vec<DestructurePattern>, Span),
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: Option<TypeRef>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum FnBody {
    /// `function add(a, b) => a + b!`
    Expr(Expr),
    /// `function f() => { ... }`
    Block(Block),
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ClassMember {
    Field {
        decl: DeclKind,
        name: String,
        value: Expr,
        span: Span,
    },
    Method {
        is_async: bool,
        name: String,
        params: Vec<Param>,
        body: FnBody,
        span: Span,
    },
}

/// Type annotation. We parse a small grammar and otherwise treat as opaque
/// text for diagnostics.
#[derive(Debug, Clone)]
pub struct TypeRef {
    pub source: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Lifetime {
    /// `<N>` lines (signed; negative means hoisting)
    Lines(i64),
    /// `<Ns>` seconds
    Seconds(f64),
    /// `<Infinity>` — survives between runs (we treat it as live forever)
    Infinity,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Number {
        value: f64,
        /// The exact source text, used by `====` to compare literal identity.
        literal: String,
        span: Span,
    },
    String {
        parts: Vec<StrPart>,
        /// Number of opening quote chars; 0 means an unquoted "bareword" string.
        quote_count: usize,
        span: Span,
    },
    Bool {
        value: BoolVal,
        span: Span,
    },
    Undefined {
        span: Span,
    },
    Null {
        span: Span,
    },
    Ident {
        name: String,
        span: Span,
    },
    Array {
        items: Vec<Expr>,
        span: Span,
    },
    Object {
        entries: Vec<(String, Expr)>,
        span: Span,
    },
    Index {
        target: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    Member {
        target: Box<Expr>,
        name: String,
        span: Span,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
        span: Span,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },
    /// `previous x`, `next x`, `current x`
    Time {
        when: TimeKind,
        target: Box<Expr>,
        span: Span,
    },
    /// `await x`
    Await {
        target: Box<Expr>,
        span: Span,
    },
    /// `new ClassName(args)`
    New {
        class: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    /// Anonymous lambda `(a, b) => body`
    Lambda {
        is_async: bool,
        params: Vec<Param>,
        body: Box<FnBody>,
        span: Span,
    },
    /// `use(initial)` — a signal getter/setter combined.
    UseSignal {
        initial: Box<Expr>,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Number { span, .. }
            | Expr::String { span, .. }
            | Expr::Bool { span, .. }
            | Expr::Undefined { span }
            | Expr::Null { span }
            | Expr::Ident { span, .. }
            | Expr::Array { span, .. }
            | Expr::Object { span, .. }
            | Expr::Index { span, .. }
            | Expr::Member { span, .. }
            | Expr::Call { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Time { span, .. }
            | Expr::Await { span, .. }
            | Expr::New { span, .. }
            | Expr::Lambda { span, .. }
            | Expr::UseSignal { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone)]
pub enum StrPart {
    Lit(String),
    Expr(Expr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoolVal {
    True,
    False,
    Maybe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    /// `;` — the not prefix
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    /// `=` (1) — least-precise comparison.
    EqLoose1,
    /// `==` (2) — JS-style loose equality.
    EqLoose2,
    /// `===` (3) — strict equality.
    EqStrict,
    /// `====` (4) — extreme/identity equality.
    EqExtreme,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeKind {
    Previous,
    Next,
    Current,
}
