//! `stickpile`: command-line client for a stickpile server (see docs/cli.md).

mod cli;
mod commands;
mod output;

use std::process::ExitCode;

use clap::Parser;
use stickpile_core::ClientConfig;

use crate::cli::{Cli, Command, ConfigCmd};
use crate::commands::Ctx;

fn init_tracing(verbose: u8) {
    use tracing_subscriber::EnvFilter;
    let default = match verbose {
        0 => "warn",
        1 => "info,stickpile_core=info",
        _ => "debug,hyper=info,reqwest=info,rustls=info",
    };
    let filter = std::env::var("STICKPILE_LOG")
        .ok()
        .and_then(|s| EnvFilter::try_new(s).ok())
        .unwrap_or_else(|| EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
}

fn main() -> ExitCode {
    let cli = Cli::parse_from(cli::protect_negations(std::env::args_os()));

    // The detached clipboard helper: no config, no runtime, no logging.
    if matches!(cli.command, Command::ServeClipboard) {
        return match stickpile_core::clipboard::serve::serve_stdin() {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::FAILURE,
        };
    }

    init_tracing(cli.verbose);
    let config_path = cli.config.clone().unwrap_or_else(ClientConfig::default_path);

    // `config check` validates exactly the given file and must work without a
    // server, token or cache (it runs in the home-manager module's build check).
    if let Command::Config(cmd) = &cli.command {
        let result = match cmd {
            ConfigCmd::Check { file } => commands::config_check(file.as_deref().unwrap_or(&config_path), cli.json),
            ConfigCmd::Path => {
                println!("{}", config_path.display());
                Ok(())
            }
        };
        return report(result);
    }

    let ctx = match Ctx::load(config_path, cli.server.clone(), cli.json) {
        Ok(c) => c,
        Err(e) => return report(Err(e)),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return report(Err(e.into())),
    };
    let result = runtime.block_on(commands::run(&ctx, cli.command));
    // Do not wait for background tasks (e.g. a usage flush) on exit.
    runtime.shutdown_timeout(std::time::Duration::from_millis(200));
    report(result)
}

fn report(result: anyhow::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            if let Some(hint) = commands::explain(&e) {
                eprintln!("hint: {hint}");
            }
            ExitCode::FAILURE
        }
    }
}
