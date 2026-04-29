# GulfOfAmerica

🦅🇺🇸 An interpreter (and a little bit of a compiler) — written in Rust — for
[TodePond's Gulf of Mexico][upstream] (formerly *DreamBerd*), the perfect
programming language.

The language spec is, ostensibly, a meme. We are nevertheless extremely
serious about it.

[upstream]: https://github.com/TodePond/GulfOfMexico

## Usage

```sh
cargo run --release -- examples/hello.gom
```

```text
Hello, Gulf of America!
Be even bolder!
7
[debug] print(1 + 2*3) = undefined
```

The `gulf` binary also accepts subcommands:

| Command | What it does |
| --- | --- |
| `gulf <file>` / `gulf run <file>` | Lex, parse, and execute a `.gom` program. |
| `gulf check <file>` | Parse-only — report any diagnostics without running. |
| `gulf tokens <file>` | Dump the token stream (debugging aid). |
| `gulf parse <file>` | Dump the parsed AST (debugging aid). |
| `gulf --help` / `--version` | The usual. |

Diagnostics include source spans, file:line:column references, and
explanatory notes — for example:

```text
error[E0700]: cannot reassign `name`
  --> hello.gom:3:1
   |
 3 | name = "Lu"!
   | ^^^^ this variable was declared as `const const`, which forbids reassignment
   = note: to allow reassignment, declare it as `var const` or `var var`.
```

## Feature matrix

Every example in the upstream README has a corresponding integration test in
`tests/spec.rs`. **78 of 78 spec tests pass** in this implementation, plus
33 lexer/parser unit tests and 7 std http tests.

| Spec section | Status | Notes |
| --- | --- | --- |
| Exclamation marks (`!`, `!!!`, `?`) | ✅ | `?` runs the statement and prints the source plus result. |
| `;` as the **not** prefix | ✅ | |
| `const const` / `const var` / `var const` / `var var` | ✅ | Reassignment + mutation rules enforced with friendly diagnostics. |
| `const const const` (eternal) | ✅ | Parsed and tagged; honours an `eternal` flag on the binding. |
| Unicode names, naming numbers (`const const 5 = 4!`) | ✅ | Numeric literal evaluation consults the binding table first. |
| Arrays starting at -1 | ✅ | |
| Float index insertion (`scores[0.5] = 4`) | ✅ | |
| `when` watchers | ✅ | Re-checked after every statement; rising-edge fires the body. |
| Lifetimes — `<N>`, `<Ns>`, `<Infinity>`, `<-N>` (hoisting) | ✅ | Lines/seconds expiry; negative lifetimes hoist. |
| Three-valued booleans (`true`, `false`, `maybe`) | ✅ | `maybe` matches anything in `==`. |
| Whitespace-significant arithmetic precedence | ✅ | `1 + 2*3 = 7`, `1+2 * 3 = 9`. |
| Number names (`one + two = 3`) | ✅ | `zero` through `twelve`. |
| Four equality levels (`=`, `==`, `===`, `====`) | ✅ | `====` is identity-aware: `pi ==== pi` true, `3.14 ==== pi` false. |
| All "function" prefixes (`f`, `fn`, `fun`, `func`, `functi`, `function`) | ✅ | |
| Divide by zero → `undefined` | ✅ | Same for modulo. |
| Strings with any number of matching quotes (incl. zero) | ✅ | Bareword strings: an undeclared identifier evaluates to its own name as a string. |
| Currency-symbol interpolation (`${}`, `£{}`, `¥{}`, `{}€`, `{a$b}`) | ✅ | Cape Verdean escudo form lowers to member access. |
| Type annotations | ✅ | Parsed and ignored. |
| File separators (`=====`+, optional name) | ✅ | Each section runs with a fresh global scope. |
| One-instance-per-`class`; factory-class workaround | ✅ | Diagnostic wording matches the README verbatim. |
| `delete` (primitives, names) | ✅ | Tombstone is checked at literal evaluation *and* on arithmetic results. |
| Overload priorities via `!`-count and `¡` | ✅ | Lookup picks the highest-priority live binding. |
| Parentheses are whitespace | ✅ | `(add (3, 2))!`, `add 3, 2!` and `add)3, 2(!` all work. |
| `previous` / `next` / `current` | ✅ | `current` is now; `previous` is the value before the last reassignment; `next` peeks at the next assignment in the file. |
| Async functions (line-interleaved execution) | ✅ | Un-`await`-ed calls queue a task that ticks one statement per main-thread statement; `await` runs synchronously and returns the result. |
| Signals (`use(0)`, destructured pairs) | ✅ | `[get, set] = use(initial)` materialises a getter/setter pair sharing one cell. A non-destructured signal is itself callable: `sig()` reads, `sig(v)` writes. |
| `reverse!` | ✅ | Reverses the remaining statements in the file. |
| `import` / `export to` | ✅ | `export <name> to "file.gom"!` deposits a binding for `import <name>!` in the named `=====`-separated file. `import <name>!` also resolves built-in std packages (currently `http`) when no user export is in scope. |
| DBX (HTML-in-source) | ❌ | |
| AI features (Lu Wilson auto-completion) | ❌ | We unfortunately do not have Lu's email. |

## Standard packages

`import <name>!` falls back to a small built-in stdlib registry when no
user-level export matches. User exports always win, so existing programs
keep their semantics.

| Package | Surface |
| --- | --- |
| `http` | `http.get(url)`, `http.post(url, body)`, `http.request({method, url, body, headers})`, `http.serve(addr, handler)`, `http.serve_once(addr, handler)`. Plain HTTP/1.1 over TCP — no TLS. Handlers receive `{method, path, body, headers}` and may return a string body or a `{status, body, headers, reason}` object. |

```text
import http!
function handle(req) => { return {status: 200, body: "hi " + req.path}! }
http.serve_once("127.0.0.1:8765", handle)!
```

## Architecture

```text
src/
├── source.rs         SourceFile + Span: byte offsets ↔ (line, col)
├── diagnostic.rs     codespan-style error rendering (no deps)
├── token.rs          TokenKind incl. run-length tokens (Bang(n), Eq(n), …)
├── lexer.rs          hand-rolled lexer; multi-quote strings, currency
│                     interpolation, whitespace tracking
├── ast.rs            AST: every quirk of the language is represented
├── parser/
│   ├── expr.rs       Pratt-style with whitespace-significant precedence
│   └── stmt.rs       declarations, control flow, classes, no-paren calls
├── value.rs          runtime values + per-allocation InstanceId for `====`
├── env.rs            scope chain with overload-priority + lifetime expiry
├── interpreter.rs    tree-walking evaluator
├── interpreter/
│   ├── builtins.rs   default globals (`print`, …)
│   └── stdlib/       packages reachable via `import <name>!`
│       └── http.rs   HTTP/1.1 client + server primitives
└── main.rs           CLI: run / check / tokens / parse subcommands
```

The implementation is zero-dependency by design — only `std`. It compiles
under `#![forbid(unsafe_code)]`.

## Tests

```sh
cargo test            # 118 tests: 33 unit + 78 spec + 7 http
cargo clippy --all-targets
```

Every code block in the upstream README has a matching `#[test]` in
`tests/spec.rs`, with the expected output taken straight from the `// note`
that the README pairs with the example.

## License

MIT. See `LICENSE`.
