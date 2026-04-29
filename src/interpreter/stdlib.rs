//! Built-in standard packages, surfaced to programs via `import <name>!`.
//!
//! User-level `export ... to "<file>"!` continues to take precedence. Only
//! when the per-file exports table has no entry for a given name does the
//! interpreter consult this registry. That keeps the existing import
//! semantics intact and lets stdlib names be shadowed by user programs.

pub mod http;

use crate::value::Value;

pub fn lookup(name: &str) -> Option<Value> {
    match name {
        "http" => Some(http::module()),
        _ => None,
    }
}
