//! Command implementations.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _, Result};
use serde_json::json;
use stickpile_api::*;
use stickpile_core::cache::Cache;
use stickpile_core::client::{ApiClient, PopularParams, UploadFile};
use stickpile_core::clipboard::serve::spawn_detached;
use stickpile_core::{token, ClientConfig, CoreError, Session};

use crate::cli::*;
use crate::output::{self, Table};

const CLIENT_NAME: &str = concat!("cli/", env!("CARGO_PKG_VERSION"));
/// Files per upload request.
const UPLOAD_BATCH: usize = 16;
const JOB_POLL: Duration = Duration::from_secs(1);
const JOB_MAX_WAIT: Duration = Duration::from_secs(30 * 60);

pub struct Ctx {
    pub config: ClientConfig,
    pub config_path: PathBuf,
    pub json: bool,
}

impl Ctx {
    /// Load the config (missing file = defaults) and apply `--server`.
    pub fn load(config_path: PathBuf, server: Option<String>, json: bool) -> Result<Ctx> {
        let mut config = ClientConfig::load(&config_path)?;
        if let Some(url) = server {
            config.server_url = url;
            config.validate()?;
        }
        Ok(Ctx {
            config,
            config_path,
            json,
        })
    }

    fn token(&self) -> Result<Option<String>> {
        Ok(token::read_token(&self.config.token_path())?)
    }

    /// Client authenticated with the stored token (if any).
    fn client(&self) -> Result<ApiClient> {
        Ok(ApiClient::from_config(&self.config)?.with_token(self.token()?))
    }

    async fn session(&self) -> Result<Session> {
        Ok(Session::open(self.config.clone()).await?.with_client_name(CLIENT_NAME))
    }
}

/// Make core errors actionable on the command line.
pub fn explain(e: &anyhow::Error) -> Option<&'static str> {
    let core = e.downcast_ref::<CoreError>()?;
    match core {
        CoreError::Api { status: 401, .. } => Some("the server rejected the token; run `stickpile login`"),
        CoreError::Api { status: 403, .. } => Some("your account is not allowed to do this"),
        CoreError::Offline(_) => Some("check `server_url` in the config, or that the server is running"),
        _ => None,
    }
}

pub async fn run(ctx: &Ctx, command: Command) -> Result<()> {
    match command {
        Command::Login(a) => login(ctx, a).await,
        Command::Logout => logout(ctx).await,
        Command::Whoami => whoami(ctx).await,
        Command::Config(c) => config_cmd(ctx, c),
        Command::Sets(c) => sets(ctx, c).await,
        Command::Upload { set, files } => upload(ctx, &set, &files).await,
        Command::Import { dir, into } => import(ctx, &dir, into.as_deref()).await,
        Command::Export { set, dir } => export(ctx, &set, &dir).await,
        Command::Search(a) => search(ctx, a).await,
        Command::Copy(a) => copy(ctx, a).await,
        Command::Sync => sync(ctx).await,
        Command::Flush => flush(ctx).await,
        Command::Like { item } => like(ctx, &item, true).await,
        Command::Unlike { item } => like(ctx, &item, false).await,
        Command::Popular(a) => popular(ctx, a).await,
        Command::Tokens(c) => tokens(ctx, c).await,
        Command::Gitpack(c) => gitpack(ctx, c).await,
        Command::Pull { set, wait } => {
            let client = ctx.client()?;
            let id = resolve_set(&client, &set).await?;
            let job = client.pull_set(&id).await?;
            show_job(ctx, &client, job, wait).await
        }
        Command::Telegram(c) => telegram(ctx, c).await,
        Command::Job { id, wait } => {
            let client = ctx.client()?;
            let job = client.job(&id).await?;
            show_job(ctx, &client, job, wait).await
        }
        Command::Admin(c) => admin(ctx, c).await,
        Command::Backup { dir } => backup(ctx, &dir).await,
        Command::ServeClipboard => unreachable!("handled before the runtime starts"),
    }
}

// ---------------------------------------------------------------------------
// config (no server, no token)
// ---------------------------------------------------------------------------

/// `config check` / `config path`. Never touches the token, cache or server.
pub fn config_cmd(ctx_path_json: &Ctx, cmd: ConfigCmd) -> Result<()> {
    match cmd {
        ConfigCmd::Path => {
            println!("{}", ctx_path_json.config_path.display());
            Ok(())
        }
        ConfigCmd::Check { file } => config_check(
            file.as_deref().unwrap_or(&ctx_path_json.config_path),
            ctx_path_json.json,
        ),
    }
}

pub fn config_check(path: &Path, as_json: bool) -> Result<()> {
    if !path.is_file() {
        bail!("{}: no such file", path.display());
    }
    // The file itself is checked strictly; skipped settings.toml entries
    // (GUI settings under a read-only config) only warn.
    let layered = stickpile_core::settings::load_layered(path)?;
    for w in &layered.warnings {
        eprintln!("warning: {w}");
    }
    if as_json {
        output::json(&layered.config)?;
    } else {
        println!("OK {}", path.display());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// auth
// ---------------------------------------------------------------------------

fn prompt_line(prompt: &str) -> Result<String> {
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

async fn login(ctx: &Ctx, a: LoginArgs) -> Result<()> {
    let token_name = a.token_name.unwrap_or_else(|| format!("stickpile-cli@{}", hostname()));
    let client = ApiClient::from_config(&ctx.config)?;

    // On the tailnet (behind `tailscale serve`) the server already knows who we
    // are: turn that identity into a token without asking for a password.
    if a.username.is_none() && a.password_file.is_none() {
        match client.me().await {
            Ok(user) => {
                tracing::info!("authenticated by tailnet identity as {}", user.username);
                let created = client
                    .create_token(&CreateToken {
                        name: token_name,
                        expires_at: None,
                    })
                    .await?;
                return finish_login(ctx, created, "tailnet identity");
            }
            Err(CoreError::NotLoggedIn) => {}
            Err(e) => return Err(e.into()),
        }
    }

    let username = match a.username {
        Some(u) => u,
        None => prompt_line("Username: ")?,
    };
    if username.is_empty() {
        bail!("a username is required");
    }
    let password = match &a.password_file {
        Some(f) => {
            let text = std::fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?;
            text.lines().next().unwrap_or_default().to_string()
        }
        None => rpassword::prompt_password("Password: ")?,
    };
    let mut req = TokenRequest {
        username,
        password,
        totp: a.totp,
        token_name,
    };
    let created = match client.login(&req).await {
        Err(e) if e.is_totp_required() && req.totp.is_none() => {
            req.totp = Some(prompt_line("TOTP code: ")?);
            client.login(&req).await?
        }
        other => other?,
    };
    finish_login(ctx, created, "password")
}

fn finish_login(ctx: &Ctx, created: TokenCreated, method: &str) -> Result<()> {
    let path = ctx.config.token_path();
    token::write_token(&path, &created.token)?;
    // Bind the offline cache to this server and account.
    match Cache::open(&ctx.config.cache_path(), ctx.config.cache_max_mb) {
        Ok(cache) => {
            cache.bind_server(&ctx.config.server_url)?;
            cache.set_me(&created.user)?;
        }
        Err(e) => tracing::warn!("offline cache: {e}"),
    }
    if ctx.json {
        return output::json(
            &json!({ "user": created.user, "token": created.info, "token_file": path, "method": method }),
        );
    }
    println!(
        "Logged in to {} as {} ({method}).",
        ctx.config.server_url, created.user.username
    );
    println!("Token {:?} saved to {}.", created.info.name, path.display());
    Ok(())
}

async fn logout(ctx: &Ctx) -> Result<()> {
    let path = ctx.config.token_path();
    let Some(secret) = ctx.token()? else {
        println!("Not logged in.");
        return Ok(());
    };
    let session = ctx.session().await?;
    if let Err(e) = session.flush_usage().await {
        tracing::info!("usage events stay queued: {e}");
    }
    let client = ApiClient::from_config(&ctx.config)?.with_token(Some(secret));
    let revoked = match client.current_token_id().await {
        Ok(Some(id)) => client.revoke_token(&id).await.map(|_| true),
        Ok(None) => Ok(false),
        Err(e) => Err(e),
    };
    match &revoked {
        Ok(true) => {}
        Ok(false) => eprintln!("warning: this token was not found on the server; deleting it locally"),
        Err(e) if e.is_unauthorized() => {}
        Err(e) => eprintln!("warning: could not revoke the token on the server ({e}); revoke it in the web UI"),
    }
    token::delete_token(&path)?;
    session.cache().reset_account_data()?;
    if ctx.json {
        return output::json(&json!({ "revoked": matches!(revoked, Ok(true)), "token_file": path }));
    }
    println!("Logged out; removed {}.", path.display());
    Ok(())
}

async fn whoami(ctx: &Ctx) -> Result<()> {
    let user = ctx.client()?.me().await?;
    if ctx.json {
        return output::json(&user);
    }
    println!("{} ({}) on {}", user.username, user.display_name, ctx.config.server_url);
    println!(
        "role: {}  totp: {}",
        output::name(&user.role),
        if user.totp_enabled { "on" } else { "off" }
    );
    if let Some(ts) = &user.tailscale_login {
        println!("tailscale: {ts}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// sets
// ---------------------------------------------------------------------------

/// Directory name for a set slug (slugs are `[a-z0-9-]`; anything else is replaced).
fn dir_name_for_slug(slug: &str) -> String {
    let s: String = slug
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "set".into()
    } else {
        s
    }
}

fn looks_like_id(s: &str) -> bool {
    s.len() == 26 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Set id for an id or an own slug.
pub async fn resolve_set(client: &ApiClient, arg: &str) -> Result<String> {
    let sets = client.sets(Some("mine")).await?;
    if let Some(s) = sets.iter().find(|s| s.id == arg || s.slug.eq_ignore_ascii_case(arg)) {
        return Ok(s.id.clone());
    }
    if looks_like_id(arg) {
        return Ok(arg.to_string());
    }
    bail!("no set of yours with slug or id {arg:?} (see `stickpile sets list`)")
}

fn set_rows(sets: &[SetSummary]) -> Table {
    let mut t = Table::new(&[
        "SLUG",
        "NAME",
        "OWNER",
        "VISIBILITY",
        "SOURCE",
        "ITEMS",
        "UPDATED",
        "ID",
    ]);
    for s in sets {
        t.row(vec![
            s.slug.clone(),
            s.name.clone(),
            s.owner_username.clone(),
            output::name(&s.visibility),
            output::name(&s.source),
            s.item_count.to_string(),
            output::time(s.updated_at),
            s.id.clone(),
        ]);
    }
    t
}

async fn sets(ctx: &Ctx, cmd: SetsCmd) -> Result<()> {
    let client = ctx.client()?;
    match cmd {
        SetsCmd::List { scope } => {
            let sets = client.sets(Some(scope.as_str())).await?;
            if ctx.json {
                return output::json(&sets);
            }
            if sets.is_empty() {
                println!("No sets.");
            } else {
                set_rows(&sets).print();
            }
        }
        SetsCmd::Create {
            slug,
            name,
            description,
            visibility,
            aspect,
            px,
        } => {
            let req = CreateSet {
                name: name.unwrap_or_else(|| slug.clone()),
                slug,
                description: description.unwrap_or_default(),
                visibility,
                aspect_ratio: aspect,
                recommended_px: px,
            };
            let set = client.create_set(&req).await?;
            if ctx.json {
                return output::json(&set);
            }
            println!(
                "Created set {} ({}), {}.",
                set.slug,
                set.id,
                output::name(&set.visibility)
            );
        }
        SetsCmd::Show { set } => {
            let id = resolve_set(&client, &set).await?;
            let detail = client.set(&id).await?;
            if ctx.json {
                return output::json(&detail);
            }
            print_set(&detail);
        }
        SetsCmd::Rm { set, yes } => {
            let id = resolve_set(&client, &set).await?;
            let detail = client.set(&id).await?;
            if !yes {
                if !std::io::stdin().is_terminal() {
                    bail!("refusing to delete without confirmation; pass --yes");
                }
                let answer = prompt_line(&format!(
                    "Delete set {} ({} items)? This cannot be undone. [y/N] ",
                    detail.set.slug, detail.set.item_count
                ))?;
                if !matches!(answer.to_lowercase().as_str(), "y" | "yes") {
                    bail!("aborted");
                }
            }
            client.delete_set(&id).await?;
            if ctx.json {
                return output::json(&json!({ "deleted": id }));
            }
            println!("Deleted set {}.", detail.set.slug);
        }
    }
    Ok(())
}

fn print_set(d: &SetDetail) {
    let s = &d.set;
    println!("{} ({})", s.name, s.slug);
    println!("  id:          {}", s.id);
    println!("  owner:       {}", s.owner_username);
    println!("  visibility:  {}", output::name(&s.visibility));
    println!("  source:      {}", output::name(&s.source));
    if let Some(g) = &s.git {
        println!(
            "  git:         {} ({}){}",
            g.url,
            output::name(&g.mode),
            g.branch.as_ref().map(|b| format!(" branch {b}")).unwrap_or_default()
        );
        if let Some(e) = &g.last_error {
            println!("  git error:   {e}");
        }
    }
    if let Some(p) = &s.telegram_pack {
        println!("  telegram:    {p}");
    }
    if let Some((w, h)) = s.aspect_ratio {
        println!("  aspect:      {w}:{h}");
    }
    if let Some(px) = s.recommended_px {
        println!("  size:        {px} px");
    }
    println!("  version:     {}", s.version);
    if !s.description.is_empty() {
        println!("  description: {}", s.description);
    }
    println!();
    if d.items.is_empty() {
        println!("No items.");
        return;
    }
    let mut t = Table::new(&["#", "NAME", "TYPE", "SIZE", "Q", "LIKES", "COPIES", "KEYWORDS", "ID"]);
    for (i, it) in d.items.iter().enumerate() {
        t.row(vec![
            (i + 1).to_string(),
            it.name.clone(),
            it.media.format.extension().to_string(),
            output::bytes(it.media.size_bytes),
            output::opt(it.media.quality),
            it.likes.to_string(),
            it.copies.to_string(),
            it.keywords.join(", "),
            it.id.clone(),
        ]);
    }
    t.print();
}

async fn upload(ctx: &Ctx, set: &str, files: &[PathBuf]) -> Result<()> {
    let client = ctx.client()?;
    let id = resolve_set(&client, set).await?;
    let mut all = Vec::new();
    let mut failed = 0usize;
    for chunk in files.chunks(UPLOAD_BATCH) {
        let parts = chunk
            .iter()
            .map(|p| UploadFile::from_path(p))
            .collect::<Result<Vec<_>, _>>()?;
        let results = client.upload(&id, parts, None).await?;
        for r in &results {
            if r.error.is_some() {
                failed += 1;
            }
            if !ctx.json {
                print_upload(r);
            }
        }
        all.extend(results);
    }
    if ctx.json {
        output::json(&all)?;
    }
    if failed > 0 {
        bail!("{failed} of {} files failed", files.len());
    }
    Ok(())
}

fn print_upload(r: &UploadResult) {
    match (&r.item, &r.error) {
        (_, Some(e)) => println!("{}: error: {e}", r.file),
        (Some(item), None) if r.already_in_set => {
            println!("{}: already in this set as {:?} ({})", r.file, item.name, item.id)
        }
        (Some(item), None) => println!("{}: added as {:?} ({})", r.file, item.name, item.id),
        (None, None) => println!("{}: no result", r.file),
    }
    for nd in &r.near_duplicates {
        println!("  note: {}", near_duplicate_text(nd));
    }
}

fn near_duplicate_text(nd: &NearDuplicate) -> String {
    let what = if nd.same_content {
        "the same file"
    } else {
        "a near-duplicate"
    };
    format!(
        "you already have {what} in set {}: {:?} ({}, distance {})",
        nd.existing.set_slug, nd.existing.name, nd.existing.id, nd.distance
    )
}

async fn import(ctx: &Ctx, dir: &Path, into: Option<&str>) -> Result<()> {
    let client = ctx.client()?;
    let into = match into {
        Some(s) => Some(resolve_set(&client, s).await?),
        None => None,
    };
    let (report, warnings) = client.import_dir(dir, into.as_deref()).await?;
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    if ctx.json {
        return output::json(&json!({ "report": report, "warnings": warnings }));
    }
    let set = report
        .set
        .as_ref()
        .map(|s| format!("{} ({})", s.slug, s.id))
        .unwrap_or_else(|| "?".into());
    println!(
        "Imported {} into set {set}: {} created, {} updated, {} unchanged, {} removed.",
        dir.display(),
        report.created,
        report.updated,
        report.unchanged,
        report.removed
    );
    for e in &report.errors {
        println!("  error: {}: {}", e.file, e.message);
    }
    for nd in &report.near_duplicates {
        println!("  note: {}", near_duplicate_text(nd));
    }
    Ok(())
}

async fn export(ctx: &Ctx, set: &str, dir: &Path) -> Result<()> {
    let client = ctx.client()?;
    let id = resolve_set(&client, set).await?;
    let bundle = client.export_to_dir(&id, dir).await?;
    if ctx.json {
        let files: Vec<&str> = bundle.files.iter().map(|f| f.filename.as_str()).collect();
        return output::json(
            &json!({ "dir": dir, "name": bundle.manifest.name, "version": bundle.manifest.version, "files": files }),
        );
    }
    println!(
        "Exported {} ({} items, version {}) to {}.",
        bundle.manifest.name,
        bundle.files.len(),
        bundle.manifest.version,
        dir.display()
    );
    Ok(())
}

async fn backup(ctx: &Ctx, dir: &Path) -> Result<()> {
    let client = ctx.client()?;
    let sets = client.sets(Some("mine")).await?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut done = Vec::new();
    let mut failed = Vec::new();
    for s in &sets {
        let target = dir.join(dir_name_for_slug(&s.slug));
        match client.export_to_dir(&s.id, &target).await {
            Ok(b) => {
                if !ctx.json {
                    println!("{}: {} items -> {}", s.slug, b.files.len(), target.display());
                }
                done.push(json!({ "slug": s.slug, "id": s.id, "dir": target, "items": b.files.len() }));
            }
            Err(e) => {
                eprintln!("{}: error: {e}", s.slug);
                failed.push(json!({ "slug": s.slug, "id": s.id, "error": e.to_string() }));
            }
        }
    }
    if ctx.json {
        output::json(&json!({ "exported": done, "failed": failed }))?;
    } else {
        println!("Backed up {} of {} sets to {}.", done.len(), sets.len(), dir.display());
    }
    if !failed.is_empty() {
        bail!("{} sets failed", failed.len());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// search & copy
// ---------------------------------------------------------------------------

fn hit_rows(hits: &[SearchHit]) -> Table {
    let mut t = Table::new(&["#", "NAME", "SET", "TYPE", "Q", "LIKES", "COPIES", "MATCHED", "ID"]);
    for (i, h) in hits.iter().enumerate() {
        let it = &h.item;
        let mut name = it.name.clone();
        if h.incompatible.is_some() {
            name.push_str(" (!)");
        }
        t.row(vec![
            (i + 1).to_string(),
            name,
            it.set_slug.clone(),
            it.format.extension().to_string(),
            output::opt(it.quality),
            it.likes.to_string(),
            it.copies.to_string(),
            h.matched.join(","),
            it.id.clone(),
        ]);
    }
    t
}

async fn search(ctx: &Ctx, a: SearchArgs) -> Result<()> {
    let session = ctx.session().await?;
    let query = query_string(&a.query);
    let out = session.search(&query, a.target, a.limit).await?;
    if ctx.json {
        return output::json(&json!({ "offline": out.offline, "hits": out.hits }));
    }
    if out.offline {
        eprintln!("(server unreachable: results from the offline cache)");
    }
    if out.hits.is_empty() {
        println!("No matches.");
        return Ok(());
    }
    hit_rows(&out.hits).print();
    let incompatible: Vec<&SearchHit> = out.hits.iter().filter(|h| h.incompatible.is_some()).collect();
    if let (Some(t), false) = (a.target, incompatible.is_empty()) {
        println!();
        for h in incompatible {
            println!(
                "(!) {}: cannot be sent to {}: {}",
                h.item.name,
                t.as_str(),
                h.incompatible.as_deref().unwrap_or_default()
            );
        }
    }
    Ok(())
}

async fn copy(ctx: &Ctx, a: CopyArgs) -> Result<()> {
    let session = ctx.session().await?;
    let target = a.target.unwrap_or(ctx.config.default_target);
    let sticker = a.sticker || ctx.config.prefer_stickers;
    let (item, offline) = match &a.id {
        Some(id) => (session.item_summary(id).await?, false),
        None => {
            let query = query_string(&a.query);
            let out = session.search(&query, Some(target), 1).await?;
            let hit = out
                .hits
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("no match for {query:?}"))?;
            (hit.item, out.offline)
        }
    };
    let prepared = session.prepare_copy(&item, target, sticker).await?;
    if let Some(w) = &prepared.warning {
        eprintln!("warning: {w}");
    }
    if !a.print_path {
        let exe = std::env::current_exe().context("locating the stickpile binary")?;
        spawn_detached(&exe, &["__serve-clipboard"], &prepared.offers, Duration::from_secs(10))?;
    }
    session.record_copy(&item, &prepared)?;
    // The process exits right away: give the queued event one chance to go out.
    match tokio::time::timeout(
        Duration::from_secs(ctx.config.timeout_secs.max(1)),
        session.flush_usage(),
    )
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::info!("usage event stays queued: {e}"),
        Err(_) => tracing::info!("usage upload timed out; the event stays queued"),
    }
    if ctx.json {
        let mimes: Vec<&str> = prepared.offers.iter().map(|o| o.mime.as_str()).collect();
        return output::json(&json!({
            "item": item,
            "file": prepared.file,
            "variant": prepared.variant,
            "target": prepared.target,
            "sticker": prepared.sticker,
            "offers": mimes,
            "clipboard": !a.print_path,
            "warning": prepared.warning,
            "offline": offline,
        }));
    }
    if a.print_path {
        println!("{}", prepared.file.display());
    } else {
        let kind = if prepared.sticker {
            "sticker"
        } else {
            prepared.variant.as_str()
        };
        println!("Copied {:?} ({kind}) for {}.", item.name, target.as_str());
    }
    Ok(())
}

async fn sync(ctx: &Ctx) -> Result<()> {
    let session = ctx.session().await?;
    let report = session.sync_report().await?;
    let pending = session.pending_usage();
    if ctx.json {
        return output::json(&json!({
            "full": report.full,
            "pages": report.pages,
            "upserted": report.upserted,
            "removed": report.removed,
            "total": report.total,
            "server_time": report.server_time,
            "usage_pending": pending,
        }));
    }
    println!(
        "Synced{}: {} updated, {} removed, {} items cached.",
        if report.full { " (full)" } else { "" },
        report.upserted,
        report.removed,
        report.total
    );
    if pending > 0 {
        println!("{pending} usage events still queued.");
    }
    Ok(())
}

async fn flush(ctx: &Ctx) -> Result<()> {
    let session = ctx.session().await?;
    let r = session.flush_usage().await?;
    if ctx.json {
        return output::json(&json!({
            "sent": r.sent,
            "accepted": r.accepted,
            "duplicates": r.duplicates,
            "rejected": r.rejected,
            "dropped": r.dropped,
            "remaining": r.remaining,
        }));
    }
    println!(
        "Sent {} usage events ({} accepted, {} duplicates, {} rejected); {} queued.",
        r.sent,
        r.accepted,
        r.duplicates,
        r.rejected.len(),
        r.remaining
    );
    Ok(())
}

async fn like(ctx: &Ctx, item: &str, liked: bool) -> Result<()> {
    let client = ctx.client()?;
    let state = if liked {
        client.like(item).await?
    } else {
        client.unlike(item).await?
    };
    if ctx.json {
        return output::json(&state);
    }
    println!(
        "{} ({} likes).",
        if state.liked { "Liked" } else { "Not liked" },
        state.likes
    );
    Ok(())
}

async fn popular(ctx: &Ctx, a: PopularArgs) -> Result<()> {
    let client = ctx.client()?;
    let set = match &a.set {
        Some(s) => Some(resolve_set(&client, s).await?),
        None => None,
    };
    let scope = match a.scope {
        PopularScope::Server => "server",
        PopularScope::Mine => "mine",
        PopularScope::Set => "set",
    };
    let window = match a.window {
        Window::Day => "day",
        Window::Week => "week",
        Window::Month => "month",
        Window::All => "all",
    };
    let p = client
        .popular(&PopularParams {
            scope: Some(scope.into()),
            set,
            window: Some(window.into()),
            limit: Some(a.limit),
        })
        .await?;
    if ctx.json {
        return output::json(&p);
    }
    for (title, col, entries) in [
        ("Most copied", "COPIES", &p.most_copied),
        ("Most liked", "LIKES", &p.most_liked),
        ("Trending (last 7 days)", "COPIES/7D", &p.trending),
    ] {
        println!("{title}:");
        if entries.is_empty() {
            println!("  (nothing yet)\n");
            continue;
        }
        let mut t = Table::new(&["#", col, "NAME", "SET", "TYPE", "ID"]);
        for (i, e) in entries.iter().enumerate() {
            t.row(vec![
                (i + 1).to_string(),
                e.value.to_string(),
                e.item.name.clone(),
                e.item.set_slug.clone(),
                e.item.format.extension().into(),
                e.item.id.clone(),
            ]);
        }
        t.print();
        println!();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// tokens, git packs, telegram, jobs
// ---------------------------------------------------------------------------

async fn tokens(ctx: &Ctx, cmd: TokensCmd) -> Result<()> {
    let client = ctx.client()?;
    match cmd {
        TokensCmd::List => {
            let tokens = client.tokens().await?;
            if ctx.json {
                return output::json(&tokens);
            }
            let current = client.token().unwrap_or_default();
            let mut t = Table::new(&["ID", "NAME", "PREFIX", "CREATED", "LAST USED", "EXPIRES", "STATUS"]);
            for tk in &tokens {
                let status = if tk.revoked {
                    "revoked"
                } else if !tk.prefix.is_empty() && current.starts_with(tk.prefix.trim_end_matches('…')) {
                    "this client"
                } else {
                    "active"
                };
                t.row(vec![
                    tk.id.clone(),
                    tk.name.clone(),
                    tk.prefix.clone(),
                    output::time(tk.created_at),
                    output::opt_time(tk.last_used_at),
                    output::opt_time(tk.expires_at),
                    status.into(),
                ]);
            }
            t.print();
        }
        TokensCmd::Create { name, expires_days } => {
            let expires_at = expires_days.map(|d| unix_now() + d * 86_400);
            let created = client.create_token(&CreateToken { name, expires_at }).await?;
            if ctx.json {
                return output::json(&json!({ "token": created.token, "info": created.info }));
            }
            println!(
                "Created token {:?} ({}). It is shown only once:",
                created.info.name, created.info.id
            );
            println!("{}", created.token);
        }
        TokensCmd::Revoke { id } => {
            client.revoke_token(&id).await?;
            if ctx.json {
                return output::json(&json!({ "revoked": id }));
            }
            println!("Revoked token {id}.");
        }
    }
    Ok(())
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

async fn gitpack(ctx: &Ctx, cmd: GitpackCmd) -> Result<()> {
    let client = ctx.client()?;
    let GitpackCmd::Add {
        url,
        branch,
        subdir,
        mode,
        slug,
    } = cmd;
    let res = client
        .create_gitpack(&CreateGitPack {
            url,
            branch,
            subdir,
            mode: mode.into(),
            slug,
        })
        .await?;
    if ctx.json {
        return output::json(&res);
    }
    let set = res.set();
    println!(
        "Created set {} ({}) from {}.",
        set.slug,
        set.id,
        set.git.as_ref().map(|g| g.url.as_str()).unwrap_or("?")
    );
    if let stickpile_core::client::GitPackResponse::Created(c) = &res {
        println!("Webhook URL:    {}", c.webhook_url);
        println!("Webhook secret: {} (shown once)", c.webhook_secret);
        println!("Clone job:      {} ({})", c.job.id, output::name(&c.job.status));
    }
    Ok(())
}

async fn telegram(ctx: &Ctx, cmd: TelegramCmd) -> Result<()> {
    let client = ctx.client()?;
    match cmd {
        TelegramCmd::Import {
            pack,
            slug,
            visibility,
            wait,
        } => {
            let job = client
                .telegram_import(&TelegramImport {
                    pack_name: pack,
                    slug,
                    visibility,
                })
                .await?;
            show_job(ctx, &client, job, wait).await
        }
        TelegramCmd::Export { set, pack, title, wait } => {
            let id = resolve_set(&client, &set).await?;
            let job = client
                .telegram_export(&id, &TelegramExport { pack_name: pack, title })
                .await?;
            show_job(ctx, &client, job, wait).await
        }
    }
}

async fn show_job(ctx: &Ctx, client: &ApiClient, job: Job, wait: bool) -> Result<()> {
    let job = if wait && !matches!(job.status, JobStatus::Done | JobStatus::Failed) {
        if !ctx.json {
            eprintln!("Waiting for job {}…", job.id);
        }
        client.wait_job(&job.id, JOB_POLL, JOB_MAX_WAIT).await?
    } else {
        job
    };
    if ctx.json {
        output::json(&job)?;
    } else {
        println!(
            "Job {} ({}): {}",
            job.id,
            output::name(&job.kind),
            output::name(&job.status)
        );
        if let Some(s) = &job.subject {
            println!("  subject:  {s}");
        }
        if let Some(r) = &job.result {
            println!("  result:   {r}");
        }
        if let Some(e) = &job.error {
            println!("  error:    {e}");
        }
    }
    if job.status == JobStatus::Failed {
        bail!("job {} failed", job.id);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// admin
// ---------------------------------------------------------------------------

async fn admin(ctx: &Ctx, cmd: AdminCmd) -> Result<()> {
    let client = ctx.client()?;
    match cmd {
        AdminCmd::Users => {
            let users = client.admin_users().await?;
            if ctx.json {
                return output::json(&users);
            }
            let mut t = Table::new(&[
                "USERNAME",
                "ROLE",
                "STATUS",
                "TOTP",
                "SETS",
                "ITEMS",
                "STORAGE",
                "LAST LOGIN",
                "ID",
            ]);
            for u in &users {
                t.row(vec![
                    u.user.username.clone(),
                    output::name(&u.user.role),
                    if u.disabled { "disabled".into() } else { "active".into() },
                    if u.user.totp_enabled { "on".into() } else { "off".into() },
                    u.sets.to_string(),
                    u.items.to_string(),
                    output::bytes(u.storage_bytes),
                    output::opt_time(u.last_login_at),
                    u.user.id.clone(),
                ]);
            }
            t.print();
        }
        AdminCmd::Invite { role, expires_days } => {
            let inv = client
                .admin_create_invitation(&CreateInvitation {
                    role: role.into(),
                    expires_in: expires_days.map(|d| d * 86_400),
                })
                .await?;
            if ctx.json {
                return output::json(&inv);
            }
            println!(
                "Invitation {} ({}), expires {}:",
                inv.invitation.id,
                output::name(&inv.invitation.role),
                output::opt_time(inv.invitation.expires_at)
            );
            println!("{}", inv.url);
        }
        AdminCmd::Audit { limit, before } => {
            let entries = client.admin_audit(before, Some(limit)).await?;
            if ctx.json {
                return output::json(&entries);
            }
            let mut t = Table::new(&["ID", "AT", "ACTOR", "ACTION", "TARGET", "IP"]);
            for e in &entries {
                t.row(vec![
                    e.id.to_string(),
                    output::time(e.at),
                    e.actor_username.clone().unwrap_or_else(|| "-".into()),
                    e.action.clone(),
                    output::opt(e.target.clone()),
                    output::opt(e.ip.clone()),
                ]);
            }
            t.print();
        }
        AdminCmd::Storage => {
            let s = client.admin_storage().await?;
            if ctx.json {
                return output::json(&s);
            }
            println!("media:    {} in {} files", output::bytes(s.media_bytes), s.media_count);
            println!("database: {}", output::bytes(s.database_bytes));
            println!("items:    {}", s.item_count);
            if !s.per_user.is_empty() {
                println!();
                let mut t = Table::new(&["USER", "STORAGE"]);
                for (u, b) in &s.per_user {
                    t.row(vec![u.clone(), output::bytes(*b)]);
                }
                t.print();
            }
        }
        AdminCmd::Jobs { status } => {
            let jobs = client.admin_jobs(status.map(Into::into)).await?;
            if ctx.json {
                return output::json(&jobs);
            }
            let mut t = Table::new(&["ID", "KIND", "STATUS", "ATTEMPTS", "SUBJECT", "CREATED", "ERROR"]);
            for j in &jobs {
                t.row(vec![
                    j.id.clone(),
                    output::name(&j.kind),
                    output::name(&j.status),
                    j.attempts.to_string(),
                    output::opt(j.subject.clone()),
                    output::time(j.created_at),
                    output::opt(j.error.clone()),
                ]);
            }
            t.print();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_dir_names_are_safe() {
        assert_eq!(dir_name_for_slug("my-cats"), "my-cats");
        assert_eq!(dir_name_for_slug("../etc"), "etc");
        assert_eq!(dir_name_for_slug(".."), "set");
        assert_eq!(dir_name_for_slug("a/b"), "a-b");
    }

    #[test]
    fn ids_and_slugs() {
        assert!(looks_like_id("abcdefghijklmnopqrstuvwxyz"));
        assert!(!looks_like_id("cats"));
        assert!(!looks_like_id("ABCDEFGHIJKLMNOPQRSTUVWXYZ"));
    }

    #[test]
    fn offline_errors_get_a_hint() {
        let e = anyhow::Error::from(CoreError::Offline("refused".into()));
        assert!(explain(&e).unwrap().contains("server_url"));
        let e = anyhow::Error::from(CoreError::Api {
            status: 401,
            error: ApiError {
                code: "unauthorized".into(),
                message: "revoked".into(),
            },
        });
        assert!(explain(&e).unwrap().contains("stickpile login"));
        assert!(explain(&anyhow!("other")).is_none());
    }

    #[test]
    fn near_duplicate_wording() {
        let (_server, items) = {
            let server = stickpile_core::mock::MockServer::start();
            let items = {
                let mut st = server.state();
                let s = st.add_set("alice", "cats", "Cats", Visibility::Private);
                let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
                let mut png = std::io::Cursor::new(Vec::new());
                img.write_to(&mut png, image::ImageFormat::Png).unwrap();
                vec![st.add_item(&s.id, "cat", &[], png.into_inner())]
            };
            (server, items)
        };
        let nd = NearDuplicate {
            item_id: "x".into(),
            existing: items[0].clone(),
            distance: 3,
            same_content: false,
        };
        let text = near_duplicate_text(&nd);
        assert!(
            text.contains("in set cats") && text.contains("near-duplicate") && text.contains("distance 3"),
            "{text}"
        );
    }
}
