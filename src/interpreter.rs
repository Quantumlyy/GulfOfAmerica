//! Interpreter (placeholder — full implementation follows in a later commit).

use crate::ast::Program;
use crate::diagnostic::Diagnostic;
use crate::source::SourceFile;

#[derive(Debug, Default)]
pub struct RunOutcome {
    pub output: String,
}

#[derive(Default)]
pub struct Interpreter {
    pub output: String,
    pub line_counter: usize,
}

impl Interpreter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn run(&mut self, _file: &SourceFile, _program: &Program) -> Result<RunOutcome, Diagnostic> {
        Err(Diagnostic::error("interpreter not yet implemented"))
    }
}
