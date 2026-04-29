//! Tree-walking interpreter.
//!
//! Built up incrementally:
//!
//! 1. **Part 1** (this file): values, environments, builtins, evaluation of
//!    literals + identifiers + `print`. Statement execution covers expression
//!    statements, `?` debug printing, and `Stmt::Let` declarations.
//! 2. *Part 2* will add operators (including the four levels of equality and
//!    divide-by-zero-is-`undefined`).
//! 3. *Part 3* will add user-defined functions and calls.
//! 4. *Part 4* will add classes, `when` watchers, lifetimes, and file
//!    separators.

mod builtins;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use crate::ast::{
    BinOp, BindingTarget, BoolVal as AstBool, DeclKind, Expr, Lifetime, Program, Stmt, StrPart,
    UnaryOp,
};
use crate::diagnostic::{Diagnostic, Label};
use crate::env::{self, Binding, Scope};
use crate::source::{SourceFile, Span};
use crate::value::{fresh_id, BoolVal, BuiltinFn, Object, Value};

/// Public outcome of running a program.
#[derive(Debug, Default)]
pub struct RunOutcome {
    pub output: String,
}

pub struct Interpreter {
    pub(crate) output: Rc<RefCell<String>>,
    /// 1-based statement counter, used for line-based lifetimes and the
    /// "current line" diagnostics.
    pub(crate) line: usize,
    pub(crate) start_time: Instant,
    pub(crate) globals: Scope,
    /// Names that have been `delete`d (primitives or keywords). The string is
    /// the surface form, e.g. `"3"` or `"class"`.
    pub(crate) deleted: Vec<String>,
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

impl Interpreter {
    pub fn new() -> Self {
        let mut interp = Self {
            output: Rc::new(RefCell::new(String::new())),
            line: 0,
            start_time: Instant::now(),
            globals: env::new_scope(),
            deleted: Vec::new(),
        };
        builtins::install(&mut interp);
        interp
    }

    pub fn run(
        &mut self,
        _file: &SourceFile,
        program: &Program,
    ) -> Result<RunOutcome, Diagnostic> {
        for file in &program.files {
            // Each `=====`-separated file gets a fresh global scope so that
            // declarations don't leak across boundaries.
            if !self.globals.borrow().bindings.is_empty()
                && program.files.iter().filter(|_| true).count() > 0
                && file.stmts.is_empty()
            {
                continue;
            }
            self.run_file(file)?;
            // Reset globals between files; keep builtins alive.
            self.globals = env::new_scope();
            builtins::install(self);
            self.line = 0;
            self.deleted.clear();
        }
        let output = self.output.borrow().clone();
        Ok(RunOutcome { output })
    }

    fn run_file(&mut self, file: &crate::ast::File) -> Result<(), Diagnostic> {
        // Two-pass: first, hoist negative-lifetime declarations so that they
        // are visible *before* their physical position in the file.
        for (i, stmt) in file.stmts.iter().enumerate() {
            if let Stmt::Let {
                lifetime: Some(Lifetime::Lines(n)),
                ..
            } = stmt
            {
                if *n < 0 {
                    self.exec_let(stmt, i + 1)?;
                }
            }
        }
        for (i, stmt) in file.stmts.iter().enumerate() {
            self.line = i + 1;
            // Skip negative-lifetime declarations (already hoisted).
            if let Stmt::Let {
                lifetime: Some(Lifetime::Lines(n)),
                ..
            } = stmt
            {
                if *n < 0 {
                    continue;
                }
            }
            self.exec_stmt(stmt, &Rc::clone(&self.globals))?;
        }
        Ok(())
    }

    pub(crate) fn exec_stmt(&mut self, stmt: &Stmt, scope: &Scope) -> Result<(), Diagnostic> {
        match stmt {
            Stmt::Expr {
                expr,
                bangs: _,
                questions,
                span,
            } => {
                let v = self.eval_expr(expr, scope)?;
                if let Some(_n) = questions {
                    let src = self.format_expr_source(expr);
                    self.output
                        .borrow_mut()
                        .push_str(&format!("[debug] {src} = {}\n", v.display()));
                }
                let _ = span;
                Ok(())
            }
            Stmt::Let { .. } => self.exec_let(stmt, self.line),
            Stmt::Assign { .. } => {
                Err(self.todo("assignment", stmt_span(stmt)))
            }
            other => Err(self.todo(
                &format!("statement kind {:?}", std::mem::discriminant(other)),
                stmt_span(other),
            )),
        }
    }

    pub(crate) fn exec_let(&mut self, stmt: &Stmt, line: usize) -> Result<(), Diagnostic> {
        let Stmt::Let {
            decl,
            const_depth,
            target,
            ty: _,
            lifetime,
            value,
            priority,
            span,
        } = stmt
        else {
            unreachable!()
        };
        let scope = Rc::clone(&self.globals);
        let v = self.eval_expr(value, &scope)?;
        let eternal = *const_depth >= 3;
        match target {
            BindingTarget::Ident { name, span: _ } => {
                env::insert(
                    &scope,
                    Binding {
                        name: name.clone(),
                        value: v,
                        decl: *decl,
                        priority: *priority,
                        created_line: line,
                        created_at: Instant::now(),
                        lifetime: lifetime.clone(),
                        eternal,
                    },
                );
            }
            BindingTarget::Destructure { .. } => {
                return Err(self.todo("destructuring", *span));
            }
        }
        Ok(())
    }

    pub(crate) fn eval_expr(&mut self, expr: &Expr, scope: &Scope) -> Result<Value, Diagnostic> {
        match expr {
            Expr::Number { value, .. } => Ok(Value::Number(*value)),
            Expr::String {
                parts,
                quote_count: _,
                span: _,
            } => self.eval_string_parts(parts, scope),
            Expr::Bool { value, .. } => Ok(Value::Bool(match value {
                AstBool::True => BoolVal::True,
                AstBool::False => BoolVal::False,
                AstBool::Maybe => BoolVal::Maybe,
            })),
            Expr::Null { .. } => Ok(Value::Null),
            Expr::Undefined { .. } => Ok(Value::Undefined),
            Expr::Ident { name, span } => self.eval_ident(name, *span, scope),
            Expr::Array { items, .. } => {
                let mut vs = Vec::with_capacity(items.len());
                for it in items {
                    vs.push(self.eval_expr(it, scope)?);
                }
                Ok(Value::Array(Rc::new(RefCell::new(vs)), fresh_id()))
            }
            Expr::Object { entries, .. } => {
                let mut o = Object::new();
                for (k, v) in entries {
                    let v = self.eval_expr(v, scope)?;
                    o.set(k, v);
                }
                Ok(Value::Object(Rc::new(RefCell::new(o)), fresh_id()))
            }
            Expr::Call { callee, args, span } => self.eval_call(callee, args, *span, scope),
            Expr::Unary { op, operand, span } => self.eval_unary(*op, operand, *span, scope),
            Expr::Binary { op, lhs, rhs, span } => self.eval_binary(*op, lhs, rhs, *span, scope),
            Expr::Member { target, name, span } => self.eval_member(target, name, *span, scope),
            Expr::Index {
                target,
                index,
                span,
            } => self.eval_index(target, index, *span, scope),
            other => Err(self.todo("expression", other.span())),
        }
    }

    fn eval_string_parts(&mut self, parts: &[StrPart], scope: &Scope) -> Result<Value, Diagnostic> {
        let mut out = String::new();
        for p in parts {
            match p {
                StrPart::Lit(s) => out.push_str(s),
                StrPart::Expr(e) => {
                    let v = self.eval_expr(e, scope)?;
                    out.push_str(&v.display());
                }
            }
        }
        let chars: Vec<char> = out.chars().collect();
        Ok(Value::String(Rc::new(RefCell::new(chars)), fresh_id()))
    }

    fn eval_ident(
        &mut self,
        name: &str,
        span: Span,
        scope: &Scope,
    ) -> Result<Value, Diagnostic> {
        // Tombstones from `delete`.
        if self.deleted.iter().any(|d| d == name) {
            return Err(Diagnostic::error(format!("`{name}` has been deleted"))
                .with_code("E0301")
                .with_label(Label::primary(
                    span,
                    "this value was previously deleted",
                ))
                .with_note("`delete` removes a value (or keyword) for the rest of the program."));
        }
        if let Some(v) = env::lookup(scope, name, self.line, self.start_time) {
            return Ok(v);
        }
        // Number-name aliases: `one`, `two`, ... `ten`.
        if let Some(n) = number_name(name) {
            // Check for a redefinition of the corresponding digit.
            let key = format!("{n}");
            if let Some(v) = env::lookup(scope, &key, self.line, self.start_time) {
                return Ok(v);
            }
            return Ok(Value::Number(n));
        }
        // Has the name ever been declared? If so, accessing it now means it
        // expired or hasn't yet been hoisted, which is a real error.
        if env::was_ever_declared(scope, name) {
            return Err(Diagnostic::error(format!("`{name}` has expired"))
                .with_code("E0302")
                .with_label(Label::primary(
                    span,
                    format!("`{name}` is no longer alive at this point"),
                ))
                .with_note(
                    "the binding's lifetime has elapsed. Lifetimes are declared as `<N>` (lines) \
                     or `<Ns>` (seconds); use `<Infinity>` to keep a value forever.",
                ));
        }
        // Bareword string fallback: undeclared identifiers evaluate to a
        // string of their own name.
        let chars: Vec<char> = name.chars().collect();
        Ok(Value::String(Rc::new(RefCell::new(chars)), fresh_id()))
    }

    fn eval_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        span: Span,
        scope: &Scope,
    ) -> Result<Value, Diagnostic> {
        let f = self.eval_expr(callee, scope)?;
        let mut vs = Vec::with_capacity(args.len());
        for a in args {
            vs.push(self.eval_expr(a, scope)?);
        }
        match f {
            Value::BuiltinFn(bf) => (bf.call)(self, vs, span),
            Value::String(_, _) => Err(Diagnostic::error("cannot call a string as a function")
                .with_code("E0400")
                .with_label(Label::primary(
                    callee.span(),
                    "this evaluated to a string, not a function",
                ))),
            other => Err(Diagnostic::error(format!(
                "cannot call a {} as a function",
                other.type_name()
            ))
            .with_code("E0401")
            .with_label(Label::primary(
                callee.span(),
                "this is not a function",
            ))),
        }
    }

    fn eval_unary(
        &mut self,
        op: UnaryOp,
        operand: &Expr,
        span: Span,
        scope: &Scope,
    ) -> Result<Value, Diagnostic> {
        let v = self.eval_expr(operand, scope)?;
        match op {
            UnaryOp::Neg => match v {
                Value::Number(n) => Ok(Value::Number(-n)),
                other => Err(Diagnostic::error(format!(
                    "cannot negate a {}",
                    other.type_name()
                ))
                .with_code("E0500")
                .with_label(Label::primary(span, "expected a number here"))),
            },
            UnaryOp::Not => Ok(Value::Bool(BoolVal::from_bool(!truthy(&v)))),
        }
    }

    fn eval_binary(
        &mut self,
        op: BinOp,
        _lhs: &Expr,
        _rhs: &Expr,
        span: Span,
        _scope: &Scope,
    ) -> Result<Value, Diagnostic> {
        // Filled in by the next interpreter chunk.
        Err(self.todo(&format!("binary operator {op:?}"), span))
    }

    fn eval_member(
        &mut self,
        _target: &Expr,
        _name: &str,
        span: Span,
        _scope: &Scope,
    ) -> Result<Value, Diagnostic> {
        Err(self.todo("member access", span))
    }

    fn eval_index(
        &mut self,
        _target: &Expr,
        _index: &Expr,
        span: Span,
        _scope: &Scope,
    ) -> Result<Value, Diagnostic> {
        Err(self.todo("indexing", span))
    }

    pub(crate) fn format_expr_source(&self, _expr: &Expr) -> String {
        "<expr>".to_string()
    }

    pub(crate) fn todo(&self, what: &str, span: Span) -> Diagnostic {
        Diagnostic::error(format!("{what} not yet implemented in the interpreter"))
            .with_label(Label::primary(span, "encountered here"))
            .with_note(
                "the implementation lands in subsequent commits; this branch hasn't reached \
                 that part of the language yet.",
            )
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Undefined | Value::Null => false,
        Value::Bool(b) => b.is_truthy(),
        Value::Number(n) => *n != 0.0,
        Value::String(s, _) => !s.borrow().is_empty(),
        _ => true,
    }
}

fn number_name(name: &str) -> Option<f64> {
    Some(match name {
        "zero" => 0.0,
        "one" => 1.0,
        "two" => 2.0,
        "three" => 3.0,
        "four" => 4.0,
        "five" => 5.0,
        "six" => 6.0,
        "seven" => 7.0,
        "eight" => 8.0,
        "nine" => 9.0,
        "ten" => 10.0,
        "eleven" => 11.0,
        "twelve" => 12.0,
        _ => return None,
    })
}

fn stmt_span(s: &Stmt) -> Span {
    match s {
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

#[allow(dead_code)]
fn _used_in_later_commits(_: DeclKind, _: &mut BuiltinFn) {}
