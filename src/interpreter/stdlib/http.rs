//! Standard `http` module: a tiny HTTP/1.1 client and server.
//!
//! Imported via `import http!`. Exposes:
//!
//! * `http.get(url)` — perform a GET; returns `{status, reason, body, headers}`.
//! * `http.post(url, body)` — perform a POST.
//! * `http.request({method, url, body, headers})` — generic request.
//! * `http.serve(addr, handler)` — accept connections forever, calling the
//!   handler for each. The handler receives `{method, path, body, headers}`
//!   and may return either a string (used as the body, status 200) or an
//!   object `{status, body, headers, reason}`.
//! * `http.serve_once(addr, handler)` — handle a single request and return.
//!
//! Implementation notes:
//!
//! * Plain `http://`. TLS is intentionally out of scope — we are zero-deps.
//! * Header names are lower-cased on the way in so handlers can match
//!   `headers["host"]` regardless of how the peer cased them.
//! * Bodies are read using `Content-Length`. Chunked transfer encoding is not
//!   yet supported (responses without a length terminate at EOF, which is the
//!   common case for `Connection: close`).
//! * Server reads expect a `Content-Length` for the body if one is present.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::rc::Rc;
use std::time::Duration;

use crate::diagnostic::{Diagnostic, Label};
use crate::interpreter::Interpreter;
use crate::source::Span;
use crate::value::{fresh_id, BuiltinFn, Object, Value};

/// Build a fresh `http` module value. Each `import http!` produces its own
/// instance, but since the entries are stateless builtins this is cheap.
pub fn module() -> Value {
    let mut o = Object::new();
    o.set("get", builtin("http.get", get));
    o.set("post", builtin("http.post", post));
    o.set("request", builtin("http.request", request));
    o.set("serve", builtin("http.serve", serve_forever));
    o.set("serve_once", builtin("http.serve_once", serve_once));
    Value::Object(Rc::new(RefCell::new(o)), fresh_id())
}

fn builtin(
    name: &'static str,
    call: fn(&mut Interpreter, Vec<Value>, Span) -> Result<Value, Diagnostic>,
) -> Value {
    Value::BuiltinFn(Rc::new(BuiltinFn {
        name,
        call: Box::new(call),
    }))
}

// ---------------------------------------------------------------------------
// Diagnostic + value-construction helpers.
// ---------------------------------------------------------------------------

fn err(span: Span, msg: impl Into<String>) -> Diagnostic {
    Diagnostic::error(msg.into())
        .with_code("E0900")
        .with_label(Label::primary(span, "in this http call"))
}

fn s(text: impl Into<String>) -> Value {
    let chars: Vec<char> = text.into().chars().collect();
    Value::String(Rc::new(RefCell::new(chars)), fresh_id())
}

fn obj(entries: Vec<(&str, Value)>) -> Value {
    let mut o = Object::new();
    for (k, v) in entries {
        o.set(k, v);
    }
    Value::Object(Rc::new(RefCell::new(o)), fresh_id())
}

fn as_string(v: &Value) -> Option<String> {
    if let Value::String(chars, _) = v {
        Some(chars.borrow().iter().collect())
    } else {
        None
    }
}

fn as_object(v: &Value) -> Option<Rc<RefCell<Object>>> {
    if let Value::Object(o, _) = v {
        Some(Rc::clone(o))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// URL parsing.
// ---------------------------------------------------------------------------

struct Url {
    host: String,
    port: u16,
    path: String,
}

fn parse_url(url: &str, span: Span) -> Result<Url, Diagnostic> {
    // Fragments never go on the wire — strip them before splitting.
    let url = url.split_once('#').map_or(url, |(head, _)| head);
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(err(span, format!("malformed URL `{url}`: missing scheme")));
    };
    if scheme != "http" {
        return Err(err(
            span,
            format!("only http:// URLs are supported, got `{scheme}://`"),
        )
        .with_note("the std http module is plaintext-only; TLS is out of scope."));
    }
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => match p.parse::<u16>() {
            Ok(n) => (h.to_string(), n),
            Err(_) => return Err(err(span, format!("invalid port in URL `{url}`"))),
        },
        None => (authority.to_string(), 80u16),
    };
    Ok(Url {
        host,
        port,
        path: path.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Client.
// ---------------------------------------------------------------------------

fn get(_interp: &mut Interpreter, args: Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
    let Some(url) = args.first().and_then(as_string) else {
        return Err(err(span, "http.get expects a string URL"));
    };
    do_request("GET", &url, "", &[], span)
}

fn post(_interp: &mut Interpreter, args: Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
    let Some(url) = args.first().and_then(as_string) else {
        return Err(err(span, "http.post expects a string URL"));
    };
    let body = args.get(1).and_then(as_string).unwrap_or_default();
    do_request("POST", &url, &body, &[], span)
}

fn request(_interp: &mut Interpreter, args: Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
    let Some(opts_rc) = args.first().and_then(as_object) else {
        return Err(err(span, "http.request expects an options object"));
    };
    let opts = opts_rc.borrow();
    let method = opts
        .get("method")
        .and_then(|v| as_string(&v))
        .unwrap_or_else(|| "GET".into());
    let Some(url) = opts.get("url").and_then(|v| as_string(&v)) else {
        return Err(err(span, "http.request requires a `url` field"));
    };
    let body = opts
        .get("body")
        .and_then(|v| as_string(&v))
        .unwrap_or_default();
    let headers = collect_headers(opts.get("headers").as_ref());
    do_request(&method, &url, &body, &headers, span)
}

fn collect_headers(headers: Option<&Value>) -> Vec<(String, String)> {
    let Some(v) = headers else {
        return Vec::new();
    };
    let Some(o) = as_object(v) else {
        return Vec::new();
    };
    let inner = o.borrow();
    let mut out = Vec::with_capacity(inner.entries.len());
    for (k, v) in &inner.entries {
        out.push((k.clone(), as_string(v).unwrap_or_else(|| v.display())));
    }
    out
}

fn do_request(
    method: &str,
    url: &str,
    body: &str,
    extra_headers: &[(String, String)],
    span: Span,
) -> Result<Value, Diagnostic> {
    // Programmer errors (bad URL, unsupported scheme) bubble up as
    // Diagnostics. Transient I/O errors come back as `{ok: false, error}`
    // so user code can branch on them without aborting the program.
    let parsed = parse_url(url, span)?;
    Ok(match try_request(method, &parsed, body, extra_headers) {
        Ok(v) => v,
        Err(msg) => error_response(&msg),
    })
}

fn try_request(
    method: &str,
    parsed: &Url,
    body: &str,
    extra_headers: &[(String, String)],
) -> Result<Value, String> {
    let addr_str = format!("{}:{}", parsed.host, parsed.port);
    let addrs: Vec<_> = addr_str
        .to_socket_addrs()
        .map_err(|e| format!("could not resolve `{addr_str}`: {e}"))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("no addresses resolved for `{addr_str}`"));
    }
    let mut stream = TcpStream::connect(&addrs[..])
        .map_err(|e| format!("could not connect to `{addr_str}`: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));

    let mut req = String::new();
    req.push_str(method);
    req.push(' ');
    req.push_str(&parsed.path);
    req.push_str(" HTTP/1.1\r\n");
    req.push_str(&format!("Host: {}\r\n", parsed.host));
    req.push_str("Connection: close\r\n");
    req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    let mut sent_ct = false;
    for (k, v) in extra_headers {
        if k.eq_ignore_ascii_case("content-type") {
            sent_ct = true;
        }
        // `host`, `connection`, and `content-length` we emit ourselves;
        // user-supplied duplicates still go on the wire so explicit
        // overrides win.
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if !sent_ct && !body.is_empty() {
        req.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    }
    req.push_str("\r\n");
    req.push_str(body);
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("write failed: {e}"))?;

    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .map_err(|e| format!("read failed: {e}"))?;
    parse_response(&buf)
}

fn error_response(message: &str) -> Value {
    obj(vec![
        ("ok", Value::Bool(crate::value::BoolVal::False)),
        ("error", s(message.to_string())),
        ("status", Value::Number(0.0)),
        ("body", s(String::new())),
        (
            "headers",
            Value::Object(Rc::new(RefCell::new(Object::new())), fresh_id()),
        ),
    ])
}

fn parse_response(bytes: &[u8]) -> Result<Value, String> {
    let split = find_double_crlf(bytes)
        .ok_or_else(|| "malformed response: missing header terminator".to_string())?;
    let head = std::str::from_utf8(&bytes[..split])
        .map_err(|_| "response headers were not valid UTF-8".to_string())?;
    let body_bytes = &bytes[split + 4..];

    let mut lines = head.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| "empty response".to_string())?;
    let mut status_parts = status_line.splitn(3, ' ');
    let _version = status_parts.next();
    let status: u16 = status_parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("malformed status line: {status_line}"))?;
    let reason = status_parts.next().unwrap_or("").to_string();

    let mut headers = Object::new();
    let mut chunked = false;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_lowercase();
            let val = v.trim();
            if key == "transfer-encoding" && val.eq_ignore_ascii_case("chunked") {
                chunked = true;
            }
            headers.set(&key, s(val.to_string()));
        }
    }

    let body_bytes_owned: Vec<u8> = if chunked {
        decode_chunked(body_bytes).map_err(|e| format!("chunked decode failed: {e}"))?
    } else {
        body_bytes.to_vec()
    };
    let body_str = String::from_utf8_lossy(&body_bytes_owned).into_owned();
    Ok(obj(vec![
        ("ok", Value::Bool(crate::value::BoolVal::True)),
        ("status", Value::Number(f64::from(status))),
        ("reason", s(reason)),
        ("body", s(body_str)),
        (
            "headers",
            Value::Object(Rc::new(RefCell::new(headers)), fresh_id()),
        ),
    ]))
}

fn find_double_crlf(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Decode a `Transfer-Encoding: chunked` body. Format per RFC 7230 §4.1:
///
/// ```text
/// chunk          = chunk-size [ chunk-ext ] CRLF chunk-data CRLF
/// last-chunk     = 1*("0") [ chunk-ext ] CRLF
/// trailer-part   = *( header-field CRLF )
/// ```
///
/// We ignore chunk extensions and trailer headers. Returns the concatenated
/// chunk data on success.
fn decode_chunked(mut bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let line_end = bytes
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| "expected chunk size".to_string())?;
        let size_line = std::str::from_utf8(&bytes[..line_end])
            .map_err(|_| "chunk size was not UTF-8".to_string())?;
        let size_hex = size_line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_hex, 16)
            .map_err(|_| format!("invalid chunk size `{size_hex}`"))?;
        bytes = &bytes[line_end + 2..];
        if size == 0 {
            // Optional trailer-part follows; we ignore it. End on the
            // mandatory blank line.
            return Ok(out);
        }
        if bytes.len() < size + 2 {
            return Err("chunk body truncated".into());
        }
        out.extend_from_slice(&bytes[..size]);
        if &bytes[size..size + 2] != b"\r\n" {
            return Err("chunk not terminated by CRLF".into());
        }
        bytes = &bytes[size + 2..];
    }
}

// ---------------------------------------------------------------------------
// Server.
// ---------------------------------------------------------------------------

fn serve_forever(
    interp: &mut Interpreter,
    args: Vec<Value>,
    span: Span,
) -> Result<Value, Diagnostic> {
    let (addr, handler) = parse_serve_args(&args, span)?;
    let listener = bind(&addr, span)?;
    loop {
        let (stream, _peer) = listener
            .accept()
            .map_err(|e| err(span, format!("accept failed: {e}")))?;
        handle_connection(interp, stream, &handler, span)?;
    }
}

fn serve_once(
    interp: &mut Interpreter,
    args: Vec<Value>,
    span: Span,
) -> Result<Value, Diagnostic> {
    let (addr, handler) = parse_serve_args(&args, span)?;
    let listener = bind(&addr, span)?;
    let (stream, _peer) = listener
        .accept()
        .map_err(|e| err(span, format!("accept failed: {e}")))?;
    handle_connection(interp, stream, &handler, span)?;
    Ok(Value::Undefined)
}

fn parse_serve_args(args: &[Value], span: Span) -> Result<(String, Value), Diagnostic> {
    let Some(addr) = args.first().and_then(as_string) else {
        return Err(err(span, "expected an address string as the first argument"));
    };
    let handler = args
        .get(1)
        .cloned()
        .ok_or_else(|| err(span, "expected a handler function as the second argument"))?;
    if !matches!(handler, Value::Function(_) | Value::BuiltinFn(_)) {
        return Err(err(
            span,
            format!(
                "handler must be a function, got {}",
                handler.type_name()
            ),
        ));
    }
    Ok((addr, handler))
}

fn bind(addr: &str, span: Span) -> Result<TcpListener, Diagnostic> {
    TcpListener::bind(addr).map_err(|e| err(span, format!("could not bind to `{addr}`: {e}")))
}

fn handle_connection(
    interp: &mut Interpreter,
    mut stream: TcpStream,
    handler: &Value,
    span: Span,
) -> Result<(), Diagnostic> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let request = match read_request(&mut stream) {
        Ok(r) => r,
        Err(e) => {
            let _ = write_response(&mut stream, 400, "Bad Request", &[], e.as_bytes());
            return Ok(());
        }
    };
    let result = interp.invoke_value(handler.clone(), vec![request], span)?;
    let (status, reason, body, headers) = response_from_value(&result);
    let _ = write_response(&mut stream, status, &reason, &headers, body.as_bytes());
    Ok(())
}

fn read_request(stream: &mut TcpStream) -> Result<Value, String> {
    let cloned = stream.try_clone().map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(cloned);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    let line = line.trim_end_matches(['\r', '\n']);
    let mut parts = line.splitn(3, ' ');
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let _version = parts.next().unwrap_or("");

    let mut headers = Object::new();
    let mut content_length: usize = 0;
    loop {
        let mut h = String::new();
        let n = reader.read_line(&mut h).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        let h = h.trim_end_matches(['\r', '\n']);
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            let key = k.trim().to_lowercase();
            let val = v.trim().to_string();
            if key == "content-length" {
                content_length = val.parse().unwrap_or(0);
            }
            headers.set(&key, s(val));
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).map_err(|e| e.to_string())?;
    }
    let body_str = String::from_utf8_lossy(&body).into_owned();
    Ok(obj(vec![
        ("method", s(method)),
        ("path", s(path)),
        ("body", s(body_str)),
        (
            "headers",
            Value::Object(Rc::new(RefCell::new(headers)), fresh_id()),
        ),
    ]))
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
    let mut sent_cl = false;
    let mut sent_ct = false;
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("content-length") {
            sent_cl = true;
        }
        if k.eq_ignore_ascii_case("content-type") {
            sent_ct = true;
        }
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if !sent_cl {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    if !sent_ct {
        head.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    }
    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn response_from_value(v: &Value) -> (u16, String, String, Vec<(String, String)>) {
    match v {
        Value::Object(o, _) => {
            let o = o.borrow();
            let status = o
                .get("status")
                .and_then(|s| match s {
                    Value::Number(n) => Some(n as u16),
                    _ => None,
                })
                .unwrap_or(200);
            let reason = o
                .get("reason")
                .and_then(|v| as_string(&v))
                .unwrap_or_else(|| status_reason(status).to_string());
            let body = o
                .get("body")
                .map(|v| match v {
                    Value::String(_, _) => as_string(&v).unwrap_or_default(),
                    other => other.display(),
                })
                .unwrap_or_default();
            let headers = collect_headers(o.get("headers").as_ref());
            (status, reason, body, headers)
        }
        Value::String(_, _) => (
            200,
            "OK".to_string(),
            as_string(v).unwrap_or_default(),
            Vec::new(),
        ),
        Value::Undefined | Value::Null => (200, "OK".to_string(), String::new(), Vec::new()),
        other => (200, "OK".to_string(), other.display(), Vec::new()),
    }
}

fn status_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        418 => "I'm a teapot",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "OK",
    }
}
