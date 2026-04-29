//! Variable environment.
//!
//! The environment models Gulf of Mexico's overload-priority semantics:
//! a single name can be bound multiple times; lookup returns the live binding
//! with the highest priority. Each binding has a [`DeclKind`] that controls
//! whether it can be reassigned or its inner value mutated, and an optional
//! [`Lifetime`] that triggers expiry after some number of executed lines or
//! seconds.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use crate::ast::{DeclKind, Lifetime};
use crate::value::Value;

#[derive(Debug, Clone)]
pub struct Binding {
    pub name: String,
    pub value: Value,
    pub decl: DeclKind,
    pub priority: i32,
    pub created_line: usize,
    pub created_at: Instant,
    pub lifetime: Option<Lifetime>,
    /// True for the third `const` in `const const const`. Such bindings cannot
    /// be deleted or shadowed even by higher-priority bindings.
    pub eternal: bool,
}

impl Binding {
    pub fn is_alive(&self, now_line: usize, now_time: Instant) -> bool {
        match &self.lifetime {
            None | Some(Lifetime::Infinity) => true,
            Some(Lifetime::Lines(n)) => {
                if *n >= 0 {
                    now_line < self.created_line + (*n as usize)
                } else {
                    // Negative lifetime: alive *before* the declaration line.
                    now_line < self.created_line
                }
            }
            Some(Lifetime::Seconds(s)) => {
                let elapsed = now_time.saturating_duration_since(self.created_at);
                elapsed.as_secs_f64() < *s
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct ScopeData {
    pub bindings: Vec<Binding>,
    pub parent: Option<Scope>,
}

pub type Scope = Rc<RefCell<ScopeData>>;

pub fn new_scope() -> Scope {
    Rc::new(RefCell::new(ScopeData::default()))
}

pub fn child_scope(parent: &Scope) -> Scope {
    Rc::new(RefCell::new(ScopeData {
        bindings: Vec::new(),
        parent: Some(Rc::clone(parent)),
    }))
}
