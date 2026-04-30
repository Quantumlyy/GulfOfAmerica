//! Language Server Protocol implementation.
//!
//! Feature-gated behind `lsp`. The thin `gulf-lsp` binary just wires this
//! module up to stdio. Pure helpers (position math, diagnostics conversion,
//! symbol/definition extraction) are exposed so they can be unit-tested
//! without spinning up an actual JSON-RPC session.

#![allow(clippy::cast_possible_truncation, clippy::needless_pass_by_value)]

use std::collections::HashMap;
use std::sync::Mutex;

use tower_lsp::jsonrpc::Result as JsonResult;
use tower_lsp::lsp_types::{
    Diagnostic as LspDiagnostic, DiagnosticSeverity, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, InitializedParams, Location, MarkupContent, MarkupKind,
    MessageType, NumberOrString, OneOf, Position, Range, ServerCapabilities, ServerInfo,
    SymbolKind, TextDocumentSyncCapability, TextDocumentSyncKind, Url,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

use crate::ast::{BindingTarget, ClassMember, Stmt};
use crate::source::SourceFile;
use crate::{Diagnostic as GulfDiagnostic, Span};

// ---------------------------------------------------------------------------
// Server entry point.
// ---------------------------------------------------------------------------

/// Run the LSP server, speaking JSON-RPC over stdio. Blocks until the client
/// disconnects.
pub async fn run() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[derive(Debug)]
pub struct Backend {
    client: Client,
    docs: Mutex<HashMap<Url, String>>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            docs: Mutex::new(HashMap::new()),
        }
    }

    fn put_doc(&self, uri: Url, text: String) {
        self.docs
            .lock()
            .expect("docs mutex poisoned")
            .insert(uri, text);
    }

    fn get_doc(&self, uri: &Url) -> Option<String> {
        self.docs
            .lock()
            .expect("docs mutex poisoned")
            .get(uri)
            .cloned()
    }

    fn drop_doc(&self, uri: &Url) {
        self.docs.lock().expect("docs mutex poisoned").remove(uri);
    }

    async fn refresh_diagnostics(&self, uri: Url, text: &str) {
        let diags = compute_diagnostics(text);
        self.client.publish_diagnostics(uri, diags, None).await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _params: InitializeParams) -> JsonResult<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                definition_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "gulf-lsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "gulf-lsp initialized")
            .await;
    }

    async fn shutdown(&self) -> JsonResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let text = params.text_document.text;
        self.put_doc(uri.clone(), text.clone());
        self.refresh_diagnostics(uri, &text).await;
    }

    async fn did_change(&self, mut params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        if let Some(change) = params.content_changes.pop() {
            self.put_doc(uri.clone(), change.text.clone());
            self.refresh_diagnostics(uri, &change.text).await;
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        if let Some(text) = params.text {
            let uri = params.text_document.uri.clone();
            self.put_doc(uri.clone(), text.clone());
            self.refresh_diagnostics(uri, &text).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.drop_doc(&uri);
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }

    async fn hover(&self, params: HoverParams) -> JsonResult<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let Some(text) = self.get_doc(&uri) else {
            return Ok(None);
        };
        Ok(hover_at(&text, pos))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> JsonResult<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;
        let Some(text) = self.get_doc(&uri) else {
            return Ok(None);
        };
        Ok(Some(DocumentSymbolResponse::Nested(collect_symbols(&text))))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> JsonResult<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let Some(text) = self.get_doc(&uri) else {
            return Ok(None);
        };
        let offset = byte_offset_at(&text, pos);
        let Some((word, _)) = word_at(&text, offset) else {
            return Ok(None);
        };
        let Some(span) = find_definition(&text, &word) else {
            return Ok(None);
        };
        Ok(Some(GotoDefinitionResponse::Scalar(Location {
            uri,
            range: range_from_span(&text, span),
        })))
    }
}

// ---------------------------------------------------------------------------
// Position / span / range conversions.
// ---------------------------------------------------------------------------

/// LSP `Position` -> byte offset into `text`. LSP positions count UTF-16
/// code units within a line, so we honour that for non-ASCII.
pub fn byte_offset_at(text: &str, pos: Position) -> usize {
    let mut current_line: u32 = 0;
    let mut line_start: usize = 0;
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if current_line == pos.line {
            break;
        }
        if b == b'\n' {
            current_line += 1;
            line_start = i + 1;
        }
    }
    if current_line < pos.line {
        return text.len();
    }
    let line_slice = &text[line_start..];
    let mut col: u32 = 0;
    for (i, ch) in line_slice.char_indices() {
        if col >= pos.character {
            return line_start + i;
        }
        col += ch.len_utf16() as u32;
        if ch == '\n' {
            return line_start + i;
        }
    }
    text.len()
}

/// Byte offset -> LSP `Position`.
pub fn position_at(text: &str, offset: usize) -> Position {
    let offset = offset.min(text.len());
    let prefix = &text[..offset];
    let line: u32 = prefix.bytes().filter(|&b| b == b'\n').count() as u32;
    let last_nl = prefix.rfind('\n').map_or(0, |i| i + 1);
    let col_text = &prefix[last_nl..];
    let character: u32 = col_text.chars().map(|c| c.len_utf16() as u32).sum();
    Position { line, character }
}

pub fn range_from_span(text: &str, span: Span) -> Range {
    Range {
        start: position_at(text, span.start),
        end: position_at(text, span.end),
    }
}

// ---------------------------------------------------------------------------
// Diagnostics.
// ---------------------------------------------------------------------------

pub fn compute_diagnostics(text: &str) -> Vec<LspDiagnostic> {
    let file = SourceFile::new("(buffer)".into(), text.to_string());
    let tokens = match crate::lexer::lex(&file) {
        Ok(t) => t,
        Err(d) => return vec![lsp_diag_from(text, &d)],
    };
    let (_, parse_diags) = crate::parser::parse_recovering(&file, tokens);
    parse_diags
        .iter()
        .map(|d| lsp_diag_from(text, d))
        .collect()
}

fn lsp_diag_from(text: &str, d: &GulfDiagnostic) -> LspDiagnostic {
    let primary_span = d
        .labels
        .iter()
        .find(|l| l.primary)
        .or_else(|| d.labels.first())
        .map_or(Span::DUMMY, |l| l.span);
    let mut message = d.message.clone();
    for note in &d.notes {
        message.push_str("\n\nnote: ");
        message.push_str(note);
    }
    LspDiagnostic {
        range: range_from_span(text, primary_span),
        severity: Some(DiagnosticSeverity::ERROR),
        code: d.code.map(|c| NumberOrString::String(c.to_string())),
        code_description: None,
        source: Some("gulf".into()),
        message,
        related_information: None,
        tags: None,
        data: None,
    }
}

// ---------------------------------------------------------------------------
// Word-at-cursor.
// ---------------------------------------------------------------------------

pub fn word_at(text: &str, offset: usize) -> Option<(String, Span)> {
    let bytes = text.as_bytes();
    if offset > bytes.len() {
        return None;
    }
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut start = offset;
    while start > 0 && is_ident(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = offset;
    while end < bytes.len() && is_ident(bytes[end]) {
        end += 1;
    }
    if start == end {
        return None;
    }
    Some((text[start..end].to_string(), Span::new(start, end)))
}

// ---------------------------------------------------------------------------
// Hover.
// ---------------------------------------------------------------------------

pub fn hover_at(text: &str, pos: Position) -> Option<Hover> {
    let offset = byte_offset_at(text, pos);
    let (word, span) = word_at(text, offset)?;
    let docs = lookup_docs(&word)?;
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: docs.into(),
        }),
        range: Some(range_from_span(text, span)),
    })
}

pub fn lookup_docs(word: &str) -> Option<&'static str> {
    Some(match word {
        "function" | "fn" | "fun" | "func" | "functi" | "f" => {
            "**function** — declares a function. All of `f`, `fn`, `fun`, `func`, `functi`, and `function` are accepted; pick your favourite."
        }
        "class" => {
            "**class** — declares a class. Only one instance per class is allowed; for factories, use a function that returns an object."
        }
        "const" => {
            "**const** — outermost binding qualifier. Combine: `const const` (immutable), `const var` (reassignable inner), `const const const` (eternal: cannot be deleted or shadowed)."
        }
        "var" => {
            "**var** — outermost binding qualifier. Combine: `var const` (reassignable, inner immutable) or `var var` (fully mutable)."
        }
        "when" => {
            "**when** — installs a watcher. The condition is re-checked after every statement; a rising edge runs the body."
        }
        "import" => {
            "**import** — pulls a binding into the current `=====`-separated file. Resolves user exports first, then std packages (currently `http`)."
        }
        "export" => {
            "**export** — `export <name> to \"<file>\"!` deposits a binding for `import <name>!` in the named file."
        }
        "to" => "**to** — used by `export <name> to \"<file>\"!` to name the target file.",
        "delete" => {
            "**delete** — tombstones a primitive or name. Subsequent uses are an error."
        }
        "reverse" => {
            "**reverse!** — flips the remaining statements in the current file end-to-end."
        }
        "if" => "**if** — conditional statement.",
        "else" => "**else** — alternate branch of an `if`.",
        "return" => "**return** — exits the enclosing function with a value.",
        "async" => {
            "**async** — declares an async function. Un-`await`-ed calls run line-interleaved with the main thread; `await`-ing runs synchronously and yields the result."
        }
        "await" => "**await** — runs an async function synchronously and yields its result.",
        "new" => {
            "**new** — instantiates a class. Only one instance is allowed per class; reusing `new` on the same class is a diagnostic."
        }
        "true" => "**true** — boolean literal.",
        "false" => "**false** — boolean literal.",
        "maybe" => "**maybe** — third boolean value. Equal to anything under `==` (the JS-style level).",
        "null" => "**null** — literal.",
        "undefined" => "**undefined** — literal.",
        "use" => {
            "**use(initial)** — produces a signal value. `[get, set] = use(0)` destructures into a getter/setter pair sharing one cell."
        }
        "previous" => "**previous x** — value of `x` immediately before its last reassignment.",
        "next" => "**next x** — peeks at the next assignment to `x` in the file.",
        "current" => "**current x** — current value of `x` (== `x`).",
        "print" => "**print(...)** — built-in. Prints space-separated arguments followed by a newline.",
        "http" => {
            "**http** — std package. `http.get(url)`, `http.post(url, body)`, `http.request({method, url, body, headers})`, `http.serve(addr, handler)`, `http.serve_once(addr, handler)`. Plain HTTP/1.1; no TLS."
        }
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Document symbols.
// ---------------------------------------------------------------------------

pub fn collect_symbols(text: &str) -> Vec<DocumentSymbol> {
    let file = SourceFile::new("(buffer)".into(), text.to_string());
    let Ok(tokens) = crate::lexer::lex(&file) else {
        return Vec::new();
    };
    // Recovering parser: emit symbols for whatever statements survived,
    // even if the file has errors elsewhere.
    let (program, _diags) = crate::parser::parse_recovering(&file, tokens);
    let mut out = Vec::new();
    for f in &program.files {
        for stmt in &f.stmts {
            if let Some(sym) = symbol_for(text, stmt) {
                out.push(sym);
            }
        }
    }
    out
}

fn symbol_for(text: &str, stmt: &Stmt) -> Option<DocumentSymbol> {
    match stmt {
        Stmt::FnDecl {
            name, span, is_async, ..
        } => Some(make_symbol(
            text,
            name,
            if *is_async { "async function" } else { "function" },
            SymbolKind::FUNCTION,
            *span,
            None,
        )),
        Stmt::ClassDecl {
            name,
            members,
            span,
        } => {
            let mut children = Vec::new();
            for m in members {
                children.push(match m {
                    ClassMember::Method {
                        name,
                        span,
                        is_async,
                        ..
                    } => make_symbol(
                        text,
                        name,
                        if *is_async { "async method" } else { "method" },
                        SymbolKind::METHOD,
                        *span,
                        None,
                    ),
                    ClassMember::Field {
                        name, span, decl, ..
                    } => make_symbol(text, name, decl.label(), SymbolKind::FIELD, *span, None),
                });
            }
            Some(make_symbol(
                text,
                name,
                "class",
                SymbolKind::CLASS,
                *span,
                Some(children),
            ))
        }
        Stmt::Let {
            target:
                BindingTarget::Ident {
                    name,
                    span: ident_span,
                },
            decl,
            span,
            ..
        } => {
            let mut sym = make_symbol(
                text,
                name,
                decl.label(),
                SymbolKind::VARIABLE,
                *span,
                None,
            );
            sym.selection_range = range_from_span(text, *ident_span);
            Some(sym)
        }
        _ => None,
    }
}

#[allow(deprecated)] // DocumentSymbol::deprecated is deprecated but still required by the type.
fn make_symbol(
    text: &str,
    name: &str,
    detail: &str,
    kind: SymbolKind,
    span: Span,
    children: Option<Vec<DocumentSymbol>>,
) -> DocumentSymbol {
    let range = range_from_span(text, span);
    DocumentSymbol {
        name: name.to_string(),
        detail: Some(detail.to_string()),
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range: range,
        children,
    }
}

// ---------------------------------------------------------------------------
// Goto-definition.
// ---------------------------------------------------------------------------

pub fn find_definition(text: &str, name: &str) -> Option<Span> {
    let file = SourceFile::new("(buffer)".into(), text.to_string());
    let tokens = crate::lexer::lex(&file).ok()?;
    let (program, _) = crate::parser::parse_recovering(&file, tokens);
    for f in &program.files {
        for stmt in &f.stmts {
            if let Some(s) = decl_span_in_stmt(stmt, name) {
                return Some(s);
            }
        }
    }
    None
}

/// Find the span of a declaration named `name`, descending into function
/// bodies, class methods, and `if` / `when` blocks. The first match wins —
/// good enough for navigation; not a real resolver.
fn decl_span_in_stmt(stmt: &Stmt, name: &str) -> Option<Span> {
    use crate::ast::{Block, ClassMember, FnBody};

    fn walk_block(block: &Block, name: &str) -> Option<Span> {
        for s in &block.stmts {
            if let Some(s) = decl_span_in_stmt(s, name) {
                return Some(s);
            }
        }
        None
    }
    fn walk_body(body: &FnBody, name: &str) -> Option<Span> {
        match body {
            FnBody::Expr(_) => None,
            FnBody::Block(b) => walk_block(b, name),
        }
    }

    match stmt {
        Stmt::FnDecl {
            name: n,
            span,
            body,
            ..
        } => {
            if n == name {
                return Some(*span);
            }
            walk_body(body, name)
        }
        Stmt::ClassDecl {
            name: n,
            members,
            span,
        } => {
            if n == name {
                return Some(*span);
            }
            for m in members {
                match m {
                    ClassMember::Method {
                        name: mn,
                        span,
                        body,
                        ..
                    } => {
                        if mn == name {
                            return Some(*span);
                        }
                        if let Some(s) = walk_body(body, name) {
                            return Some(s);
                        }
                    }
                    ClassMember::Field {
                        name: fname, span, ..
                    } => {
                        if fname == name {
                            return Some(*span);
                        }
                    }
                }
            }
            None
        }
        Stmt::Let {
            target: BindingTarget::Ident { name: n, span },
            ..
        } if n == name => Some(*span),
        Stmt::If {
            then_block,
            else_block,
            ..
        } => walk_block(then_block, name).or_else(|| {
            else_block
                .as_ref()
                .and_then(|b| walk_block(b, name))
        }),
        Stmt::When { block, .. } => walk_block(block, name),
        _ => None,
    }
}
