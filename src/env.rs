//! Variable environment.
//!
//! Models Gulf of Mexico's overload-priority semantics: a single name can be
//! bound multiple times; lookup returns the live binding with the highest
//! priority. Each binding has a [`DeclKind`] that controls whether it can be
//! reassigned or its inner value mutated, and an optional [`Lifetime`] that
//! triggers expiry after some number of executed lines or seconds.

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
    /// Sequential index of the statement that introduced this binding. Used
    /// for line-based lifetime expiry and for hoisting.
    pub created_line: usize,
    pub created_at: Instant,
    pub lifetime: Option<Lifetime>,
    /// True for the third `const` in `const const const`. Such bindings
    /// cannot be deleted or shadowed even by higher-priority bindings.
    pub eternal: bool,
}

impl Binding {
    /// Is this binding reachable on `now_line` (a 1-based statement index)?
    /// Negative line lifetimes implement *hoisting* — the binding is
    /// reachable on lines strictly before `created_line`.
    pub fn is_alive(&self, now_line: usize, now_time: Instant) -> bool {
        match &self.lifetime {
            None | Some(Lifetime::Infinity) => true,
            Some(Lifetime::Lines(n)) => {
                if *n >= 0 {
                    let n = *n as usize;
                    now_line >= self.created_line && now_line < self.created_line + n
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

/// Look up a name, walking up the scope chain. Returns the highest-priority
/// live binding for that name in the closest scope that has any.
pub fn lookup(scope: &Scope, name: &str, now_line: usize, now_time: Instant) -> Option<Value> {
    let mut current = Some(Rc::clone(scope));
    while let Some(s) = current {
        let s_ref = s.borrow();
        let mut best: Option<&Binding> = None;
        for b in &s_ref.bindings {
            if b.name != name {
                continue;
            }
            if !b.is_alive(now_line, now_time) {
                continue;
            }
            match best {
                None => best = Some(b),
                Some(prev) => {
                    if b.priority > prev.priority
                        || (b.priority == prev.priority && b.created_line > prev.created_line)
                    {
                        best = Some(b);
                    }
                }
            }
        }
        if let Some(b) = best {
            return Some(b.value.clone());
        }
        current = s_ref.parent.clone();
    }
    None
}

/// Like [`lookup`], but returns the binding metadata instead of the value, so
/// callers can inspect mutability/reassignability.
pub fn lookup_binding<R>(
    scope: &Scope,
    name: &str,
    now_line: usize,
    now_time: Instant,
    f: impl FnOnce(&Binding) -> R,
) -> Option<R> {
    let mut current = Some(Rc::clone(scope));
    while let Some(s) = current {
        let s_ref = s.borrow();
        let mut best: Option<&Binding> = None;
        for b in &s_ref.bindings {
            if b.name != name {
                continue;
            }
            if !b.is_alive(now_line, now_time) {
                continue;
            }
            match best {
                None => best = Some(b),
                Some(prev) => {
                    if b.priority > prev.priority
                        || (b.priority == prev.priority && b.created_line > prev.created_line)
                    {
                        best = Some(b);
                    }
                }
            }
        }
        if let Some(b) = best {
            return Some(f(b));
        }
        current = s_ref.parent.clone();
    }
    None
}

/// True iff a binding with this name has *ever* been declared in this scope
/// chain — used for lifetime-expiry diagnostics ("name has expired") and
/// `delete`-tombstone errors. Includes expired bindings.
pub fn was_ever_declared(scope: &Scope, name: &str) -> bool {
    let mut current = Some(Rc::clone(scope));
    while let Some(s) = current {
        let s_ref = s.borrow();
        if s_ref.bindings.iter().any(|b| b.name == name) {
            return true;
        }
        current = s_ref.parent.clone();
    }
    false
}

/// Insert a new binding in the current scope. Multiple bindings with the same
/// name are kept side-by-side to support the overload-priority semantics.
pub fn insert(scope: &Scope, binding: Binding) {
    scope.borrow_mut().bindings.push(binding);
}

/// Reassign the highest-priority live binding for `name`. Returns `Err` with
/// a message if there is no such binding.
pub fn reassign(
    scope: &Scope,
    name: &str,
    value: Value,
    now_line: usize,
    now_time: Instant,
) -> Result<DeclKind, &'static str> {
    let mut current = Some(Rc::clone(scope));
    while let Some(s) = current {
        let mut s_ref = s.borrow_mut();
        let mut best_idx: Option<usize> = None;
        let mut best_prio = i32::MIN;
        let mut best_line = 0usize;
        for (i, b) in s_ref.bindings.iter().enumerate() {
            if b.name != name {
                continue;
            }
            if !b.is_alive(now_line, now_time) {
                continue;
            }
            if b.priority > best_prio
                || (b.priority == best_prio && b.created_line > best_line)
            {
                best_prio = b.priority;
                best_line = b.created_line;
                best_idx = Some(i);
            }
        }
        if let Some(i) = best_idx {
            let decl = s_ref.bindings[i].decl;
            s_ref.bindings[i].value = value;
            return Ok(decl);
        }
        let parent = s_ref.parent.clone();
        drop(s_ref);
        current = parent;
    }
    Err("no such binding to reassign")
}
