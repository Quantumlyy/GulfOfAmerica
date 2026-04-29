//! Source files and byte spans.

use std::fmt;

/// A half-open byte range `[start, end)` inside a single source file.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const DUMMY: Span = Span { start: 0, end: 0 };

    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub const fn point(at: usize) -> Self {
        Self {
            start: at,
            end: at,
        }
    }

    pub fn merge(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    pub const fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub const fn is_empty(self) -> bool {
        self.start >= self.end
    }
}

impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

/// A loaded source file plus a precomputed line index.
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub name: String,
    pub text: String,
    line_starts: Vec<usize>,
}

impl SourceFile {
    pub fn new(name: String, text: String) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self {
            name,
            text,
            line_starts,
        }
    }

    /// 1-based (line, column) for a byte offset. Column is 1-based and counts
    /// `char` widths (not bytes), which is good enough for ASCII-leaning code.
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.text.len());
        let line_idx = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let line_start = self.line_starts[line_idx];
        let prefix = &self.text.as_bytes()[line_start..offset];
        // Count chars by walking the slice as &str.
        let prefix_str = std::str::from_utf8(prefix).unwrap_or("");
        let col = prefix_str.chars().count() + 1;
        (line_idx + 1, col)
    }

    pub fn line_text(&self, line_1based: usize) -> &str {
        if line_1based == 0 || line_1based > self.line_starts.len() {
            return "";
        }
        let start = self.line_starts[line_1based - 1];
        let end = if line_1based < self.line_starts.len() {
            self.line_starts[line_1based].saturating_sub(1)
        } else {
            self.text.len()
        };
        let end = end.min(self.text.len());
        // Trim trailing carriage return if present.
        let slice = &self.text[start..end];
        slice.strip_suffix('\r').unwrap_or(slice)
    }

    pub fn slice(&self, span: Span) -> &str {
        let end = span.end.min(self.text.len());
        let start = span.start.min(end);
        &self.text[start..end]
    }
}
