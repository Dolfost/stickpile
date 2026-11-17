//! `stickpile offline ...` and `stickpile cache clear`: keeping stickers on
//! this device (docs/offline.md). The logic is in `stickpile_core::offline`;
//! this prints, asks and maps the downloader's end to an exit status.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use serde_json::{json, Value};
use stickpile_api::ItemSummary;
use stickpile_core::offline::{
    Estimate, OfflineControl, OfflineState, OfflineStatus, Plan, PlanScope, RunEnd, Scope, ScopeProgress, SkipReason,
    Skipped,
};
use stickpile_core::Session;

use crate::cli::{CacheCmd, OfflineCmd, OfflineStatusArgs, ScopeRef};
use crate::commands::{prompt_line, resolve_set, Ctx, Hinted, Incomplete};
use crate::output::{self, Table};

const MIB: u64 = 1024 * 1024;
/// Rows of the skipped table in `offline status` (`--json` has them all).
const SKIPPED_ROWS: usize = 20;

pub async fn offline(ctx: &Ctx, cmd: OfflineCmd) -> Result<()> {
    match cmd {
        OfflineCmd::Keep { scope, yes, dry_run } => keep(ctx, &scope.scope(), yes, dry_run).await,
        OfflineCmd::Release { scope } => release(ctx, &scope.scope()).await,
        OfflineCmd::Status(a) => status(ctx, a).await,
        OfflineCmd::Sync => sync(ctx).await,
        OfflineCmd::RemoveAll { yes } => remove_all(ctx, yes).await,
    }
}

pub async fn cache(ctx: &Ctx, cmd: CacheCmd) -> Result<()> {
    match cmd {
        CacheCmd::Clear => clear(ctx).await,
    }
}

// ---------------------------------------------------------------------------
// names
// ---------------------------------------------------------------------------

/// A pack as the offline cache knows it (from its items).
#[derive(Debug, Clone)]
struct Pack {
    id: String,
    slug: String,
    name: String,
    owner: String,
}

/// What the offline cache knows, for names and for looking up arguments.
struct Known {
    items: Vec<ItemSummary>,
    /// The account's username.
    me: Option<String>,
}

impl Known {
    fn load(session: &Session) -> Result<Known> {
        Ok(Known {
            items: session.cache().items()?,
            me: session.cache().me()?.map(|u| u.username),
        })
    }

    fn item(&self, id: &str) -> Option<&ItemSummary> {
        self.items.iter().find(|i| i.id == id)
    }

    fn packs(&self) -> Vec<Pack> {
        let mut out: BTreeMap<&str, Pack> = BTreeMap::new();
        for i in &self.items {
            out.entry(i.set_id.as_str()).or_insert_with(|| Pack {
                id: i.set_id.clone(),
                slug: i.set_slug.clone(),
                name: i.set_name.clone(),
                owner: i.owner_username.clone(),
            });
        }
        out.into_values().collect()
    }

    fn pack(&self, id: &str) -> Option<Pack> {
        self.packs().into_iter().find(|p| p.id == id)
    }

    /// A pack by id or slug; with two packs of that slug, the account's own.
    fn find_pack(&self, arg: &str) -> Result<Option<Pack>> {
        let packs = self.packs();
        if let Some(p) = packs.iter().find(|p| p.id == arg) {
            return Ok(Some(p.clone()));
        }
        let matches: Vec<&Pack> = packs.iter().filter(|p| p.slug.eq_ignore_ascii_case(arg)).collect();
        match matches.as_slice() {
            [] => Ok(None),
            [one] => Ok(Some((*one).clone())),
            many => {
                if let Some(own) = many.iter().find(|p| self.me.as_deref() == Some(p.owner.as_str())) {
                    return Ok(Some((*own).clone()));
                }
                let list: Vec<String> = many
                    .iter()
                    .map(|p| format!("{}/{} ({})", p.owner, p.slug, p.id))
                    .collect();
                bail!(
                    "{} packs have the slug {arg:?}: {}; give the set id",
                    many.len(),
                    list.join(", ")
                )
            }
        }
    }

    /// `scope` as found in the cache; `None` when the cache does not know it.
    fn resolve(&self, r: &ScopeRef) -> Result<Option<Found>> {
        Ok(match r {
            ScopeRef::Item(id) => self.item(id).map(|i| Found::item(&i.id, Some(&i.name))),
            ScopeRef::Set(arg) => self.find_pack(arg)?.map(|p| Found::pack(&p)),
            ScopeRef::Library => Some(Found::library()),
        })
    }

    /// A kept scope's label.
    fn label(&self, scope: &Scope) -> Found {
        match scope {
            Scope::Item(id) => Found::item(id, self.item(id).map(|i| i.name.as_str())),
            Scope::Set(id) => match self.pack(id) {
                Some(p) => Found::pack(&p),
                None => Found {
                    scope: scope.clone(),
                    label: format!("pack {id}"),
                    name: None,
                    slug: None,
                },
            },
            Scope::Library => Found::library(),
        }
    }
}

/// A scope looked up, with what to call it.
struct Found {
    scope: Scope,
    /// `pack "Cats" (cats)`, `"grumpy cat" (<id>)`, `the library`.
    label: String,
    name: Option<String>,
    slug: Option<String>,
}

impl Found {
    fn item(id: &str, name: Option<&str>) -> Found {
        Found {
            scope: Scope::Item(id.to_string()),
            label: match name {
                Some(n) => format!("{n:?} ({id})"),
                None => format!("item {id}"),
            },
            name: name.map(str::to_string),
            slug: None,
        }
    }

    fn pack(p: &Pack) -> Found {
        Found {
            scope: Scope::Set(p.id.clone()),
            label: format!("pack {:?} ({})", p.name, p.slug),
            name: Some(p.name.clone()),
            slug: Some(p.slug.clone()),
        }
    }

    fn library() -> Found {
        Found {
            scope: Scope::Library,
            label: "the library".into(),
            name: None,
            slug: None,
        }
    }

    /// The scope as JSON (`{"scope": "set", "id": ...}`) with its name.
    fn json(&self) -> Value {
        let mut v = serde_json::to_value(&self.scope).unwrap_or(Value::Null);
        if let Value::Object(m) = &mut v {
            m.insert("name".into(), json!(self.name));
            if self.slug.is_some() {
                m.insert("slug".into(), json!(self.slug));
            }
        }
        v
    }
}

fn describe(r: &ScopeRef) -> String {
    match r {
        ScopeRef::Item(id) => format!("item {id:?}"),
        ScopeRef::Set(s) => format!("pack {s:?}"),
        ScopeRef::Library => "the library".into(),
    }
}

/// Look `r` up for keeping (or estimating): in the cache, else after a sync,
/// else (a pack of yours with no items yet) on the server.
async fn resolve_keep(session: &Session, r: &ScopeRef) -> Result<Found> {
    if let Some(found) = Known::load(session)?.resolve(r)? {
        return Ok(found);
    }
    match session.sync().await {
        Ok(_) => {}
        Err(e) if e.is_offline() => {
            bail!(
                "{} is not in the offline cache, and the server is unreachable ({e}): run `stickpile sync` while online",
                describe(r)
            )
        }
        Err(e) => return Err(e.into()),
    }
    if let Some(found) = Known::load(session)?.resolve(r)? {
        return Ok(found);
    }
    match r {
        ScopeRef::Item(id) => bail!(
            "no item {id:?} in the offline cache (it lists the items of your packs and of public ones; see `stickpile search`)"
        ),
        ScopeRef::Set(arg) => {
            // A pack none of whose items the cache lists (an empty one of
            // yours): keeping it keeps what is added to it later.
            let id = resolve_set(session.client(), arg).await?;
            let set = session.client().set(&id).await?.set;
            Ok(Found::pack(&Pack {
                id: set.id,
                slug: set.slug,
                name: set.name,
                owner: set.owner_username,
            }))
        }
        ScopeRef::Library => unreachable!("the library is always found"),
    }
}

// ---------------------------------------------------------------------------
// formatting
// ---------------------------------------------------------------------------

/// `1 file`, `3 files`.
fn count(n: u64, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// The first letter upper case.
fn sentence(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// What is kept, counted: `2 stickers, 1 pack and the library`.
fn keeps_text(keeps: &[Scope]) -> String {
    let stickers = keeps.iter().filter(|s| matches!(s, Scope::Item(_))).count() as u64;
    let packs = keeps.iter().filter(|s| matches!(s, Scope::Set(_))).count() as u64;
    let mut parts = Vec::new();
    if stickers > 0 {
        parts.push(count(stickers, "sticker"));
    }
    if packs > 0 {
        parts.push(count(packs, "pack"));
    }
    if keeps.contains(&Scope::Library) {
        parts.push("the library".into());
    }
    match parts.len() {
        0 => "nothing".into(),
        1 => parts.remove(0),
        _ => {
            let last = parts.pop().unwrap_or_default();
            format!("{} and {last}", parts.join(", "))
        }
    }
}

fn budget_text(budget: Option<u64>) -> String {
    match budget {
        Some(b) => output::bytes(b),
        None => "no limit".into(),
    }
}

/// The kept bytes against the budget.
fn against_budget(bytes: u64, budget: Option<u64>) -> String {
    match budget {
        None => format!("{} (no budget: offline.max_mb = 0)", output::bytes(bytes)),
        Some(b) if bytes <= b => format!(
            "{} of the {} budget (offline.max_mb)",
            output::bytes(bytes),
            output::bytes(b)
        ),
        Some(b) => format!(
            "{}, over the {} budget (offline.max_mb): downloading stops at the budget",
            output::bytes(bytes),
            output::bytes(b)
        ),
    }
}

fn skip_text(r: &SkipReason) -> String {
    match r {
        SkipReason::TooLarge { limit, .. } => {
            format!("larger than offline.max_file_mb ({} MiB)", limit / MIB)
        }
        other => other.to_string(),
    }
}

/// Skipped entries grouped by reason: (`2 items`, reason).
fn skip_groups(skipped: &[Skipped]) -> Vec<(String, String)> {
    let mut groups: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for s in skipped {
        let e = groups.entry(skip_text(&s.reason)).or_default();
        if s.variant.is_none() {
            e.0 += 1;
        } else {
            e.1 += 1;
        }
    }
    groups
        .into_iter()
        .map(|(reason, (items, files))| {
            let mut what = Vec::new();
            if items > 0 {
                what.push(count(items, "item"));
            }
            if files > 0 {
                what.push(count(files, "file"));
            }
            (what.join(" and "), reason)
        })
        .collect()
}

fn write_estimate(out: &mut dyn Write, label: &str, est: &Estimate) -> std::io::Result<()> {
    let t = &est.totals;
    writeln!(out, "Keeping {label} offline takes:")?;
    writeln!(out, "  items:        {}", est.items)?;
    writeln!(out, "  files:        {} ({})", t.files, output::bytes(t.bytes))?;
    writeln!(
        out,
        "  on device:    {} ({})",
        t.present_files,
        output::bytes(t.present_bytes)
    )?;
    writeln!(
        out,
        "  to download:  {} ({})",
        t.missing_files(),
        output::bytes(t.missing_bytes())
    )?;
    for (what, reason) in skip_groups(&est.skipped) {
        writeln!(out, "  skipped:      {what}: {reason}")?;
    }
    writeln!(
        out,
        "  kept files afterwards: {}",
        against_budget(est.kept_bytes_after, est.budget)
    )
}

fn estimate_json(found: &Found, est: &Estimate) -> Value {
    json!({
        "scope": found.json(),
        "estimate": est,
        "missing_files": est.totals.missing_files(),
        "missing_bytes": est.totals.missing_bytes(),
        "fits": est.fits(),
    })
}

// ---------------------------------------------------------------------------
// keep, release
// ---------------------------------------------------------------------------

fn confirm(question: &str, refusal: &str) -> Result<()> {
    if !std::io::stdin().is_terminal() {
        bail!("{refusal} without confirmation; pass --yes");
    }
    let answer = prompt_line(&format!("{question} [y/N] "))?;
    if !matches!(answer.to_lowercase().as_str(), "y" | "yes") {
        bail!("aborted");
    }
    Ok(())
}

async fn keep(ctx: &Ctx, r: &ScopeRef, yes: bool, dry_run: bool) -> Result<()> {
    let session = ctx.session().await?;
    let found = resolve_keep(&session, r).await?;
    let est = session.offline_estimate(&found.scope)?;
    if dry_run {
        return show_estimate(ctx, &found, &est);
    }
    if session.offline_keeps()?.contains(&found.scope) {
        if ctx.json {
            return output::json(&json!({ "kept": found.json(), "already": true, "estimate": est }));
        }
        println!("Already kept offline: {}.", found.label);
        return Ok(());
    }
    if !matches!(found.scope, Scope::Item(_)) {
        // A pack or the library: the estimate first.
        if ctx.json {
            write_estimate(&mut std::io::stderr(), &found.label, &est)?;
        } else {
            write_estimate(&mut std::io::stdout(), &found.label, &est)?;
        }
        if !yes {
            confirm(
                &format!("Keep {} offline?", found.label),
                &format!("refusing to keep {}", found.label),
            )?;
        }
    }
    session.offline_keep(&found.scope)?;
    if ctx.json {
        return output::json(&json!({ "kept": found.json(), "already": false, "estimate": est, "fits": est.fits() }));
    }
    let t = &est.totals;
    let rest = if t.missing_files() == 0 {
        "all on this device already".to_string()
    } else {
        format!(
            "{} of them on this device; `stickpile offline sync` downloads the rest",
            t.present_files
        )
    };
    println!(
        "Kept {} offline: {} ({}), {rest}.",
        found.label,
        count(t.files, "file"),
        output::bytes(t.bytes)
    );
    if !est.fits() {
        eprintln!(
            "warning: kept files would take {}",
            against_budget(est.kept_bytes_after, est.budget)
        );
    }
    Ok(())
}

fn show_estimate(ctx: &Ctx, found: &Found, est: &Estimate) -> Result<()> {
    if ctx.json {
        return output::json(&estimate_json(found, est));
    }
    write_estimate(&mut std::io::stdout(), &found.label, est)?;
    Ok(())
}

async fn release(ctx: &Ctx, r: &ScopeRef) -> Result<()> {
    let session = ctx.session().await?;
    let keeps = session.offline_keeps()?;
    let known = Known::load(&session)?;
    let found = match r {
        ScopeRef::Set(arg) if keeps.contains(&Scope::Set(arg.clone())) => known.label(&Scope::Set(arg.clone())),
        ScopeRef::Set(arg) => match known.find_pack(arg)? {
            Some(p) => Found::pack(&p),
            None => bail!("no pack {arg:?} in the offline cache (`stickpile offline status` lists what is kept)"),
        },
        ScopeRef::Item(id) => known.label(&Scope::Item(id.clone())),
        ScopeRef::Library => Found::library(),
    };
    let was_kept = keeps.contains(&found.scope);
    if was_kept {
        session.offline_release(&found.scope)?;
    }
    let usage = session.offline_usage()?;
    if ctx.json {
        return output::json(&json!({ "released": found.json(), "was_kept": was_kept, "usage": usage }));
    }
    if was_kept {
        println!(
            "Released {}: its files stay in the cache until evicted (cache_max_mb).",
            found.label
        );
    } else {
        println!("{} was not kept offline; nothing to release.", sentence(&found.label));
    }
    println!(
        "Kept files now: {} ({}).",
        count(usage.kept_files, "file"),
        output::bytes(usage.kept_bytes)
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

/// The KEPT and NAME columns of a plan scope.
fn scope_columns(scope: &PlanScope, name: Option<&str>, known: &Known, used_count: u32) -> (String, String) {
    let name_or = |fallback: &str| name.map(str::to_string).unwrap_or_else(|| fallback.to_string());
    match scope {
        PlanScope::Thumbnails => ("thumbnails (auto)".into(), "every item's thumbnail".into()),
        PlanScope::Kept(Scope::Item(_)) => ("sticker".into(), name_or("(not in the offline cache)")),
        PlanScope::Kept(Scope::Set(id)) => {
            let slug = known.pack(id).map(|p| format!(" ({})", p.slug)).unwrap_or_default();
            (
                "pack".into(),
                format!("{}{slug}", name_or("(not in the offline cache)")),
            )
        }
        PlanScope::Kept(Scope::Library) => ("library".into(), "everything the cache lists".into()),
        PlanScope::Used => ("used (auto)".into(), format!("the {used_count} most recently copied")),
        PlanScope::Liked => ("liked (auto)".into(), "the stickers you liked".into()),
        PlanScope::OwnPacks => ("own packs (auto)".into(), "the packs you own".into()),
    }
}

fn scope_id(scope: &PlanScope) -> String {
    match scope {
        PlanScope::Kept(Scope::Item(id)) | PlanScope::Kept(Scope::Set(id)) => id.clone(),
        _ => "-".into(),
    }
}

async fn status(ctx: &Ctx, a: OfflineStatusArgs) -> Result<()> {
    let session = ctx.session().await?;
    if let Some(r) = a.scope() {
        let found = resolve_keep(&session, &r).await?;
        let est = session.offline_estimate(&found.scope)?;
        return show_estimate(ctx, &found, &est);
    }
    let plan = session.offline_plan()?;
    let usage = session.offline_usage()?;
    let keeps = session.offline_keeps()?;
    let known = Known::load(&session)?;
    if ctx.json {
        let keeps: Vec<Value> = keeps.iter().map(|s| known.label(s).json()).collect();
        return output::json(&json!({
            "keeps": keeps,
            "usage": usage,
            "items": plan.items,
            "totals": plan.totals,
            "missing_files": plan.totals.missing_files(),
            "missing_bytes": plan.totals.missing_bytes(),
            "complete": plan.complete(),
            "over_budget": plan.over_budget(),
            "budget": plan.budget,
            "scopes": plan.scopes,
            "skipped": plan.skipped,
            "failures": plan.failures,
        }));
    }
    print_status(&session, &plan, &usage, keeps.is_empty(), &known);
    Ok(())
}

fn print_status(
    session: &Session,
    plan: &Plan,
    usage: &stickpile_core::offline::OfflineUsage,
    nothing_chosen: bool,
    known: &Known,
) {
    let used_count = session.offline_options().auto.used_count;
    if plan.scopes.is_empty() {
        println!("Nothing is kept offline: keep a sticker, a pack or the library with `stickpile offline keep`.");
    } else {
        let mut t = Table::new(&["KEPT", "NAME", "ITEMS", "FILES", "SIZE", "ID"]);
        for s in &plan.scopes {
            let (kind, name) = scope_columns(&s.scope, s.name.as_deref(), known, used_count);
            let items = match s.scope {
                PlanScope::Thumbnails => "-".to_string(),
                _ => s.items.to_string(),
            };
            t.row(vec![
                kind,
                name,
                items,
                format!("{} of {}", s.totals.present_files, s.totals.files),
                format!(
                    "{} of {}",
                    output::bytes(s.totals.present_bytes),
                    output::bytes(s.totals.bytes)
                ),
                scope_id(&s.scope),
            ]);
        }
        t.print();
        if nothing_chosen {
            println!("(No sticker, pack or library chosen: `stickpile offline keep <item>|--set <pack>|--all`.)");
        }
    }
    println!();
    println!(
        "Kept files:   {}, {}",
        usage.kept_files,
        against_budget(usage.kept_bytes, usage.budget)
    );
    println!(
        "Other files:  {}, {} (evicted beyond cache_max_mb = {} MiB)",
        usage.other_files,
        output::bytes(usage.other_bytes),
        session.config().cache_max_mb
    );
    let t = &plan.totals;
    if t.missing_files() == 0 {
        println!(
            "To download:  nothing; all {} on this device",
            count(t.files, "wanted file")
        );
    } else {
        println!(
            "To download:  {} of {} ({} of {}): run `stickpile offline sync`",
            t.missing_files(),
            count(t.files, "file"),
            output::bytes(t.missing_bytes()),
            output::bytes(t.bytes)
        );
    }
    if plan.over_budget() {
        println!(
            "Over budget:  the kept files want {}; downloading stops at the {} budget (offline.max_mb)",
            output::bytes(t.bytes),
            budget_text(plan.budget)
        );
    }
    if !plan.skipped.is_empty() {
        println!();
        println!("Skipped ({}):", plan.skipped.len());
        for (what, reason) in skip_groups(&plan.skipped) {
            println!("  {what}: {reason}");
        }
        let mut t = Table::new(&["NAME", "RENDITION", "REASON", "ITEM"]);
        for s in plan.skipped.iter().take(SKIPPED_ROWS) {
            t.row(vec![
                if s.name.is_empty() { "-".into() } else { s.name.clone() },
                s.variant.map_or("-".into(), |v| v.as_str().to_string()),
                skip_text(&s.reason),
                s.item_id.clone(),
            ]);
        }
        t.print();
        if plan.skipped.len() > SKIPPED_ROWS {
            println!(
                "... and {} more (`--json` lists them all)",
                plan.skipped.len() - SKIPPED_ROWS
            );
        }
    }
    if !plan.failures.is_empty() {
        println!();
        println!(
            "Failed ({}; `stickpile offline sync` tries them again):",
            plan.failures.len()
        );
        let mut t = Table::new(&["NAME", "ATTEMPTS", "LAST", "ERROR", "FILE"]);
        for f in &plan.failures {
            t.row(vec![
                known
                    .item(&f.item_id)
                    .map(|i| i.name.clone())
                    .unwrap_or_else(|| f.item_id.clone()),
                f.attempts.to_string(),
                output::time(f.at),
                f.error.clone(),
                f.path.clone(),
            ]);
        }
        t.print();
    }
}

// ---------------------------------------------------------------------------
// sync: the downloader to completion
// ---------------------------------------------------------------------------

/// Progress on stderr: in place on a terminal, a line now and then (and on
/// every change of state) otherwise.
struct Progress {
    on: bool,
    tty: bool,
    last: String,
    at: Option<Instant>,
    state: Option<OfflineState>,
    /// A line in place is open (a newline is owed).
    open: bool,
}

impl Progress {
    fn new(on: bool) -> Progress {
        Progress {
            on,
            tty: std::io::stderr().is_terminal(),
            last: String::new(),
            at: None,
            state: None,
            open: false,
        }
    }

    /// Show `s` unless shown a moment ago (`last`: the run's end, always).
    fn show(&mut self, s: &OfflineStatus, last: bool) {
        if !self.on {
            return;
        }
        let line = progress_line(s);
        if line == self.last {
            return;
        }
        let every = if self.tty {
            Duration::from_millis(100)
        } else {
            Duration::from_secs(2)
        };
        let new_state = self.state != Some(s.state);
        if !last && !new_state && self.at.is_some_and(|t| t.elapsed() < every) {
            return;
        }
        if self.tty {
            eprint!("\r{line}\x1b[K");
            self.open = true;
        } else {
            eprintln!("{line}");
        }
        self.last = line;
        self.at = Some(Instant::now());
        self.state = Some(s.state);
    }

    fn finish(&mut self) {
        if self.open {
            eprintln!();
            self.open = false;
        }
    }
}

fn state_text(state: OfflineState) -> &'static str {
    match state {
        OfflineState::Idle => "done",
        OfflineState::Downloading => "downloading",
        OfflineState::Paused => "paused",
        OfflineState::WaitingForNetwork => "waiting for the network",
        OfflineState::LimitReached => "budget reached",
        OfflineState::DiskFull => "disk full",
    }
}

fn progress_line(s: &OfflineStatus) -> String {
    let failed = if s.failed > 0 {
        format!(", {} failed", s.failed)
    } else {
        String::new()
    };
    format!(
        "offline: {}: {}/{} files, {}/{}{failed}",
        state_text(s.state),
        s.files_done,
        s.files_total,
        output::bytes(s.bytes_done),
        output::bytes(s.bytes_total)
    )
}

/// The status of a run that did not start (the server is unreachable).
fn plan_status(plan: &Plan, message: String) -> OfflineStatus {
    OfflineStatus {
        state: OfflineState::WaitingForNetwork,
        files_done: plan.totals.present_files,
        files_total: plan.totals.files,
        bytes_done: plan.totals.present_bytes,
        bytes_total: plan.totals.bytes,
        skipped: plan.skipped.len() as u64,
        failed: plan.failures.len() as u64,
        scopes: plan
            .scopes
            .iter()
            .map(|s| ScopeProgress {
                scope: s.scope.clone(),
                name: s.name.clone(),
                files_done: s.totals.present_files,
                files_total: s.totals.files,
                bytes_done: s.totals.present_bytes,
                bytes_total: s.totals.bytes,
            })
            .collect(),
        message: Some(message),
    }
}

/// Run the downloader to its end, showing the progress. The first Ctrl-C
/// cancels the run (the downloads stop, their part files go); a second one
/// drops it at once (which cleans up as well).
async fn run_downloader(ctx: &Ctx, session: &Session) -> Result<(RunEnd, OfflineStatus)> {
    let control = OfflineControl::new();
    let mut rx = control.subscribe();
    let mut progress = Progress::new(!ctx.json);
    let run = session.offline_run(&control);
    tokio::pin!(run);
    let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());
    let mut cancelled = false;
    let ended = loop {
        tokio::select! {
            out = &mut run => break Some(out),
            Ok(()) = rx.changed() => {
                let s = rx.borrow_and_update().clone();
                progress.show(&s, false);
            }
            _ = &mut ctrl_c => {
                progress.finish();
                if cancelled {
                    eprintln!("Stopped.");
                    break None;
                }
                cancelled = true;
                control.cancel();
                eprintln!("Cancelling (Ctrl-C again stops at once)...");
                ctrl_c = Box::pin(tokio::signal::ctrl_c());
            }
        }
    };
    match ended {
        Some(out) => {
            if let Ok(out) = &out {
                progress.show(&out.status, true);
            }
            progress.finish();
            let out = out?;
            Ok((out.end, out.status))
        }
        None => {
            progress.finish();
            Ok((RunEnd::Cancelled, control.status()))
        }
    }
}

async fn sync(ctx: &Ctx) -> Result<()> {
    let session = ctx.session().await?;
    let (synced, unreachable) = match session.sync_report().await {
        Ok(r) => {
            if !ctx.json {
                println!(
                    "Synced{}: {} updated, {} removed, {} items cached.",
                    if r.full { " (full)" } else { "" },
                    r.upserted,
                    r.removed,
                    r.total
                );
            }
            let j = json!({
                "full": r.full,
                "pages": r.pages,
                "upserted": r.upserted,
                "removed": r.removed,
                "total": r.total,
                "server_time": r.server_time,
            });
            (j, None)
        }
        Err(e) if e.is_offline() => (Value::Null, Some(e.to_string())),
        Err(e) => return Err(e.into()),
    };
    let before = session.offline_plan()?;
    let (end, status) = match unreachable {
        // Nothing can be fetched; the plan says how far the device is.
        Some(message) => (RunEnd::WaitingForNetwork, plan_status(&before, message)),
        None => run_downloader(ctx, &session).await?,
    };
    let budget = session.offline_usage()?.budget;
    let complete = end == RunEnd::Complete && status.files_done == status.files_total;
    let got_files = status.files_done.saturating_sub(before.totals.present_files);
    let got_bytes = status.bytes_done.saturating_sub(before.totals.present_bytes);
    if ctx.json {
        output::json(&json!({
            "sync": synced,
            "end": end,
            "complete": complete,
            "downloaded_files": got_files,
            "downloaded_bytes": got_bytes,
            "budget": budget,
            "status": status,
        }))?;
    } else {
        println!(
            "Downloaded {} ({}); {} of {} kept files are on this device ({} of {}).",
            count(got_files, "file"),
            output::bytes(got_bytes),
            status.files_done,
            status.files_total,
            output::bytes(status.bytes_done),
            output::bytes(status.bytes_total)
        );
    }
    sync_end(end, complete, &status, budget)
}

/// The exit of `offline sync`: 0 complete, 2 [`Incomplete`], 1 an error.
fn sync_end(end: RunEnd, complete: bool, s: &OfflineStatus, budget: Option<u64>) -> Result<()> {
    let missing_files = s.files_total.saturating_sub(s.files_done);
    let missing_bytes = s.bytes_total.saturating_sub(s.bytes_done);
    let left = if missing_files == 0 {
        "everything kept is on this device (as of the last sync)".to_string()
    } else {
        format!(
            "{} ({}) not on this device",
            count(missing_files, "file"),
            output::bytes(missing_bytes)
        )
    };
    let message = s.message.clone().unwrap_or_default();
    // The core's network errors say "server unreachable: ..." already.
    let unreachable = if message.contains("unreachable") {
        message.clone()
    } else {
        format!("the server is unreachable ({message})")
    };
    let incomplete = |reason: String, hint: &str| -> Result<()> {
        Err(Incomplete {
            reason,
            hint: Some(hint.to_string()),
        }
        .into())
    };
    match end {
        RunEnd::Complete if complete => Ok(()),
        RunEnd::Complete => incomplete(
            format!("{} failed; {left}", count(s.failed, "file")),
            "`stickpile offline status` lists the failures with their reasons; `stickpile offline sync` tries them again",
        ),
        RunEnd::LimitReached => incomplete(
            format!(
                "the offline budget is reached: kept files take {} and offline.max_mb is {}; {left}; nothing was deleted",
                output::bytes(s.bytes_done),
                budget_text(budget)
            ),
            "raise offline.max_mb in the config, or release something (`stickpile offline release`)",
        ),
        RunEnd::WaitingForNetwork => incomplete(
            format!("{unreachable}; {left}"),
            "run `stickpile offline sync` again when the server is back",
        ),
        RunEnd::Cancelled => incomplete(
            format!("cancelled; {left}"),
            "`stickpile offline sync` goes on where it stopped",
        ),
        RunEnd::DiskFull => incomplete(
            format!("the disk is full ({message}); {left}; nothing was deleted"),
            "free some space and run `stickpile offline sync` again",
        ),
        RunEnd::Unauthorized => Err(Hinted {
            message: format!("the server rejected the token ({message})"),
            hint: "run `stickpile login`".into(),
        }
        .into()),
    }
}

// ---------------------------------------------------------------------------
// clearing
// ---------------------------------------------------------------------------

async fn remove_all(ctx: &Ctx, yes: bool) -> Result<()> {
    let session = ctx.session().await?;
    let usage = session.offline_usage()?;
    let keeps = session.offline_keeps()?;
    if usage.kept_files == 0 && keeps.is_empty() {
        if ctx.json {
            return output::json(&json!({ "removed_files": 0, "freed_bytes": 0, "forgotten": 0 }));
        }
        println!("Nothing is kept offline.");
        return Ok(());
    }
    if !yes {
        confirm(
            &format!(
                "Delete {} ({}) and forget what is kept ({})?",
                count(usage.kept_files, "kept file"),
                output::bytes(usage.kept_bytes),
                keeps_text(&keeps)
            ),
            "refusing to delete the kept files",
        )?;
    }
    let report = session.offline_remove_all()?;
    let auto = &session.offline_options().auto;
    let rules: Vec<&str> = [
        (auto.thumbnails, "thumbnails"),
        (auto.used, "used"),
        (auto.liked, "liked"),
        (auto.own_packs, "own_packs"),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .map(|(_, n)| *n)
    .collect();
    if ctx.json {
        return output::json(&json!({
            "removed_files": report.removed_files,
            "freed_bytes": report.freed_bytes,
            "forgotten": keeps.len(),
            "other_bytes": report.total_bytes,
            "auto_rules": rules,
        }));
    }
    println!(
        "Deleted {} ({}) and forgot what was kept ({}).",
        count(report.removed_files as u64, "kept file"),
        output::bytes(report.freed_bytes),
        keeps_text(&keeps)
    );
    if !rules.is_empty() {
        println!(
            "The automatic rules on in [offline.auto] ({}) keep their files again at the next `stickpile offline sync`; turn them off in the config to stop that.",
            rules.join(", ")
        );
    }
    Ok(())
}

async fn clear(ctx: &Ctx) -> Result<()> {
    let session = ctx.session().await?;
    let r = session.clear_cache()?;
    if ctx.json {
        return output::json(&json!({
            "removed_files": r.removed_files,
            "freed_bytes": r.freed_bytes,
            "other_bytes": r.total_bytes,
            "kept_files": r.kept_files,
            "kept_bytes": r.kept_bytes,
        }));
    }
    print!(
        "Deleted {} ({}).",
        count(r.removed_files as u64, "cached file"),
        output::bytes(r.freed_bytes)
    );
    if r.kept_files > 0 {
        print!(
            " {} ({}) kept offline stay: `stickpile offline remove-all` deletes them.",
            count(r.kept_files as u64, "file"),
            output::bytes(r.kept_bytes)
        );
    }
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use stickpile_api::{MediaKind, Variant};

    fn skipped(reason: SkipReason, variant: Option<Variant>) -> Skipped {
        Skipped {
            item_id: "x".into(),
            name: "x".into(),
            variant,
            reason,
        }
    }

    #[test]
    fn skipped_entries_group_by_reason() {
        let groups = skip_groups(&[
            skipped(SkipReason::KindOff { kind: MediaKind::Video }, None),
            skipped(SkipReason::KindOff { kind: MediaKind::Video }, None),
            skipped(
                SkipReason::TooLarge {
                    size: 3 * MIB,
                    limit: 2 * MIB,
                },
                Some(Variant::Original),
            ),
            skipped(
                SkipReason::TooLarge {
                    size: 5 * MIB,
                    limit: 2 * MIB,
                },
                Some(Variant::Png),
            ),
        ]);
        assert_eq!(
            groups,
            [
                (
                    "2 files".to_string(),
                    "larger than offline.max_file_mb (2 MiB)".to_string()
                ),
                (
                    "2 items".to_string(),
                    "videos are not kept with packs, the library or automatic rules".to_string()
                ),
            ]
        );
    }

    #[test]
    fn budget_wording() {
        assert_eq!(
            against_budget(512 * 1024, Some(MIB)),
            "512.0 KiB of the 1.0 MiB budget (offline.max_mb)"
        );
        assert!(against_budget(2 * MIB, Some(MIB)).contains("over the 1.0 MiB budget"));
        assert!(against_budget(2 * MIB, None).contains("no budget"));
        assert_eq!(count(1, "file"), "1 file");
        assert_eq!(count(0, "file"), "0 files");
        assert_eq!(sentence("the library"), "The library");
        let keeps = [
            Scope::Item("a".into()),
            Scope::Library,
            Scope::Item("b".into()),
            Scope::Set("c".into()),
        ];
        assert_eq!(keeps_text(&keeps), "2 stickers, 1 pack and the library");
        assert_eq!(keeps_text(&keeps[..1]), "1 sticker");
        assert_eq!(keeps_text(&[]), "nothing");
    }

    fn status(files: (u64, u64), failed: u64, message: Option<&str>) -> OfflineStatus {
        OfflineStatus {
            files_done: files.0,
            files_total: files.1,
            bytes_done: files.0 * MIB,
            bytes_total: files.1 * MIB,
            failed,
            message: message.map(str::to_string),
            ..OfflineStatus::default()
        }
    }

    fn incomplete(r: Result<()>) -> Incomplete {
        let e = r.unwrap_err();
        let i = e
            .downcast_ref::<Incomplete>()
            .unwrap_or_else(|| panic!("not incomplete: {e}"));
        Incomplete {
            reason: i.reason.clone(),
            hint: i.hint.clone(),
        }
    }

    #[test]
    fn each_end_maps_to_its_exit() {
        assert!(sync_end(RunEnd::Complete, true, &status((3, 3), 0, None), None).is_ok());
        let i = incomplete(sync_end(RunEnd::Complete, false, &status((2, 3), 1, None), None));
        assert!(
            i.reason
                .starts_with("1 file failed; 1 file (1.0 MiB) not on this device"),
            "{}",
            i.reason
        );
        let i = incomplete(sync_end(
            RunEnd::LimitReached,
            false,
            &status((1, 3), 0, Some("offline.max_mb (1 MiB) is reached")),
            Some(MIB),
        ));
        assert!(
            i.reason.contains("budget is reached") && i.reason.contains("offline.max_mb is 1.0 MiB"),
            "{}",
            i.reason
        );
        assert!(
            i.reason.contains("2 files (2.0 MiB) not on this device"),
            "{}",
            i.reason
        );
        let i = incomplete(sync_end(
            RunEnd::WaitingForNetwork,
            false,
            &status((0, 1), 0, Some("connection refused")),
            None,
        ));
        assert!(i.reason.contains("unreachable (connection refused)"), "{}", i.reason);
        let i = incomplete(sync_end(RunEnd::Cancelled, false, &status((0, 1), 0, None), None));
        assert!(i.reason.starts_with("cancelled"), "{}", i.reason);
        let i = incomplete(sync_end(
            RunEnd::DiskFull,
            false,
            &status((0, 1), 0, Some("No space left on device")),
            None,
        ));
        assert!(i.reason.contains("disk is full"), "{}", i.reason);
        let e = sync_end(RunEnd::Unauthorized, false, &status((0, 1), 0, Some("401")), None).unwrap_err();
        assert!(e.downcast_ref::<Incomplete>().is_none(), "an error, not incomplete");
        assert!(e.to_string().contains("rejected the token"));
    }
}
