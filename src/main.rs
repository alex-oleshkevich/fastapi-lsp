mod check;
mod cli;
mod config;
mod features;
mod fixes;
mod linking;
mod offset;
mod parsing;
mod routes;
mod server;
mod state;
mod uri;
mod util;
mod watcher;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use cli::{CheckArgs, LspArgs, RoutesArgs};

const BUILD_TIMESTAMP: &str = env!("BUILD_TIMESTAMP");

#[derive(Parser)]
#[command(name = "fastapi-lsp", version = BUILD_TIMESTAMP)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Run in stdio mode — alias for `lsp` with stdio transport (for editors that omit the subcommand)
    #[arg(long, hide = true)]
    stdio: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Start the language server
    Lsp(LspArgs),
    /// Run diagnostics once and exit (CI / pre-commit)
    Check(CheckArgs),
    /// Print the resolved route table and exit
    Routes(RoutesArgs),
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    tracing::info!(
        "{} v{} starting",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    );

    let cli = Cli::parse();

    // Bare `--stdio` without a subcommand is an editor-compat alias for the lsp subcommand.
    let command = cli.command.unwrap_or(Command::Lsp(LspArgs {
        tcp: false,
        address: "127.0.0.1".parse().unwrap(),
        port: 9257,
    }));

    match command {
        Command::Lsp(args) => {
            let tcp = args.tcp.then_some((args.address, args.port));
            server::run(tcp).await;
        }
        Command::Check(args) => {
            if !args.only.is_empty() && !args.ignore.is_empty() {
                eprintln!("error: --only and --ignore are mutually exclusive");
                std::process::exit(2);
            }
            let exit_code = check::run(args).await;
            std::process::exit(exit_code);
        }
        Command::Routes(args) => {
            let exit_code = routes::run(args).await;
            std::process::exit(exit_code);
        }
    }
}
