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
use crate::value::{fresh_id, BoolVal, BuiltinFn, Class, ClassMethod, Function, Instance, Object, Value};

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
    /// Active `when` watchers — re-checked after every statement.
    pub(crate) watchers: Vec<Watcher>,
    /// Source text of the file currently being run (set in `run_file`), so
    /// that `?` debug-prints can quote the original expression.
    pub(crate) current_source: Option<Rc<String>>,
}

#[derive(Debug)]
pub struct Watcher {
    pub cond: Expr,
    pub block: crate::ast::Block,
    pub scope: Scope,
    pub last_truthy: bool,
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
            watchers: Vec::new(),
            current_source: None,
        };
        builtins::install(&mut interp);
        interp
    }

    pub fn run(
        &mut self,
        source: &SourceFile,
        program: &Program,
    ) -> Result<RunOutcome, Diagnostic> {
        self.current_source = Some(Rc::new(source.text.clone()));
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
            self.run_watchers()?;
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
            Stmt::Assign {
                target,
                value,
                priority: _,
                span,
            } => self.exec_assign(target, value, *span, scope),
            Stmt::ClassDecl {
                name,
                members,
                span,
            } => {
                let mut fields = Vec::new();
                let mut methods = Vec::new();
                for m in members {
                    match m {
                        crate::ast::ClassMember::Field {
                            decl, name, value, ..
                        } => fields.push((name.clone(), *decl, value.clone())),
                        crate::ast::ClassMember::Method {
                            is_async,
                            name,
                            params,
                            body,
                            ..
                        } => methods.push(ClassMethod {
                            is_async: *is_async,
                            name: name.clone(),
                            params: params.clone(),
                            body: body.clone(),
                        }),
                    }
                }
                let class = Value::Class(Rc::new(RefCell::new(Class {
                    name: name.clone(),
                    fields,
                    methods,
                    instance: None,
                    captured: Rc::clone(scope),
                })));
                env::insert(
                    scope,
                    Binding {
                        name: name.clone(),
                        value: class,
                        decl: DeclKind::ConstConst,
                        priority: 0,
                        created_line: self.line,
                        created_at: Instant::now(),
                        lifetime: None,
                        eternal: false,
                    },
                );
                let _ = span;
                Ok(())
            }
            Stmt::Delete { target, span } => self.exec_delete(target, *span, scope),
            Stmt::When { cond, block, span } => {
                self.install_watcher(cond, block.clone(), *span, scope)
            }
            Stmt::Reverse { .. } | Stmt::Export { .. } | Stmt::Import { .. } => {
                // Reverse / export / import are accepted but no-ops in this
                // single-file build of the interpreter.
                Ok(())
            }
            Stmt::FnDecl {
                is_async,
                name,
                params,
                body,
                priority,
                span,
            } => {
                let func = Value::Function(Rc::new(Function {
                    name: name.clone(),
                    is_async: *is_async,
                    params: params.clone(),
                    body: body.clone(),
                    captured: Rc::clone(scope),
                }));
                env::insert(
                    scope,
                    Binding {
                        name: name.clone(),
                        value: func,
                        decl: DeclKind::ConstConst,
                        priority: *priority,
                        created_line: self.line,
                        created_at: Instant::now(),
                        lifetime: None,
                        eternal: false,
                    },
                );
                let _ = span;
                Ok(())
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
                span: _,
            } => {
                let c = self.eval_expr(cond, scope)?;
                if truthy(&c) {
                    self.exec_block(then_block, scope)?;
                } else if let Some(else_block) = else_block {
                    self.exec_block(else_block, scope)?;
                }
                Ok(())
            }
            Stmt::Return { value, span } => {
                let v = match value {
                    Some(e) => self.eval_expr(e, scope)?,
                    None => Value::Undefined,
                };
                Err(return_signal(v, *span))
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
            Expr::Number { value, literal, .. } => {
                // The user can rebind a numeric literal as a name:
                // `const const 5 = 4!` makes `5` evaluate to `4`. We honour
                // any binding for the literal source text before falling back
                // to its numeric value.
                if let Some(v) = env::lookup(scope, literal, self.line, self.start_time) {
                    return Ok(v);
                }
                if self.deleted.iter().any(|d| d == literal) {
                    return Err(Diagnostic::error(format!("{literal} has been deleted"))
                        .with_code("E0301")
                        .with_label(Label::primary(
                            expr.span(),
                            "this value was previously deleted",
                        )));
                }
                Ok(Value::Number(*value))
            }
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
            Expr::New { class, args, span } => self.eval_new(class, args, *span, scope),
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
        // Special-case `bound_ident.method(...)` so we can enforce
        // mutability based on the binding's `DeclKind`.
        if let Expr::Member {
            target,
            name: method_name,
            ..
        } = callee
        {
            if let Expr::Ident { name: var_name, span: name_span } = target.as_ref() {
                if is_mutating_method(method_name) {
                    let mutable_ok = env::lookup_binding(
                        scope,
                        var_name,
                        self.line,
                        self.start_time,
                        |b| b.decl.mutable() || b.priority == i32::MIN,
                    )
                    .unwrap_or(true); // bareword strings: allow
                    if !mutable_ok {
                        let decl_label = env::lookup_binding(
                            scope,
                            var_name,
                            self.line,
                            self.start_time,
                            |b| b.decl.label(),
                        )
                        .unwrap_or("const const");
                        return Err(Diagnostic::error(format!(
                            "cannot mutate `{var_name}` via `.{method_name}()`"
                        ))
                        .with_code("E0702")
                        .with_label(Label::primary(
                            *name_span,
                            format!(
                                "this variable was declared as `{decl_label}`, which forbids \
                                 in-place mutation"
                            ),
                        ))
                        .with_note(
                            "to allow `.pop()` / `.push()` / etc., declare it as `const var` \
                             or `var var`.",
                        ));
                    }
                }
            }
        }
        let f = self.eval_expr(callee, scope)?;
        let mut vs = Vec::with_capacity(args.len());
        for a in args {
            vs.push(self.eval_expr(a, scope)?);
        }
        self.invoke(f, vs, callee, span)
    }

    pub(crate) fn invoke(
        &mut self,
        f: Value,
        vs: Vec<Value>,
        callee: &Expr,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        match f {
            Value::BuiltinFn(bf) => (bf.call)(self, vs, span),
            Value::Function(func) => self.invoke_user_fn(&func, vs, span),
            Value::String(_, _) => Err(Diagnostic::error("cannot call a string as a function")
                .with_code("E0400")
                .with_label(Label::primary(
                    callee.span(),
                    "this evaluated to a string, not a function",
                ))
                .with_note(
                    "undeclared identifiers in Gulf of Mexico evaluate to a bareword string \
                     of their own name. Maybe the function isn't defined yet?",
                )),
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

    fn invoke_user_fn(
        &mut self,
        func: &Rc<Function>,
        args: Vec<Value>,
        _span: Span,
    ) -> Result<Value, Diagnostic> {
        let call_scope = env::child_scope(&func.captured);
        for (i, p) in func.params.iter().enumerate() {
            let v = args.get(i).cloned().unwrap_or(Value::Undefined);
            env::insert(
                &call_scope,
                Binding {
                    name: p.name.clone(),
                    value: v,
                    decl: DeclKind::VarVar,
                    priority: 0,
                    created_line: self.line,
                    created_at: Instant::now(),
                    lifetime: None,
                    eternal: false,
                },
            );
        }
        match &func.body {
            crate::ast::FnBody::Expr(e) => self.eval_expr(e, &call_scope),
            crate::ast::FnBody::Block(b) => match self.exec_block(b, &call_scope) {
                Ok(()) => Ok(Value::Undefined),
                Err(d) if is_return_signal(&d) => Ok(unwrap_return_value(d)),
                Err(d) => Err(d),
            },
        }
    }

    pub(crate) fn exec_block(
        &mut self,
        block: &crate::ast::Block,
        scope: &Scope,
    ) -> Result<(), Diagnostic> {
        let inner = env::child_scope(scope);
        for s in &block.stmts {
            self.exec_stmt(s, &inner)?;
        }
        Ok(())
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
        lhs: &Expr,
        rhs: &Expr,
        span: Span,
        scope: &Scope,
    ) -> Result<Value, Diagnostic> {
        // For the four equality levels we sometimes care about the *syntactic
        // shape* of each side (the `====` "different instances are unequal"
        // rule), so handle them before evaluating.
        if matches!(op, BinOp::EqExtreme) {
            let lv = self.eval_expr(lhs, scope)?;
            let rv = self.eval_expr(rhs, scope)?;
            return Ok(Value::Bool(BoolVal::from_bool(extreme_eq(lhs, rhs, &lv, &rv))));
        }
        let lv = self.eval_expr(lhs, scope)?;
        let rv = self.eval_expr(rhs, scope)?;
        let result = match op {
            BinOp::Add => add(&lv, &rv, span),
            BinOp::Sub => num_op(&lv, &rv, span, |a, b| a - b, "subtract"),
            BinOp::Mul => num_op(&lv, &rv, span, |a, b| a * b, "multiply"),
            BinOp::Div => {
                let (a, b) = require_numbers(&lv, &rv, span, "divide")?;
                if b == 0.0 {
                    Ok(Value::Undefined)
                } else {
                    Ok(Value::Number(a / b))
                }
            }
            BinOp::Mod => {
                let (a, b) = require_numbers(&lv, &rv, span, "%")?;
                if b == 0.0 {
                    Ok(Value::Undefined)
                } else {
                    Ok(Value::Number(a.rem_euclid(b)))
                }
            }
            BinOp::EqLoose1 => Ok(Value::Bool(BoolVal::from_bool(loose_eq_1(&lv, &rv)))),
            BinOp::EqLoose2 => Ok(Value::Bool(BoolVal::from_bool(loose_eq_2(&lv, &rv)))),
            BinOp::EqStrict => Ok(Value::Bool(BoolVal::from_bool(strict_eq(&lv, &rv)))),
            BinOp::EqExtreme => unreachable!(),
            BinOp::NotEq => Ok(Value::Bool(BoolVal::from_bool(!loose_eq_2(&lv, &rv)))),
            BinOp::Lt => num_cmp(&lv, &rv, span, |a, b| a < b),
            BinOp::Gt => num_cmp(&lv, &rv, span, |a, b| a > b),
            BinOp::LtEq => num_cmp(&lv, &rv, span, |a, b| a <= b),
            BinOp::GtEq => num_cmp(&lv, &rv, span, |a, b| a >= b),
        };
        // If the producing value is a number whose surface form has been
        // `delete`d, that's an error: `delete 3!` then `2 + 1` yields 3,
        // which has been deleted.
        let v = result?;
        if let Value::Number(n) = &v {
            let display = crate::value::Value::Number(*n).display();
            if self.deleted.iter().any(|d| d == &display) {
                return Err(Diagnostic::error(format!("{display} has been deleted"))
                    .with_code("E0301")
                    .with_label(Label::primary(span, "this value was previously deleted"))
                    .with_note(
                        "primitives can be removed from a program with `delete`. Once gone, \
                         arithmetic that produces them errors.",
                    ));
            }
        }
        Ok(v)
    }

    fn eval_new(
        &mut self,
        class_expr: &Expr,
        _args: &[Expr],
        span: Span,
        scope: &Scope,
    ) -> Result<Value, Diagnostic> {
        let v = self.eval_expr(class_expr, scope)?;
        let class_rc = match v {
            Value::Class(c) => c,
            other => {
                return Err(Diagnostic::error(format!(
                    "cannot instantiate a {} with `new`",
                    other.type_name()
                ))
                .with_code("E0800")
                .with_label(Label::primary(class_expr.span(), "expected a class here")));
            }
        };
        if class_rc.borrow().instance.is_some() {
            let class_name = class_rc.borrow().name.clone();
            return Err(Diagnostic::error(format!(
                "Can't have more than one '{class_name}' instance"
            ))
            .with_code("E0801")
            .with_label(Label::primary(
                span,
                format!("a `{class_name}` instance already exists"),
            ))
            .with_note(
                "classes in Gulf of Mexico allow only one instance, ever. Use a factory \
                 function (e.g. `class PlayerMaker { function makePlayer() => { class Player \
                 { ... } new Player() }! }`) to work around this.",
            ));
        }
        // Fresh instance with each field's default value.
        let mut fields = Object::new();
        let class_borrow = class_rc.borrow();
        let class_scope = Rc::clone(&class_borrow.captured);
        let field_decls: Vec<(String, DeclKind, Expr)> = class_borrow.fields.clone();
        let methods = class_borrow.methods.clone();
        let class_name = class_borrow.name.clone();
        drop(class_borrow);
        for (name, _decl, default) in &field_decls {
            let v = self.eval_expr(default, &class_scope)?;
            fields.set(name, v);
        }
        let instance = Rc::new(RefCell::new(Instance {
            class_name: class_name.clone(),
            fields,
        }));
        let id = fresh_id();
        let inst_value = Value::Instance(Rc::clone(&instance), id);
        // Install methods as bound functions on the instance object.
        for m in methods {
            let func = Value::Function(Rc::new(Function {
                name: format!("{}.{}", class_name, m.name),
                is_async: m.is_async,
                params: m.params.clone(),
                body: m.body.clone(),
                captured: Rc::clone(&class_scope),
            }));
            instance.borrow_mut().fields.set(&m.name, func);
        }
        class_rc.borrow_mut().instance = Some(Rc::clone(&instance));
        Ok(inst_value)
    }

    fn install_watcher(
        &mut self,
        cond: &Expr,
        block: crate::ast::Block,
        _span: Span,
        scope: &Scope,
    ) -> Result<(), Diagnostic> {
        // We model `when (cond) { block }` as a re-check that runs after every
        // statement in the rest of the file. To keep the implementation
        // simple, we hook it into the file-level loop via the `watchers` list
        // on `Interpreter` and re-check at each step.
        self.watchers.push(Watcher {
            cond: cond.clone(),
            block,
            scope: Rc::clone(scope),
            last_truthy: false,
        });
        // First evaluation: if it's already true, run immediately so that the
        // user can rely on initial-state semantics if they want.
        let init = self.eval_expr(cond, scope)?;
        let last = self.watchers.last_mut().unwrap();
        last.last_truthy = truthy(&init);
        Ok(())
    }

    pub(crate) fn run_watchers(&mut self) -> Result<(), Diagnostic> {
        // Drain into a local vector so we can mutate `self.watchers` while
        // running their bodies.
        let mut watchers = std::mem::take(&mut self.watchers);
        for w in &mut watchers {
            let v = self.eval_expr(&w.cond, &Rc::clone(&w.scope))?;
            let now = truthy(&v);
            if now && !w.last_truthy {
                let scope = Rc::clone(&w.scope);
                self.exec_block(&w.block, &scope)?;
            }
            w.last_truthy = now;
        }
        self.watchers = watchers;
        Ok(())
    }

    fn exec_delete(
        &mut self,
        target: &Expr,
        _span: Span,
        _scope: &Scope,
    ) -> Result<(), Diagnostic> {
        // Tombstone the surface form. `delete 3!` -> add "3" to deleted;
        // `delete x!` -> add "x".
        let key = match target {
            Expr::Ident { name, .. } => name.clone(),
            Expr::Number { literal, .. } => literal.clone(),
            Expr::String { parts, .. } => parts
                .iter()
                .filter_map(|p| match p {
                    StrPart::Lit(s) => Some(s.clone()),
                    _ => None,
                })
                .collect::<String>(),
            other => format!("{:?}", std::mem::discriminant(other)),
        };
        self.deleted.push(key);
        Ok(())
    }

    fn eval_member(
        &mut self,
        target: &Expr,
        name: &str,
        span: Span,
        scope: &Scope,
    ) -> Result<Value, Diagnostic> {
        let v = self.eval_expr(target, scope)?;
        match &v {
            Value::Object(o, _) => Ok(o.borrow().get(name).unwrap_or(Value::Undefined)),
            Value::Instance(inst, _) => {
                Ok(inst.borrow().fields.get(name).unwrap_or(Value::Undefined))
            }
            // `name.pop()` / `name.push(c)` for strings are bound dynamically.
            Value::String(_, _) => Ok(self.string_method(v.clone(), name, span)),
            // Likewise for arrays.
            Value::Array(_, _) => Ok(self.array_method(v.clone(), name, span)),
            _ => Err(Diagnostic::error(format!(
                "cannot access member `{name}` on a {}",
                v.type_name()
            ))
            .with_code("E0600")
            .with_label(Label::primary(
                span,
                "this value has no fields or methods",
            ))),
        }
    }

    fn string_method(&self, target: Value, name: &str, _span: Span) -> Value {
        let target = target;
        let name = name.to_string();
        let bf = BuiltinFn {
            name: "<string method>",
            call: Box::new(move |_interp, args, span| {
                string_method_call(&target, &name, args, span)
            }),
        };
        Value::BuiltinFn(Rc::new(bf))
    }

    fn array_method(&self, target: Value, name: &str, _span: Span) -> Value {
        let target = target;
        let name = name.to_string();
        let bf = BuiltinFn {
            name: "<array method>",
            call: Box::new(move |_interp, args, span| {
                array_method_call(&target, &name, args, span)
            }),
        };
        Value::BuiltinFn(Rc::new(bf))
    }

    fn eval_index(
        &mut self,
        target: &Expr,
        index: &Expr,
        span: Span,
        scope: &Scope,
    ) -> Result<Value, Diagnostic> {
        let v = self.eval_expr(target, scope)?;
        let i = self.eval_expr(index, scope)?;
        match (&v, &i) {
            (Value::Array(arr, _), Value::Number(n)) => {
                let arr = arr.borrow();
                let real = (*n + 1.0).round() as i64;
                if real < 0 || real as usize >= arr.len() {
                    Ok(Value::Undefined)
                } else {
                    Ok(arr[real as usize].clone())
                }
            }
            (Value::String(s, _), Value::Number(n)) => {
                let s = s.borrow();
                let real = (*n + 1.0).round() as i64;
                if real < 0 || real as usize >= s.len() {
                    Ok(Value::Undefined)
                } else {
                    let c = s[real as usize];
                    let mut buf = Vec::new();
                    buf.push(c);
                    Ok(Value::String(
                        Rc::new(RefCell::new(buf)),
                        crate::value::fresh_id(),
                    ))
                }
            }
            (Value::Object(o, _), key) => {
                let key_str = key.display();
                Ok(o.borrow().get(&key_str).unwrap_or(Value::Undefined))
            }
            _ => Err(Diagnostic::error(format!(
                "cannot index a {} with a {}",
                v.type_name(),
                i.type_name()
            ))
            .with_code("E0601")
            .with_label(Label::primary(span, "indexing not supported here"))),
        }
    }

    pub(crate) fn format_expr_source(&self, expr: &Expr) -> String {
        if let Some(src) = &self.current_source {
            let s = expr.span();
            let end = s.end.min(src.len());
            let start = s.start.min(end);
            return src[start..end].to_string();
        }
        "<expr>".to_string()
    }

    fn exec_assign(
        &mut self,
        target: &Expr,
        value: &Expr,
        span: Span,
        scope: &Scope,
    ) -> Result<(), Diagnostic> {
        let v = self.eval_expr(value, scope)?;
        match target {
            Expr::Ident { name, span: name_span } => {
                // Reassignment: requires the binding to be `var const` or
                // `var var`.
                let decl = match env::reassign(scope, name, v.clone(), self.line, self.start_time) {
                    Ok(d) => d,
                    Err(_) => {
                        if env::was_ever_declared(scope, name) {
                            // Bound once but expired or hidden.
                            return Err(Diagnostic::error(format!("`{name}` has expired"))
                                .with_code("E0302")
                                .with_label(Label::primary(*name_span, "no live binding")));
                        }
                        // Implicit declaration as `var var`.
                        env::insert(
                            scope,
                            Binding {
                                name: name.clone(),
                                value: v,
                                decl: DeclKind::VarVar,
                                priority: 0,
                                created_line: self.line,
                                created_at: Instant::now(),
                                lifetime: None,
                                eternal: false,
                            },
                        );
                        return Ok(());
                    }
                };
                if !decl.reassignable() {
                    return Err(Diagnostic::error(format!(
                        "cannot reassign `{name}`"
                    ))
                    .with_code("E0700")
                    .with_label(Label::primary(
                        *name_span,
                        format!(
                            "this variable was declared as `{}`, which forbids reassignment",
                            decl.label()
                        ),
                    ))
                    .with_note(
                        "to allow reassignment, declare it as `var const` or `var var`.",
                    ));
                }
                Ok(())
            }
            Expr::Index { target, index, .. } => {
                let arr = self.eval_expr(target, scope)?;
                let idx = self.eval_expr(index, scope)?;
                index_assign(&arr, &idx, v, span)
            }
            Expr::Member { target, name, .. } => {
                let obj = self.eval_expr(target, scope)?;
                member_assign(&obj, name, v, span)
            }
            _ => Err(Diagnostic::error(
                "the left-hand side of `=` must be a name, member access, or index",
            )
            .with_code("E0701")
            .with_label(Label::primary(span, "this is not assignable"))),
        }
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

fn is_mutating_method(name: &str) -> bool {
    matches!(
        name,
        "pop" | "push" | "unshift" | "shift" | "splice" | "sort" | "reverse"
    )
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

// ===========================================================================
// "Return" is signalled via a sentinel diagnostic so we can unwind through
// arbitrary block nesting cheaply. The sentinel uses an unreachable error
// code that no real diagnostic emits.
// ===========================================================================

const RETURN_SIGNAL_CODE: &str = "__gulf_return__";

thread_local! {
    static RETURN_VALUE: RefCell<Option<Value>> = const { RefCell::new(None) };
}

fn return_signal(v: Value, span: Span) -> Diagnostic {
    RETURN_VALUE.with(|cell| *cell.borrow_mut() = Some(v));
    Diagnostic::error("internal: return")
        .with_code(RETURN_SIGNAL_CODE)
        .with_label(Label::primary(span, ""))
}

fn is_return_signal(d: &Diagnostic) -> bool {
    d.code == Some(RETURN_SIGNAL_CODE)
}

fn unwrap_return_value(_d: Diagnostic) -> Value {
    RETURN_VALUE.with(|cell| cell.borrow_mut().take().unwrap_or(Value::Undefined))
}

// ===========================================================================
// Equality, arithmetic, and value-coercion helpers.
// ===========================================================================

/// `=` — least-precise comparison. Numbers are compared after rounding to the
/// nearest integer (so `3 = 3.14` is `true`); booleans compare loosely;
/// otherwise this falls back to JS-loose-`==` semantics.
fn loose_eq_1(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => (x.round() - y.round()).abs() < f64::EPSILON,
        (Value::Number(x), Value::Bool(b)) | (Value::Bool(b), Value::Number(x)) => {
            (x.round() != 0.0) == b.is_truthy()
        }
        _ => loose_eq_2(a, b),
    }
}

/// `==` — JS-style loose equality. Numbers and strings coerce to numbers; `maybe`
/// matches anything; otherwise structural equality.
fn loose_eq_2(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Bool(BoolVal::Maybe), _) | (_, Value::Bool(BoolVal::Maybe)) => true,
        (Value::Number(x), Value::Number(y)) => {
            (x.is_nan() && y.is_nan()) || (x - y).abs() < f64::EPSILON
        }
        (Value::String(x, _), Value::String(y, _)) => *x.borrow() == *y.borrow(),
        (Value::Number(n), Value::String(s, _)) | (Value::String(s, _), Value::Number(n)) => {
            let s: String = s.borrow().iter().collect();
            s.parse::<f64>().is_ok_and(|sn| (sn - *n).abs() < f64::EPSILON)
        }
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Undefined, Value::Undefined) => true,
        (Value::Null, Value::Null) => true,
        (Value::Undefined, Value::Null) | (Value::Null, Value::Undefined) => true,
        _ => strict_eq(a, b),
    }
}

/// `===` — strict equality: same type AND same value.
fn strict_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => (x - y).abs() < f64::EPSILON,
        (Value::String(x, _), Value::String(y, _)) => *x.borrow() == *y.borrow(),
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Undefined, Value::Undefined) => true,
        (Value::Null, Value::Null) => true,
        _ => false,
    }
}

/// `====` — extreme equality. Two values share an identity iff they were
/// minted from the same allocation. For literals, equal values share an
/// identity; for variables, only the same variable does. So:
///
/// * `pi ==== pi` is `true` (same expression);
/// * `3.14 ==== 3.14` is `true` (literal == same literal value);
/// * `3.14 ==== pi` is `false` (literal vs. variable, different "instances").
fn extreme_eq(lhs_expr: &Expr, rhs_expr: &Expr, lv: &Value, rv: &Value) -> bool {
    let lhs_kind = expr_shape(lhs_expr);
    let rhs_kind = expr_shape(rhs_expr);
    match (lhs_kind, rhs_kind) {
        (ExprShape::Ident(a), ExprShape::Ident(b)) => a == b && strict_eq(lv, rv),
        (ExprShape::Literal, ExprShape::Literal) => strict_eq(lv, rv),
        // Mixed shapes: only true if the underlying allocations match.
        _ => match (lv.instance_id(), rv.instance_id()) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        },
    }
}

#[derive(PartialEq)]
enum ExprShape<'a> {
    Ident(&'a str),
    Literal,
    Other,
}

fn expr_shape(e: &Expr) -> ExprShape<'_> {
    match e {
        Expr::Ident { name, .. } => ExprShape::Ident(name),
        Expr::Number { .. }
        | Expr::String { .. }
        | Expr::Bool { .. }
        | Expr::Null { .. }
        | Expr::Undefined { .. } => ExprShape::Literal,
        _ => ExprShape::Other,
    }
}

fn add(a: &Value, b: &Value, span: Span) -> Result<Value, Diagnostic> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => Ok(Value::Number(x + y)),
        (Value::String(_, _), _) | (_, Value::String(_, _)) => {
            let combined = format!("{}{}", a.display(), b.display());
            let chars: Vec<char> = combined.chars().collect();
            Ok(Value::String(
                Rc::new(RefCell::new(chars)),
                crate::value::fresh_id(),
            ))
        }
        _ => Err(Diagnostic::error(format!(
            "cannot add a {} and a {}",
            a.type_name(),
            b.type_name()
        ))
        .with_code("E0500")
        .with_label(Label::primary(span, "this addition isn't well-defined"))),
    }
}

fn num_op(
    a: &Value,
    b: &Value,
    span: Span,
    f: fn(f64, f64) -> f64,
    op_name: &str,
) -> Result<Value, Diagnostic> {
    let (x, y) = require_numbers(a, b, span, op_name)?;
    Ok(Value::Number(f(x, y)))
}

fn num_cmp(
    a: &Value,
    b: &Value,
    span: Span,
    f: fn(f64, f64) -> bool,
) -> Result<Value, Diagnostic> {
    let (x, y) = require_numbers(a, b, span, "compare")?;
    Ok(Value::Bool(BoolVal::from_bool(f(x, y))))
}

fn require_numbers(
    a: &Value,
    b: &Value,
    span: Span,
    op_name: &str,
) -> Result<(f64, f64), Diagnostic> {
    let x = coerce_number(a).ok_or_else(|| {
        Diagnostic::error(format!("cannot {op_name} a {}", a.type_name()))
            .with_code("E0500")
            .with_label(Label::primary(span, "expected a number here"))
    })?;
    let y = coerce_number(b).ok_or_else(|| {
        Diagnostic::error(format!("cannot {op_name} a {}", b.type_name()))
            .with_code("E0500")
            .with_label(Label::primary(span, "expected a number here"))
    })?;
    Ok((x, y))
}

fn coerce_number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => Some(*n),
        Value::Bool(b) => Some(if b.is_truthy() { 1.0 } else { 0.0 }),
        Value::String(s, _) => {
            let s: String = s.borrow().iter().collect();
            s.parse().ok()
        }
        _ => None,
    }
}

// ===========================================================================
// Assignment helpers.
// ===========================================================================

fn index_assign(arr: &Value, idx: &Value, value: Value, span: Span) -> Result<(), Diagnostic> {
    match (arr, idx) {
        (Value::Array(a, _), Value::Number(n)) => {
            let mut a = a.borrow_mut();
            let real = *n + 1.0;
            // Float index = insertion at `floor(real) + 1`. So `[3,2,5]` with
            // `arr[0.5] = 4` -> real = 1.5 -> insert before index 2 ->
            // `[3, 2, 4, 5]`.
            if real.fract() != 0.0 {
                let pos = real.ceil() as i64;
                let pos = pos.clamp(0, a.len() as i64) as usize;
                a.insert(pos, value);
            } else {
                let pos = real as i64;
                if pos < 0 {
                    return Err(Diagnostic::error("array index out of range")
                        .with_code("E0602")
                        .with_label(Label::primary(span, "index < -1")));
                }
                let pos = pos as usize;
                if pos >= a.len() {
                    a.resize(pos + 1, Value::Undefined);
                }
                a[pos] = value;
            }
            Ok(())
        }
        (Value::Object(o, _), key) => {
            o.borrow_mut().set(&key.display(), value);
            Ok(())
        }
        _ => Err(Diagnostic::error(format!(
            "cannot index-assign into a {}",
            arr.type_name()
        ))
        .with_code("E0603")
        .with_label(Label::primary(span, "this is not an array or object"))),
    }
}

fn member_assign(target: &Value, name: &str, value: Value, span: Span) -> Result<(), Diagnostic> {
    match target {
        Value::Object(o, _) => {
            o.borrow_mut().set(name, value);
            Ok(())
        }
        Value::Instance(inst, _) => {
            inst.borrow_mut().fields.set(name, value);
            Ok(())
        }
        _ => Err(Diagnostic::error(format!(
            "cannot assign to `.{name}` on a {}",
            target.type_name()
        ))
        .with_code("E0604")
        .with_label(Label::primary(span, "this value has no fields"))),
    }
}

// ===========================================================================
// Built-in string and array methods.
// ===========================================================================

fn string_method_call(
    target: &Value,
    name: &str,
    args: Vec<Value>,
    span: Span,
) -> Result<Value, Diagnostic> {
    let Value::String(s, _) = target else {
        unreachable!()
    };
    match name {
        "pop" => {
            let popped = s.borrow_mut().pop();
            Ok(popped
                .map(|c| {
                    Value::String(
                        Rc::new(RefCell::new(vec![c])),
                        crate::value::fresh_id(),
                    )
                })
                .unwrap_or(Value::Undefined))
        }
        "push" => {
            for a in &args {
                if let Value::String(arg_s, _) = a {
                    for c in arg_s.borrow().iter() {
                        s.borrow_mut().push(*c);
                    }
                } else {
                    let txt = a.display();
                    for c in txt.chars() {
                        s.borrow_mut().push(c);
                    }
                }
            }
            Ok(Value::Undefined)
        }
        "length" => Ok(Value::Number(s.borrow().len() as f64)),
        other => Err(Diagnostic::error(format!(
            "no string method `{other}`"
        ))
        .with_code("E0610")
        .with_label(Label::primary(span, "unknown string method"))),
    }
}

fn array_method_call(
    target: &Value,
    name: &str,
    args: Vec<Value>,
    span: Span,
) -> Result<Value, Diagnostic> {
    let Value::Array(a, _) = target else {
        unreachable!()
    };
    match name {
        "pop" => Ok(a.borrow_mut().pop().unwrap_or(Value::Undefined)),
        "push" => {
            for v in args {
                a.borrow_mut().push(v);
            }
            Ok(Value::Undefined)
        }
        "length" => Ok(Value::Number(a.borrow().len() as f64)),
        other => Err(Diagnostic::error(format!(
            "no array method `{other}`"
        ))
        .with_code("E0611")
        .with_label(Label::primary(span, "unknown array method"))),
    }
}
