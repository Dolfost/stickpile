use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "stickpile-server", version, about = "Self-hosted sticker and meme library")]
struct Cli {
    /// Path to the TOML configuration file.
    #[arg(short, long, env = "STICKPILE_CONFIG", default_value = "/etc/stickpile/config.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server (default).
    Serve,
    /// Parse and validate the configuration file, then exit. Used by the Nix
    /// module check so the module and the loader cannot drift.
    CheckConfig,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve) {
        Command::CheckConfig => {
            let cfg = stickpile_server::Config::load(&cli.config)?;
            println!("configuration OK: {} (data_dir {})", cli.config.display(), cfg.server.data_dir.display());
            Ok(())
        }
        Command::Serve => anyhow::bail!("not implemented yet"),
    }
}
