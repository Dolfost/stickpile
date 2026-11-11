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
    /// Write a full backup into DIR: every set of every user as a set
    /// directory (`sets/<user>/<slug>/`), a database snapshot
    /// (`stickpile.db`) and `backup.json`. Safe while the server runs.
    Backup {
        /// Target directory: missing, empty, or an earlier backup.
        dir: PathBuf,
    },
    /// Restore a backup written by `backup` into the configured, fresh
    /// `server.data_dir` (database, originals, thumbnails; other variants are
    /// queued and made once the server runs).
    Restore {
        /// A directory written by `stickpile-server backup`.
        dir: PathBuf,
    },
    /// Exit 0 if the server answers `GET /healthz` with 2xx within 3 s, else
    /// 1. For container health checks (no shell or curl needed).
    Healthcheck {
        /// Default: `http://<server.listen>/healthz` from the configuration
        /// (a wildcard listen address is checked on loopback).
        #[arg(long)]
        url: Option<String>,
    },
}

/// Problems `check-config` reports without failing: things that only matter
/// at runtime and may legitimately be provisioned later (model downloads,
/// secrets mounted at start).
fn config_warnings(cfg: &stickpile_server::Config) -> Vec<String> {
    let mut out = Vec::new();
    if cfg.ai.enabled {
        let kind = match cfg.ai.text_encoder {
            stickpile_server::config::TextEncoder::Clip => stickpile_ai::TextEncoderKind::Clip,
            stickpile_server::config::TextEncoder::Multilingual => stickpile_ai::TextEncoderKind::Multilingual,
        };
        let missing = stickpile_ai::missing_model_files(&cfg.ai.model_dir, kind);
        if !missing.is_empty() {
            let list: Vec<String> = missing.iter().map(|p| format!("  {}", p.display())).collect();
            out.push(format!(
                "ai.enabled = true but model files are missing (AI stays off until they exist; \
                 see docs/ai.md, `scripts/fetch-models.sh`):\n{}",
                list.join("\n")
            ));
        }
    }
    let unreadable = |what: &str, p: &std::path::Path| -> Option<String> {
        match stickpile_server::config::read_secret(p) {
            Ok(s) if !s.trim().is_empty() => None,
            Ok(_) => Some(format!("{what} {} is empty", p.display())),
            Err(e) => Some(format!("{what} {} is not readable: {e}", p.display())),
        }
    };
    if cfg.telegram.enabled {
        if let Some(w) = cfg
            .telegram
            .bot_token_file
            .as_deref()
            .and_then(|p| unreadable("telegram.bot_token_file", p))
        {
            out.push(format!("{w} (the Telegram integration stays off)"));
        }
    }
    if cfg.git.enabled {
        if let Some(w) = cfg
            .git
            .https_token_file
            .as_deref()
            .and_then(|p| unreadable("git.https_token_file", p))
        {
            out.push(w);
        }
        if let Some(p) = cfg.git.ssh_key_file.as_deref() {
            if !p.is_file() {
                out.push(format!("git.ssh_key_file {} does not exist", p.display()));
            }
        }
    }
    out
}

/// `GET <url>` over plain HTTP/1.1 with a 3 s budget; Ok if 2xx.
fn healthcheck(url: &str) -> anyhow::Result<u16> {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + Duration::from_secs(3);
    let left = || {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| anyhow::anyhow!("timed out"))
    };
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| anyhow::anyhow!("only http:// URLs are supported, got {url:?}"))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if authority.is_empty() || authority.contains('@') {
        anyhow::bail!("bad URL {url:?}");
    }
    let target = if authority
        .rsplit_once(':')
        .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
        && !authority.ends_with(']')
    {
        authority.to_string()
    } else {
        format!("{authority}:80")
    };
    let addr = target
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow::anyhow!("cannot resolve {authority}"))?;
    let mut stream = TcpStream::connect_timeout(&addr, left()?)?;
    stream.set_write_timeout(Some(left()?))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {authority}\r\nUser-Agent: stickpile-healthcheck\r\nConnection: close\r\n\r\n"
    )?;
    let mut head = Vec::new();
    let mut buf = [0u8; 512];
    while !head.windows(2).any(|w| w == b"\r\n") && head.len() < 4096 {
        stream.set_read_timeout(Some(left()?))?;
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        head.extend_from_slice(&buf[..n]);
    }
    let line = String::from_utf8_lossy(&head);
    let status: u16 = line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .filter(|_| line.starts_with("HTTP/"))
        .ok_or_else(|| anyhow::anyhow!("not an HTTP response"))?;
    if (200..300).contains(&status) {
        Ok(status)
    } else {
        anyhow::bail!("HTTP {status}")
    }
}

/// `http://<listen>/healthz`, with wildcard addresses mapped to loopback.
fn default_health_url(listen: std::net::SocketAddr) -> String {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    let ip = match listen.ip() {
        IpAddr::V4(v) if v.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(v) if v.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    format!(
        "http://{}{}",
        std::net::SocketAddr::new(ip, listen.port()),
        stickpile_api::routes::HEALTH
    )
}

fn init_logging(filter: &str) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .or_else(|_| tracing_subscriber::EnvFilter::try_new(filter))
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    // No colour escapes in the journal / container log.
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .init();
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve) {
        Command::CheckConfig => {
            let cfg = stickpile_server::Config::load(&cli.config)?;
            for w in config_warnings(&cfg) {
                eprintln!("warning: {w}");
            }
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
            for w in config_warnings(&cfg) {
                tracing::warn!("{w}");
            }
            if let Err(e) = stickpile_gitpack::set_network_timeouts(
                std::time::Duration::from_secs(30),
                std::time::Duration::from_secs(120),
            ) {
                tracing::warn!("setting git network timeouts: {e}");
            }
            tokio::runtime::Runtime::new()?.block_on(stickpile_server::run(cfg))
        }
        Command::Backup { dir } => {
            let cfg = stickpile_server::Config::load(&cli.config)?;
            init_logging("warn");
            let info = tokio::runtime::Runtime::new()?.block_on(stickpile_server::service::backup::run(&cfg, &dir))?;
            println!(
                "Backed up {} sets ({} items) of {} users and the database to {}",
                info.sets,
                info.items,
                info.users,
                dir.display()
            );
            for e in &info.errors {
                eprintln!("error: {e}");
            }
            if !info.errors.is_empty() {
                anyhow::bail!(
                    "{} sets could not be exported (the database snapshot is complete)",
                    info.errors.len()
                );
            }
            Ok(())
        }
        Command::Restore { dir } => {
            let cfg = stickpile_server::Config::load(&cli.config)?;
            init_logging("warn");
            let info =
                tokio::runtime::Runtime::new()?.block_on(stickpile_server::service::backup::restore(&cfg, &dir))?;
            println!(
                "Restored the database and {} media files of {} sets into {}; {} variant jobs run when the server starts",
                info.media,
                info.sets,
                cfg.server.data_dir.display(),
                info.variant_jobs
            );
            for m in &info.missing {
                eprintln!("warning: no file in the backup for {m}");
            }
            Ok(())
        }
        Command::Healthcheck { url } => {
            let url = match url {
                Some(u) => u,
                None => default_health_url(stickpile_server::Config::load(&cli.config)?.server.listen),
            };
            match healthcheck(&url) {
                Ok(status) => {
                    println!("healthy: {url} answered {status}");
                    Ok(())
                }
                Err(e) => {
                    eprintln!("unhealthy: {url}: {e}");
                    std::process::exit(1);
                }
            }
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
