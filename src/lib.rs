//! Gulf of America — an interpreter for TodePond's Gulf of Mexico / DreamBerd.
//!
//! The language is a perfect language. Therefore, this is a perfect interpreter.
//!
//! See <https://github.com/TodePond/GulfOfMexico> for the spec.
//! See [`README.md`](https://github.com/quantumlyy/gulfofamerica) for which parts
//! of that spec we (a) implement, (b) approximate, or (c) lovingly decline to
//! implement on grounds of physical impossibility.
#![forbid(unsafe_code)]

pub mod ast;
pub mod diagnostic;
pub mod env;
pub mod interpreter;
pub mod lexer;
#[cfg(feature = "lsp")]
pub mod lsp;
pub mod parser;
pub mod source;
pub mod token;
pub mod value;

pub use diagnostic::{Diagnostic, Label, Severity};
pub use interpreter::{Interpreter, RunOutcome};
pub use source::{SourceFile, Span};

/// Convenience: lex, parse, and run a single program.
///
/// Returns the captured `print` / `?` output on success, or a rendered
/// diagnostic on failure.
pub fn run(source: &str, name: &str) -> Result<String, String> {
    let file = SourceFile::new(name.to_owned(), source.to_owned());
    let tokens = match lexer::lex(&file) {
        Ok(t) => t,
        Err(diag) => return Err(diag.render(&file)),
    };
    let program = match parser::parse(&file, tokens) {
        Ok(p) => p,
        Err(diag) => return Err(diag.render(&file)),
    };
    let mut interp = Interpreter::new();
    match interp.run(&file, &program) {
        Ok(RunOutcome { output, .. }) => Ok(output),
        Err(diag) => Err(diag.render(&file)),
    }
}
