use std::path::PathBuf;

use clap::{Parser, Subcommand};
use stickpile_api::Role;

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
    /// Create a user in the database and exit.
    CreateUser {
        username: String,
        /// Give the user the admin role.
        #[arg(long)]
        admin: bool,
        /// File holding the password (one line). Secrets are never passed inline.
        #[arg(long)]
        password_file: Option<PathBuf>,
        /// Bind the account to this tailnet login (e.g. `alice@github`).
        #[arg(long)]
        tailscale_login: Option<String>,
    },
}

fn init_logging(filter: &str) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .or_else(|_| tracing_subscriber::EnvFilter::try_new(filter))
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve) {
        Command::CheckConfig => {
            let cfg = stickpile_server::Config::load(&cli.config)?;
            println!(
                "configuration OK: {} (data_dir {})",
                cli.config.display(),
                cfg.server.data_dir.display()
            );
            Ok(())
        }
        Command::Serve => {
            let cfg = stickpile_server::Config::load(&cli.config)?;
            init_logging(&cfg.server.log);
            tokio::runtime::Runtime::new()?.block_on(stickpile_server::run(cfg))
        }
        Command::CreateUser {
            username,
            admin,
            password_file,
            tailscale_login,
        } => {
            let cfg = stickpile_server::Config::load(&cli.config)?;
            init_logging("warn");
            if password_file.is_none() && tailscale_login.is_none() {
                anyhow::bail!("give --password-file and/or --tailscale-login, or the user cannot log in");
            }
            let password = password_file
                .map(|p| {
                    stickpile_server::config::read_secret(&p)
                        .map_err(|e| anyhow::anyhow!("reading {}: {e}", p.display()))
                })
                .transpose()?;
            tokio::runtime::Runtime::new()?.block_on(async move {
                std::fs::create_dir_all(&cfg.server.data_dir)?;
                let db = stickpile_server::db::connect(&cfg.db_path()).await?;
                let user = stickpile_server::service::users::create(
                    &db,
                    stickpile_server::service::users::NewUser {
                        username,
                        display_name: None,
                        password,
                        role: if admin { Role::Admin } else { Role::User },
                        tailscale_login,
                    },
                )
                .await
                .map_err(|e| anyhow::anyhow!("{}", e.message))?;
                stickpile_server::db::audit(
                    &db,
                    None,
                    "user.create",
                    Some(&user.id),
                    None,
                    Some(serde_json::json!({"username": user.username, "role": user.role, "via": "cli"})),
                )
                .await;
                println!("created user {} ({}, id {})", user.username, user.role, user.id);
                Ok(())
            })
        }
    }
}
