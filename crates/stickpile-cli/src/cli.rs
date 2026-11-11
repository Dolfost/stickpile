//! Command-line definition (clap derive).

use std::path::PathBuf;

use clap::{ArgAction, ArgGroup, Args, Parser, Subcommand, ValueEnum};
use stickpile_api::{GitMode, JobStatus, Role, TargetApp, Visibility};

#[derive(Parser, Debug)]
#[command(
    name = "stickpile",
    version,
    about = "stickpile: your self-hosted sticker and meme library",
    propagate_version = true
)]
pub struct Cli {
    /// Config file [default: $XDG_CONFIG_HOME/stickpile/config.toml]
    #[arg(long, global = true, value_name = "PATH", env = "STICKPILE_CONFIG")]
    pub config: Option<PathBuf>,
    /// Server URL, overriding `server_url` from the config
    #[arg(long, global = true, value_name = "URL", env = "STICKPILE_SERVER")]
    pub server: Option<String>,
    /// Print machine-readable JSON instead of tables
    #[arg(long, global = true)]
    pub json: bool,
    /// More log output on stderr (-v info, -vv debug)
    #[arg(short, long, global = true, action = ArgAction::Count)]
    pub verbose: u8,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Store an API token: from the tailnet identity (no password, behind
    /// `tailscale serve`), else from username and password (+TOTP)
    Login(LoginArgs),
    /// Revoke this client's token on the server and delete the token file
    Logout,
    /// Show the logged-in user
    Whoami,
    /// Inspect or validate the client configuration
    #[command(subcommand)]
    Config(ConfigCmd),
    /// List, create, show and delete sets
    #[command(subcommand)]
    Sets(SetsCmd),
    /// Upload media files into a set (prints near-duplicate warnings)
    Upload {
        /// Set id or own slug
        set: String,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Import a set directory (set.toml + media files)
    Import {
        dir: PathBuf,
        /// Re-import into an existing own set (id or slug) instead of creating one
        #[arg(long, value_name = "SET")]
        into: Option<String>,
    },
    /// Export a set into a directory (set.toml + numbered files)
    Export {
        /// Set id or own slug
        set: String,
        /// The set directory to write (created; media files not in the set are removed)
        dir: PathBuf,
    },
    /// Search your library (falls back to the offline cache)
    Search(SearchArgs),
    /// Copy the best match (or an item by id) to the clipboard and record the use
    Copy(CopyArgs),
    /// Update the offline cache and upload queued usage events
    Sync,
    /// Upload queued usage events now
    Flush,
    /// Like an item (one like per user)
    Like { item: String },
    /// Remove your like from an item
    Unlike { item: String },
    /// Most copied, most liked and trending items
    Popular(PopularArgs),
    /// Manage API tokens
    #[command(subcommand)]
    Tokens(TokensCmd),
    /// Subscribe to git repositories as sets
    #[command(subcommand)]
    Gitpack(GitpackCmd),
    /// Re-pull a git pack now
    Pull {
        set: String,
        /// Wait for the pull job to finish
        #[arg(long)]
        wait: bool,
    },
    /// Import and publish Telegram sticker packs
    #[command(subcommand)]
    Telegram(TelegramCmd),
    /// Show (or wait for) a background job
    Job {
        id: String,
        #[arg(long)]
        wait: bool,
    },
    /// Administration (admin role required)
    #[command(subcommand)]
    Admin(AdminCmd),
    /// Export every own set into <DIR>/<slug>/
    Backup { dir: PathBuf },
    /// Internal: serve clipboard contents read from stdin until replaced
    #[command(name = "__serve-clipboard", hide = true)]
    ServeClipboard,
}

#[derive(Args, Debug)]
pub struct LoginArgs {
    #[arg(long, short)]
    pub username: Option<String>,
    /// Read the password from this file (first line) instead of prompting
    #[arg(long, value_name = "FILE")]
    pub password_file: Option<PathBuf>,
    /// Name shown in the token list [default: stickpile-cli@<hostname>]
    #[arg(long, value_name = "NAME")]
    pub token_name: Option<String>,
    /// TOTP code (prompted for when the account requires one)
    #[arg(long, value_name = "CODE")]
    pub totp: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Load and validate a config file with the real loader (no server needed)
    Check {
        /// File to check [default: --config or the default path]
        #[arg(long, value_name = "FILE")]
        file: Option<PathBuf>,
    },
    /// Print the config file path in use
    Path,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Scope {
    Mine,
    Public,
    All,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Mine => "mine",
            Scope::Public => "public",
            Scope::All => "all",
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum SetsCmd {
    /// List sets
    List {
        #[arg(long, value_enum, default_value = "all")]
        scope: Scope,
    },
    /// Create an empty set
    Create {
        /// `[a-z0-9-]`, unique among your sets
        slug: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long, value_parser = parse_visibility, default_value = "private")]
        visibility: Visibility,
        /// Recommended aspect ratio for new items, e.g. 1:1
        #[arg(long, value_parser = parse_aspect, value_name = "W:H")]
        aspect: Option<(u32, u32)>,
        /// Recommended longest side in pixels (512 for Telegram stickers)
        #[arg(long, value_name = "N")]
        px: Option<u32>,
    },
    /// Show a set and its items
    Show { set: String },
    /// Delete a set and all its items
    Rm {
        set: String,
        /// Do not ask for confirmation
        #[arg(long, short)]
        yes: bool,
    },
}

#[derive(Args, Debug)]
pub struct SearchArgs {
    /// Query words and filters (`e:😂 set:cats vibe:tired q>70 type:gif -tag:nsfw`)
    #[arg(required = true, num_args = 1..)]
    pub query: Vec<String>,
    /// Mark results that cannot be sent to this app
    #[arg(long, short, value_parser = parse_target)]
    pub target: Option<TargetApp>,
    #[arg(long, short = 'n', default_value_t = 20)]
    pub limit: usize,
}

#[derive(Args, Debug)]
#[command(group(ArgGroup::new("what").required(true).args(["query", "id"])))]
pub struct CopyArgs {
    /// Query; the best hit is copied
    #[arg(num_args = 1..)]
    pub query: Vec<String>,
    /// Copy this item instead of searching
    #[arg(long)]
    pub id: Option<String>,
    /// Target app [default: `default_target` from the config]
    #[arg(long, short, value_parser = parse_target)]
    pub target: Option<TargetApp>,
    /// Copy the sticker rendition (Telegram); also on with `prefer_stickers`
    #[arg(long)]
    pub sticker: bool,
    /// Print the path of the materialised file instead of using the clipboard
    #[arg(long)]
    pub print_path: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum PopularScope {
    Server,
    Mine,
    Set,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Window {
    Day,
    Week,
    Month,
    All,
}

#[derive(Args, Debug)]
pub struct PopularArgs {
    #[arg(long, value_enum, default_value = "server")]
    pub scope: PopularScope,
    /// Set (id or own slug) for `--scope set`
    #[arg(long, required_if_eq("scope", "set"))]
    pub set: Option<String>,
    #[arg(long, value_enum, default_value = "week")]
    pub window: Window,
    #[arg(long, short = 'n', default_value_t = 10)]
    pub limit: u32,
}

#[derive(Subcommand, Debug)]
pub enum TokensCmd {
    /// List your API tokens
    List,
    /// Create a token (the secret is shown once)
    Create {
        name: String,
        /// Expire after this many days
        #[arg(long, value_name = "DAYS")]
        expires_days: Option<i64>,
    },
    /// Revoke a token by id
    Revoke { id: String },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum GitModeArg {
    ReadOnly,
    Push,
}

impl From<GitModeArg> for GitMode {
    fn from(m: GitModeArg) -> GitMode {
        match m {
            GitModeArg::ReadOnly => GitMode::ReadOnly,
            GitModeArg::Push => GitMode::Push,
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum GitpackCmd {
    /// Bind a git repository as a new set (the server clones it in the background)
    Add {
        url: String,
        #[arg(long)]
        branch: Option<String>,
        /// Directory inside the repository holding set.toml
        #[arg(long)]
        subdir: Option<String>,
        #[arg(long, value_enum, default_value = "read-only")]
        mode: GitModeArg,
        #[arg(long)]
        slug: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum TelegramCmd {
    /// Import a Telegram sticker pack by name into a new set
    Import {
        /// Pack short name (t.me/addstickers/<name>)
        pack: String,
        #[arg(long)]
        slug: Option<String>,
        #[arg(long, value_parser = parse_visibility, default_value = "private")]
        visibility: Visibility,
        #[arg(long)]
        wait: bool,
    },
    /// Publish a set as a Telegram sticker pack (create or update)
    Export {
        set: String,
        /// Pack short name; _by_<bot> is appended if missing [default: the bound pack, else <slug>_by_<bot>]
        #[arg(long)]
        pack: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        wait: bool,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum RoleArg {
    User,
    Admin,
}

impl From<RoleArg> for Role {
    fn from(r: RoleArg) -> Role {
        match r {
            RoleArg::User => Role::User,
            RoleArg::Admin => Role::Admin,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum JobStatusArg {
    Queued,
    Running,
    Done,
    Failed,
}

impl From<JobStatusArg> for JobStatus {
    fn from(s: JobStatusArg) -> JobStatus {
        match s {
            JobStatusArg::Queued => JobStatus::Queued,
            JobStatusArg::Running => JobStatus::Running,
            JobStatusArg::Done => JobStatus::Done,
            JobStatusArg::Failed => JobStatus::Failed,
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum AdminCmd {
    /// List users
    Users,
    /// Create an invitation link
    Invite {
        #[arg(long, value_enum, default_value = "user")]
        role: RoleArg,
        #[arg(long, value_name = "DAYS")]
        expires_days: Option<i64>,
    },
    /// Show the audit log
    Audit {
        #[arg(long, short = 'n', default_value_t = 50)]
        limit: u32,
        /// Entries older than this audit id
        #[arg(long)]
        before: Option<i64>,
    },
    /// Storage usage
    Storage,
    /// Background job queue
    Jobs {
        #[arg(long, value_enum)]
        status: Option<JobStatusArg>,
    },
}

/// Marks a query word that started with `-` (a negation such as `-tag:nsfw`)
/// so clap does not take it for an option; see [`protect_negations`].
const NEGATION_MARK: char = '\u{1}';

/// Options of `search`/`copy` (and global ones) that take a value.
const VALUE_OPTIONS: &[&str] = &["--target", "-t", "--limit", "-n", "--id", "--config", "--server"];

/// Rewrite argv so that query negations after `search`/`copy` survive clap:
/// a word like `-tag:nsfw` or `-naahh` (a single dash and at least two more
/// characters, not `-vv…`) is a query word there, never an option. Words
/// after `--` are left alone.
pub fn protect_negations<I: IntoIterator<Item = std::ffi::OsString>>(args: I) -> Vec<std::ffi::OsString> {
    let mut out: Vec<std::ffi::OsString> = args.into_iter().collect();
    // Find the subcommand: the first word that is neither an option nor an option's value.
    let mut i = 1;
    while i < out.len() {
        let a = out[i].to_string_lossy();
        if a == "--config" || a == "--server" {
            i += 2;
            continue;
        }
        if a.starts_with('-') {
            i += 1;
            continue;
        }
        break;
    }
    let Some(sub) = out.get(i).map(|s| s.to_string_lossy().into_owned()) else {
        return out;
    };
    if sub != "search" && sub != "copy" {
        return out;
    }
    let mut j = i + 1;
    while j < out.len() {
        let a = out[j].to_string_lossy().into_owned();
        if a == "--" {
            break;
        }
        if VALUE_OPTIONS.contains(&a.as_str()) {
            j += 2;
            continue;
        }
        let single_dash = a.starts_with('-') && !a.starts_with("--");
        if single_dash && a.chars().count() > 2 && !a[1..].chars().all(|c| c == 'v') {
            out[j] = format!("{NEGATION_MARK}{a}").into();
        }
        j += 1;
    }
    out
}

/// Join query words, restoring words protected by [`protect_negations`].
pub fn query_string(words: &[String]) -> String {
    words
        .iter()
        .map(|w| w.strip_prefix(NEGATION_MARK).unwrap_or(w))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn parse_target(s: &str) -> Result<TargetApp, String> {
    TargetApp::parse(s).ok_or_else(|| {
        let names: Vec<&str> = TargetApp::ALL.iter().map(|t| t.as_str()).collect();
        format!("unknown target {s:?} (one of: {})", names.join(", "))
    })
}

pub fn parse_visibility(s: &str) -> Result<Visibility, String> {
    match s.to_ascii_lowercase().as_str() {
        "private" => Ok(Visibility::Private),
        "unlisted" => Ok(Visibility::Unlisted),
        "public" => Ok(Visibility::Public),
        _ => Err(format!("unknown visibility {s:?} (private, unlisted, public)")),
    }
}

pub fn parse_aspect(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s.split_once(':').ok_or_else(|| format!("expected W:H, got {s:?}"))?;
    let w: u32 = w.trim().parse().map_err(|_| format!("bad width in {s:?}"))?;
    let h: u32 = h.trim().parse().map_err(|_| format!("bad height in {s:?}"))?;
    if w == 0 || h == 0 {
        return Err("aspect ratio parts must be > 0".into());
    }
    Ok((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(protect_negations(
            std::iter::once("stickpile").chain(args.iter().copied()).map(Into::into),
        ))
    }

    #[test]
    fn definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn global_flags_anywhere() {
        let cli = parse(&[
            "search",
            "cat",
            "--json",
            "--server",
            "http://x:1",
            "--config",
            "/tmp/c.toml",
        ])
        .unwrap();
        assert!(cli.json);
        assert_eq!(cli.server.as_deref(), Some("http://x:1"));
        assert_eq!(cli.config.as_deref(), Some(std::path::Path::new("/tmp/c.toml")));
    }

    #[test]
    fn search_query_words_and_negations() {
        let cli = parse(&["search", "big", "cat", "e:😂", "--target", "Discord", "-n", "5"]).unwrap();
        let Command::Search(a) = cli.command else { panic!() };
        assert_eq!(a.query, ["big", "cat", "e:😂"]);
        assert_eq!(a.target, Some(TargetApp::Discord));
        assert_eq!(a.limit, 5);

        let cli = parse(&["search", "cat", "-tag:nsfw", "-t", "telegram", "-naahh", "-vv"]).unwrap();
        assert_eq!(cli.verbose, 2);
        let Command::Search(a) = cli.command else { panic!() };
        assert_eq!(query_string(&a.query), "cat -tag:nsfw -naahh");
        assert_eq!(a.target, Some(TargetApp::Telegram));

        let cli = parse(&["--config", "x.toml", "search", "-e:😂", "--limit", "3"]).unwrap();
        let Command::Search(a) = cli.command else { panic!() };
        assert_eq!(query_string(&a.query), "-e:😂");
        assert_eq!(a.limit, 3);

        let cli = parse(&["search", "--", "-tag:nsfw", "cat"]).unwrap();
        let Command::Search(a) = cli.command else { panic!() };
        assert_eq!(query_string(&a.query), "-tag:nsfw cat");

        let cli = parse(&["copy", "-tag:nsfw", "dog", "--sticker"]).unwrap();
        let Command::Copy(a) = cli.command else { panic!() };
        assert_eq!(query_string(&a.query), "-tag:nsfw dog");
        assert!(a.sticker);

        // Other subcommands are untouched.
        assert!(parse(&["sets", "rm", "-yes"]).is_err());

        assert!(parse(&["search"]).is_err());
        assert!(parse(&["search", "x", "--target", "myspace"])
            .unwrap_err()
            .to_string()
            .contains("unknown target"));
    }

    #[test]
    fn copy_needs_query_or_id() {
        assert!(parse(&["copy"]).is_err());
        let cli = parse(&["copy", "--id", "abc", "--sticker", "--print-path", "-t", "telegram"]).unwrap();
        let Command::Copy(a) = cli.command else { panic!() };
        assert_eq!(a.id.as_deref(), Some("abc"));
        assert!(a.sticker && a.print_path);
        assert_eq!(a.target, Some(TargetApp::Telegram));
        assert!(
            parse(&["copy", "--id", "abc", "cat"]).is_err(),
            "query and --id are exclusive"
        );
        let cli = parse(&["copy", "naahh", "bro"]).unwrap();
        let Command::Copy(a) = cli.command else { panic!() };
        assert_eq!(a.query, ["naahh", "bro"]);
    }

    #[test]
    fn sets_create_options() {
        let cli = parse(&[
            "sets",
            "create",
            "cats",
            "--name",
            "My cats",
            "--visibility",
            "public",
            "--aspect",
            "1:1",
            "--px",
            "512",
        ])
        .unwrap();
        let Command::Sets(SetsCmd::Create {
            slug,
            name,
            visibility,
            aspect,
            px,
            ..
        }) = cli.command
        else {
            panic!()
        };
        assert_eq!(slug, "cats");
        assert_eq!(name.as_deref(), Some("My cats"));
        assert_eq!(visibility, Visibility::Public);
        assert_eq!(aspect, Some((1, 1)));
        assert_eq!(px, Some(512));
        assert!(parse(&["sets", "create", "x", "--aspect", "16x9"]).is_err());
        assert!(parse(&["sets", "create", "x", "--aspect", "0:9"]).is_err());
        assert!(parse(&["sets", "create", "x", "--visibility", "secret"]).is_err());
        let cli = parse(&["sets", "list", "--scope", "mine"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Sets(SetsCmd::List { scope: Scope::Mine })
        ));
    }

    #[test]
    fn subcommands_parse() {
        for args in [
            &[
                "login",
                "--username",
                "alice",
                "--password-file",
                "/run/pw",
                "--token-name",
                "laptop",
            ][..],
            &["logout"],
            &["whoami"],
            &["config", "check", "--file", "x.toml"],
            &["config", "path"],
            &["sets", "show", "cats"],
            &["sets", "rm", "cats", "-y"],
            &["upload", "cats", "a.png", "b.gif"],
            &["import", "./my-cats", "--into", "cats"],
            &["export", "cats", "./out"],
            &["sync"],
            &["flush"],
            &["like", "abc"],
            &["unlike", "abc"],
            &["popular", "--scope", "set", "--set", "cats", "--window", "month"],
            &["tokens", "list"],
            &["tokens", "create", "phone", "--expires-days", "30"],
            &["tokens", "revoke", "abc"],
            &[
                "gitpack",
                "add",
                "https://x/y.git",
                "--branch",
                "main",
                "--subdir",
                "pack",
                "--mode",
                "push",
                "--slug",
                "y",
            ],
            &["pull", "y", "--wait"],
            &["telegram", "import", "Animals", "--slug", "animals"],
            &["telegram", "export", "cats", "--pack", "cats_by_bot", "--title", "Cats"],
            &["job", "abc", "--wait"],
            &["admin", "users"],
            &["admin", "invite", "--role", "admin", "--expires-days", "3"],
            &["admin", "audit", "-n", "5"],
            &["admin", "storage"],
            &["admin", "jobs", "--status", "failed"],
            &["backup", "/tmp/backup"],
            &["__serve-clipboard"],
        ] {
            parse(args).unwrap_or_else(|e| panic!("{args:?}: {e}"));
        }
        assert!(
            parse(&["popular", "--scope", "set"]).is_err(),
            "--set required for --scope set"
        );
        assert!(parse(&["gitpack", "add", "u", "--mode", "rw"]).is_err());
        assert!(parse(&["upload", "cats"]).is_err());
    }

    #[test]
    fn hidden_subcommand_not_in_help() {
        let help = Cli::command().render_long_help().to_string();
        assert!(!help.contains("__serve-clipboard"));
        assert!(help.contains("copy"));
    }
}
