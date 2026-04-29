//! User-facing diagnostics with source rendering.
//!
//! Errors look like:
//!
//! ```text
//! error[E0123]: unterminated string literal
//!   --> hello.gom:3:14
//!    |
//!  3 |   print("Hello)!
//!    |        ^^^^^^^ this string was opened with `"` but never closed
//!    = note: did you forget to close it? AQMI says it'll do that for you, but I am not AQMI.
//! ```

use std::fmt::Write;

use crate::source::{SourceFile, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Label {
    pub span: Span,
    pub message: String,
    pub primary: bool,
}

impl Label {
    pub fn primary(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
            primary: true,
        }
    }

    pub fn secondary(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
            primary: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: Option<&'static str>,
    pub message: String,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code: None,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code: None,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }

    pub fn with_label(mut self, label: Label) -> Self {
        self.labels.push(label);
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Render the diagnostic against a source file. Plain text, no ANSI.
    pub fn render(&self, file: &SourceFile) -> String {
        let mut out = String::new();
        let header_label = self.severity.label();
        match self.code {
            Some(code) => write!(&mut out, "{header_label}[{code}]: {}", self.message).unwrap(),
            None => write!(&mut out, "{header_label}: {}", self.message).unwrap(),
        }
        out.push('\n');

        // Find the "primary" label (first one) for the location header.
        let primary = self
            .labels
            .iter()
            .find(|l| l.primary)
            .or_else(|| self.labels.first());
        if let Some(label) = primary {
            let (line, col) = file.line_col(label.span.start);
            writeln!(&mut out, "  --> {}:{}:{}", file.name, line, col).unwrap();
        }

        // Group labels by line and render each line block.
        let mut by_line: std::collections::BTreeMap<usize, Vec<&Label>> =
            std::collections::BTreeMap::new();
        for label in &self.labels {
            let (line, _) = file.line_col(label.span.start);
            by_line.entry(line).or_default().push(label);
        }
        if !by_line.is_empty() {
            let max_line = *by_line.keys().max().unwrap();
            let gutter = max_line.to_string().len();
            writeln!(&mut out, "{:>w$} |", "", w = gutter).unwrap();
            for (line, labels) in &by_line {
                let line_text = file.line_text(*line);
                writeln!(&mut out, "{:>w$} | {}", line, line_text, w = gutter).unwrap();
                // Underline using the first label's span on this line.
                for label in labels {
                    let (_, start_col) = file.line_col(label.span.start);
                    let (end_line, end_col) = file.line_col(label.span.end);
                    let end_col_on_line = if end_line == *line {
                        end_col
                    } else {
                        // Span crosses the line: underline to end of line.
                        line_text.chars().count() + 1
                    };
                    let underline_len = end_col_on_line.saturating_sub(start_col).max(1);
                    let pad = start_col.saturating_sub(1);
                    let marker = if label.primary { '^' } else { '-' };
                    let underline: String = std::iter::repeat(marker).take(underline_len).collect();
                    let prefix: String = std::iter::repeat(' ').take(pad).collect();
                    if label.message.is_empty() {
                        writeln!(
                            &mut out,
                            "{:>w$} | {prefix}{underline}",
                            "",
                            w = gutter
                        )
                        .unwrap();
                    } else {
                        writeln!(
                            &mut out,
                            "{:>w$} | {prefix}{underline} {}",
                            "",
                            label.message,
                            w = gutter
                        )
                        .unwrap();
                    }
                }
            }
        }

        for note in &self.notes {
            // Indent multi-line notes nicely.
            let mut lines = note.lines();
            if let Some(first) = lines.next() {
                writeln!(&mut out, "  = note: {first}").unwrap();
                for rest in lines {
                    writeln!(&mut out, "          {rest}").unwrap();
                }
            }
        }

        out
    }
}

/// Convenience constructor used throughout the crate.
pub fn err(span: Span, label: &str, message: impl Into<String>) -> Diagnostic {
    let message = message.into();
    Diagnostic::error(message.clone()).with_label(Label::primary(span, label.to_owned()))
}
