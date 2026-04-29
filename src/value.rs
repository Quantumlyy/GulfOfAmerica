//! Runtime values.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use crate::ast::{FnBody, Param};

/// Stable instance identity — used by `====` for "extreme" equality.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct InstanceId(pub u64);

#[derive(Clone)]
pub enum Value {
    Undefined,
    Null,
    Number(f64),
    /// Booleans are tri-state: true / false / maybe.
    Bool(BoolVal),
    String(Rc<RefCell<Vec<char>>>, InstanceId),
    Array(Rc<RefCell<Vec<Value>>>, InstanceId),
    Object(Rc<RefCell<Object>>, InstanceId),
    Function(Rc<Function>),
    BuiltinFn(Rc<BuiltinFn>),
    Class(Rc<RefCell<Class>>),
    /// A live instance of a class.
    Instance(Rc<RefCell<Instance>>, InstanceId),
    /// Signal: a `use(x)` value. Calling with no args reads, with one arg writes.
    Signal(Rc<RefCell<Signal>>, InstanceId),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BoolVal {
    True,
    False,
    Maybe,
}

impl BoolVal {
    pub fn from_bool(b: bool) -> Self {
        if b {
            BoolVal::True
        } else {
            BoolVal::False
        }
    }

    /// Logical not: true<->false; maybe is its own opposite.
    pub fn negate(self) -> Self {
        match self {
            BoolVal::True => BoolVal::False,
            BoolVal::False => BoolVal::True,
            BoolVal::Maybe => BoolVal::Maybe,
        }
    }

    pub fn is_truthy(self) -> bool {
        // `maybe` is truthy half the time. We pick truthy here for determinism
        // unless overridden via a randomness source.
        matches!(self, BoolVal::True | BoolVal::Maybe)
    }
}

#[derive(Debug)]
pub struct Object {
    pub entries: Vec<(String, Value)>,
}

impl Object {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn get(&self, name: &str) -> Option<Value> {
        self.entries
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    }

    pub fn set(&mut self, name: &str, value: Value) {
        if let Some(slot) = self.entries.iter_mut().find(|(k, _)| k == name) {
            slot.1 = value;
        } else {
            self.entries.push((name.to_string(), value));
        }
    }
}

impl Default for Object {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Function {
    pub name: String,
    pub is_async: bool,
    pub params: Vec<Param>,
    pub body: FnBody,
    pub captured: crate::env::Scope,
}

pub struct BuiltinFn {
    pub name: &'static str,
    pub call: Box<
        dyn Fn(
            &mut crate::interpreter::Interpreter,
            Vec<Value>,
            crate::source::Span,
        ) -> Result<Value, crate::diagnostic::Diagnostic>,
    >,
}

#[derive(Debug)]
pub struct Class {
    pub name: String,
    pub fields: Vec<(String, crate::ast::DeclKind, crate::ast::Expr)>,
    pub methods: Vec<ClassMethod>,
    /// The single allowed instance. Stored on the class so reuse of `new`
    /// can detect it. `None` means no instance exists yet.
    pub instance: Option<Rc<RefCell<Instance>>>,
    pub captured: crate::env::Scope,
}

#[derive(Debug, Clone)]
pub struct ClassMethod {
    pub is_async: bool,
    pub name: String,
    pub params: Vec<Param>,
    pub body: FnBody,
}

#[derive(Debug)]
pub struct Instance {
    pub class_name: String,
    pub fields: Object,
}

#[derive(Debug)]
pub struct Signal {
    pub current: Value,
}

impl fmt::Debug for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Function")
            .field("name", &self.name)
            .field("is_async", &self.is_async)
            .field("params", &self.params.len())
            .finish()
    }
}

impl fmt::Debug for BuiltinFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BuiltinFn({})", self.name)
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Undefined => write!(f, "undefined"),
            Value::Null => write!(f, "null"),
            Value::Number(n) => write!(f, "{n}"),
            Value::Bool(b) => write!(f, "{b:?}"),
            Value::String(s, _) => {
                let s: String = s.borrow().iter().collect();
                write!(f, "{s:?}")
            }
            Value::Array(a, _) => {
                let arr = a.borrow();
                write!(f, "[")?;
                for (i, v) in arr.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{v:?}")?;
                }
                write!(f, "]")
            }
            Value::Object(o, _) => write!(f, "{:?}", o.borrow()),
            Value::Function(func) => write!(f, "<fn {}>", func.name),
            Value::BuiltinFn(func) => write!(f, "<builtin {}>", func.name),
            Value::Class(c) => write!(f, "<class {}>", c.borrow().name),
            Value::Instance(i, _) => write!(f, "<instance of {}>", i.borrow().class_name),
            Value::Signal(_, _) => write!(f, "<signal>"),
        }
    }
}

impl Value {
    /// Friendly display, used by `print`.
    pub fn display(&self) -> String {
        match self {
            Value::Undefined => "undefined".into(),
            Value::Null => "null".into(),
            Value::Number(n) => format_number(*n),
            Value::Bool(BoolVal::True) => "true".into(),
            Value::Bool(BoolVal::False) => "false".into(),
            Value::Bool(BoolVal::Maybe) => "maybe".into(),
            Value::String(s, _) => s.borrow().iter().collect(),
            Value::Array(a, _) => {
                let mut out = String::from("[");
                for (i, v) in a.borrow().iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&v.display_in_collection());
                }
                out.push(']');
                out
            }
            Value::Object(o, _) => {
                let mut out = String::from("{");
                for (i, (k, v)) in o.borrow().entries.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(k);
                    out.push_str(": ");
                    out.push_str(&v.display_in_collection());
                }
                out.push('}');
                out
            }
            Value::Function(func) => format!("<fn {}>", func.name),
            Value::BuiltinFn(func) => format!("<builtin {}>", func.name),
            Value::Class(c) => format!("<class {}>", c.borrow().name),
            Value::Instance(i, _) => format!("<instance of {}>", i.borrow().class_name),
            Value::Signal(_, _) => "<signal>".into(),
        }
    }

    /// Display variant used inside arrays/objects — quotes strings.
    pub fn display_in_collection(&self) -> String {
        match self {
            Value::String(s, _) => {
                let s: String = s.borrow().iter().collect();
                format!("\"{s}\"")
            }
            other => other.display(),
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Undefined => "undefined",
            Value::Null => "null",
            Value::Number(_) => "number",
            Value::Bool(_) => "boolean",
            Value::String(_, _) => "string",
            Value::Array(_, _) => "array",
            Value::Object(_, _) => "object",
            Value::Function(_) | Value::BuiltinFn(_) => "function",
            Value::Class(_) => "class",
            Value::Instance(_, _) => "instance",
            Value::Signal(_, _) => "signal",
        }
    }

    /// Identity for `====`. Two values share an identity iff they were minted
    /// from the same allocation. Numbers/booleans/null/undefined have no
    /// per-value identity, so `====` falls back to bit-equality.
    pub fn instance_id(&self) -> Option<InstanceId> {
        match self {
            Value::String(_, id)
            | Value::Array(_, id)
            | Value::Object(_, id)
            | Value::Instance(_, id)
            | Value::Signal(_, id) => Some(*id),
            _ => None,
        }
    }
}

fn format_number(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if n.fract() == 0.0 && n.abs() < 1e16 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Mint fresh, monotonically-increasing instance ids.
pub fn fresh_id() -> InstanceId {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    InstanceId(NEXT.fetch_add(1, Ordering::Relaxed))
}
