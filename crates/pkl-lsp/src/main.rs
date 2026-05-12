//! Entry point for the Pkl language server. Speaks LSP over stdio.

use pkl_lsp::Backend;
use tower_lsp::{LspService, Server};

#[tokio::main]
async fn main() {
    init_tracing();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

fn init_tracing() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    // Logs go to stderr so they don't corrupt the JSON-RPC stream on stdout.
    let filter = EnvFilter::try_from_env("PKL_LSP_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr))
        .init();
}
