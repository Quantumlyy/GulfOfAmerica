//! `gulf-lsp` — language server for Gulf of Mexico. Speaks JSON-RPC over
//! stdio. Build with `cargo build --features lsp --bin gulf-lsp`.

#[tokio::main]
async fn main() {
    gulf::lsp::run().await;
}
