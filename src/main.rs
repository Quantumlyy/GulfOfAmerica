//! Command-line entry point for the `gulf` interpreter.
//!
//! Usage:
//!
//! ```text
//! gulf <file.gom>           # run a program (default subcommand)
//! gulf run <file.gom>       # ditto, explicitly
//! gulf check <file.gom>     # parse-only; report errors but do not execute
//! gulf tokens <file.gom>    # dump the token stream (debugging aid)
//! gulf parse <file.gom>     # dump the AST (debugging aid)
//! gulf --version | -V
//! gulf --help    | -h
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use gulf::{lexer, parser, Interpreter, SourceFile};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        print_usage();
        return ExitCode::from(2);
    }
    match args[0].as_str() {
        "-h" | "--help" => {
            print_usage();
            ExitCode::SUCCESS
        }
        "-V" | "--version" => {
            println!("gulf {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        "run" => match args.get(1) {
            Some(path) => run_file(path),
            None => {
                eprintln!("gulf run: missing file argument");
                ExitCode::from(2)
            }
        },
        "check" => match args.get(1) {
            Some(path) => check_file(path),
            None => {
                eprintln!("gulf check: missing file argument");
                ExitCode::from(2)
            }
        },
        "tokens" => match args.get(1) {
            Some(path) => dump_tokens(path),
            None => {
                eprintln!("gulf tokens: missing file argument");
                ExitCode::from(2)
            }
        },
        "parse" => match args.get(1) {
            Some(path) => dump_ast(path),
            None => {
                eprintln!("gulf parse: missing file argument");
                ExitCode::from(2)
            }
        },
        // First arg looks like a file: run it.
        path => run_file(path),
    }
}

fn print_usage() {
    eprintln!(
        "{}",
        "\
gulf — interpreter for the Gulf of Mexico programming language

Usage:
   gulf <file.gom>           run a program
   gulf run <file.gom>       run a program (same as above)
   gulf check <file.gom>     parse-only; report diagnostics
   gulf tokens <file.gom>    dump the token stream
   gulf parse <file.gom>     dump the AST

Options:
   -h, --help     show this help
   -V, --version  show interpreter version
"
    );
}

fn read(path: &str) -> Result<SourceFile, ExitCode> {
    let text = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("gulf: cannot read `{path}`: {e}");
            return Err(ExitCode::from(2));
        }
    };
    let name = PathBuf::from(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<input>".to_string());
    Ok(SourceFile::new(name, text))
}

fn run_file(path: &str) -> ExitCode {
    let file = match read(path) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let tokens = match lexer::lex(&file) {
        Ok(t) => t,
        Err(d) => {
            eprintln!("{}", d.render(&file));
            return ExitCode::from(1);
        }
    };
    let program = match parser::parse(&file, tokens) {
        Ok(p) => p,
        Err(d) => {
            eprintln!("{}", d.render(&file));
            return ExitCode::from(1);
        }
    };
    let mut interp = Interpreter::new();
    match interp.run(&file, &program) {
        Ok(out) => {
            print!("{}", out.output);
            ExitCode::SUCCESS
        }
        Err(d) => {
            eprintln!("{}", d.render(&file));
            ExitCode::from(1)
        }
    }
}

fn check_file(path: &str) -> ExitCode {
    let file = match read(path) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let tokens = match lexer::lex(&file) {
        Ok(t) => t,
        Err(d) => {
            eprintln!("{}", d.render(&file));
            return ExitCode::from(1);
        }
    };
    let (_, diags) = parser::parse_recovering(&file, tokens);
    if !diags.is_empty() {
        for d in &diags {
            eprintln!("{}", d.render(&file));
        }
        eprintln!(
            "{} parse error{}",
            diags.len(),
            if diags.len() == 1 { "" } else { "s" }
        );
        return ExitCode::from(1);
    }
    println!("ok: {} parses cleanly", file.name);
    ExitCode::SUCCESS
}

fn dump_tokens(path: &str) -> ExitCode {
    let file = match read(path) {
        Ok(f) => f,
        Err(code) => return code,
    };
    match lexer::lex(&file) {
        Ok(tokens) => {
            for tok in tokens {
                let (line, col) = file.line_col(tok.span.start);
                println!(
                    "{:>3}:{:<3}  {:?}  ws=[{},{}]",
                    line,
                    col,
                    tok.kind,
                    tok.leading_space as u8,
                    tok.trailing_space as u8,
                );
            }
            ExitCode::SUCCESS
        }
        Err(d) => {
            eprintln!("{}", d.render(&file));
            ExitCode::from(1)
        }
    }
}

fn dump_ast(path: &str) -> ExitCode {
    let file = match read(path) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let tokens = match lexer::lex(&file) {
        Ok(t) => t,
        Err(d) => {
            eprintln!("{}", d.render(&file));
            return ExitCode::from(1);
        }
    };
    match parser::parse(&file, tokens) {
        Ok(p) => {
            println!("{p:#?}");
            ExitCode::SUCCESS
        }
        Err(d) => {
            eprintln!("{}", d.render(&file));
            ExitCode::from(1)
        }
    }
}
