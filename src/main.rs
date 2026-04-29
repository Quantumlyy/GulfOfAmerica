use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = match args.first() {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("usage: gulf <file.gom>");
            return ExitCode::from(2);
        }
    };
    let source = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not read {}: {e}", path.display());
            return ExitCode::from(2);
        }
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<input>".to_string());
    match gulf::run(&source, &name) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(diag) => {
            eprintln!("{diag}");
            ExitCode::from(1)
        }
    }
}
