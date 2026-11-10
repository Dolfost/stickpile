//! Git packs (docs/git-packs.md): a set bound to a remote repository. The
//! server clones it into `<data_dir>/gitpacks/<set id>`, imports the set
//! directory, re-pulls on a schedule or webhook, and in `push` mode commits
//! exported edits back.
//!
//! Remote URLs are user input: [`validate_url`] enforces the configured scheme
//! allow-list (no `file://` unless explicitly allowed, which only tests do) and
//! rejects credentials embedded in URLs.
//!
//! Checkouts are made with `core.symlinks = false`, so a symlink committed
//! upstream becomes a small regular file holding the link target and can never
//! point the importer outside the checkout. libgit2 runs no hooks and no
//! external filter drivers.

use std::cell::{Cell, RefCell};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use git2::build::CheckoutBuilder;
use git2::{
    AutotagOption, Cred, CredentialType, Direction, ErrorCode, FetchOptions, IndexAddOption, ObjectType, Oid,
    ProxyOptions, PushOptions, RemoteCallbacks, Repository, RepositoryInitOptions, ResetType, Signature, TreeWalkMode,
    TreeWalkResult,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

#[derive(Debug, thiserror::Error)]
pub enum GitPackError {
    #[error("URL not allowed: {0}")]
    Url(String),
    #[error("repository exceeds size limit ({0} bytes)")]
    TooLarge(u64),
    #[error("git: {0}")]
    Git(#[from] git2::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Default)]
pub struct GitAuth {
    pub ssh_key: Option<PathBuf>,
    pub https_username: String,
    pub https_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Checkout {
    /// HEAD commit after the operation (hex). Empty when the remote
    /// repository has no commits yet (push mode into a fresh repository).
    pub commit: String,
    /// False if HEAD did not move (nothing to import).
    pub changed: bool,
}

const REMOTE: &str = "origin";
const MAX_URL_LEN: usize = 2048;
/// The download may be this many times `max_bytes` (history and compression
/// overhead) before it is aborted; the checkout itself must fit `max_bytes`.
const TRANSFER_FACTOR: u64 = 4;
/// Branch used for HEAD when the remote is empty and no branch was given.
pub const DEFAULT_BRANCH: &str = "main";
/// Username sent with an HTTPS token when [`GitAuth::https_username`] is empty
/// (accepted by GitHub, GitLab, Gitea and Forgejo for personal access tokens).
pub const DEFAULT_TOKEN_USER: &str = "x-access-token";

fn git_err(msg: impl AsRef<str>) -> GitPackError {
    GitPackError::Git(git2::Error::from_str(msg.as_ref()))
}

// ---------------------------------------------------------------------------
// URL policy
// ---------------------------------------------------------------------------

/// Check a remote URL against an allow-list of schemes (`https`, `http`,
/// `ssh` — which also admits scp-like `git@host:path` — and `file`).
///
/// Always rejected: credentials in the URL (`https://user:token@…`; for ssh a
/// user name is fine but a password is not), transport helpers (`ext::…`,
/// `fd::…`), local paths without `file://`, values that look like command-line
/// options (leading `-`, also for host, user and scp path), whitespace and
/// control characters, and URLs longer than 2048 bytes.
pub fn validate_url(url: &str, allowed_schemes: &[String]) -> Result<(), GitPackError> {
    let deny = |why: &str| Err(GitPackError::Url(format!("{}: {why}", redact_url(url))));
    let allowed = |scheme: &str| allowed_schemes.iter().any(|s| s.eq_ignore_ascii_case(scheme));
    if url.is_empty() {
        return deny("empty");
    }
    if url.len() > MAX_URL_LEN {
        return deny("too long");
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return deny("contains whitespace or control characters");
    }
    if url.starts_with('-') {
        return deny("looks like a command-line option");
    }
    if let Some((prefix, _)) = url.split_once("::") {
        if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) {
            return deny("transport helpers (`<transport>::<address>`) are not allowed");
        }
    }
    if url.contains("://") {
        let parsed = match url::Url::parse(url) {
            Ok(u) => u,
            Err(e) => return deny(&format!("invalid URL ({e})")),
        };
        let scheme = parsed.scheme();
        if !allowed(scheme) {
            return deny(&format!("scheme {scheme:?} is not allowed"));
        }
        let host = parsed.host_str().unwrap_or("");
        if host.starts_with('-') || parsed.username().starts_with('-') {
            return deny("looks like a command-line option");
        }
        match scheme {
            "file" => {
                if !(host.is_empty() || host == "localhost") {
                    return deny("file:// URLs must not name a host");
                }
                if parsed.path().len() <= 1 {
                    return deny("missing path");
                }
            }
            "http" | "https" => {
                if !parsed.username().is_empty() || parsed.password().is_some() {
                    return deny("credentials in the URL are not allowed; configure a token instead");
                }
                if host.is_empty() {
                    return deny("missing host");
                }
            }
            _ => {
                if parsed.password().is_some() {
                    return deny("passwords in the URL are not allowed; configure an SSH key instead");
                }
                if host.is_empty() {
                    return deny("missing host");
                }
            }
        }
        return Ok(());
    }
    // scp-like `[user@]host:path`; anything else is a local path.
    let Some((host_part, path)) = url.split_once(':') else {
        return deny("local paths are not allowed (use file:// if enabled)");
    };
    if host_part.contains('/') || host_part.contains('\\') {
        return deny("local paths are not allowed (use file:// if enabled)");
    }
    if !allowed("ssh") {
        return deny("scp-like ssh URLs need the \"ssh\" scheme to be allowed");
    }
    let (user, host) = match host_part.rsplit_once('@') {
        Some((user, host)) => (Some(user), host),
        None => (None, host_part),
    };
    if let Some(user) = user {
        if user.is_empty() || user.starts_with('-') || user.contains('@') {
            return deny("invalid user name");
        }
    }
    if host.len() < 2 || host.starts_with('-') || !host.chars().all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c))
    {
        return deny("invalid host (local paths are not allowed)");
    }
    if path.is_empty() || path.starts_with('-') || path.starts_with('\\') {
        return deny("invalid repository path");
    }
    let first_segment = path.split('/').next().unwrap_or("");
    if first_segment.contains('@') {
        return deny("credentials in the URL are not allowed");
    }
    Ok(())
}

/// The URL with any `user:password@` part replaced by `***@` (for logs and errors).
pub fn redact_url(url: &str) -> String {
    if let Some((scheme, rest)) = url.split_once("://") {
        let authority_end = rest.find('/').unwrap_or(rest.len());
        if let Some(at) = rest[..authority_end].rfind('@') {
            return format!("{scheme}://***@{}", &rest[at + 1..]);
        }
    }
    url.to_string()
}

// ---------------------------------------------------------------------------
// Sync (clone / fetch + hard reset)
// ---------------------------------------------------------------------------

/// Clone into `dest` if absent, else fetch and hard-reset to the remote
/// branch (local changes are discarded: the database is the source of truth).
/// `branch = None` uses the remote HEAD. Shallow when supported.
///
/// Details:
/// * The remote refs are listed first; if the tracked tip is already present
///   nothing is downloaded (the reset still runs, so local edits and
///   untracked/ignored files are always removed).
/// * Shallow (depth 1) fetches are used where the transport supports them;
///   libgit2's local transport (`file://`) does not, so it falls back to a
///   full fetch.
/// * Size: the sum of blob sizes of the target tree must be `<= max_bytes`,
///   checked before anything is checked out; the download is aborted past
///   `4 * max_bytes`. On violation `dest` is deleted and
///   [`GitPackError::TooLarge`] returned.
/// * A failed first clone removes `dest` again.
/// * An empty remote yields `Checkout { commit: "", changed: false }` with
///   HEAD on `branch` (default `main`), ready for [`commit_and_push`].
pub fn sync(
    url: &str,
    branch: Option<&str>,
    dest: &Path,
    auth: &GitAuth,
    max_bytes: u64,
) -> Result<Checkout, GitPackError> {
    if url.starts_with('-') {
        return Err(GitPackError::Url(format!(
            "{}: looks like a command-line option",
            redact_url(url)
        )));
    }
    if let Some(b) = branch {
        if !git2::Branch::name_is_valid(b)? || b.starts_with('-') {
            return Err(git_err(format!("invalid branch name {b:?}")));
        }
    }
    let fresh = !dest.exists();
    let result = sync_inner(url, branch, dest, auth, max_bytes);
    match &result {
        Err(GitPackError::TooLarge(_)) if dest.exists() => {
            let _ = fs::remove_dir_all(dest);
        }
        Err(_) if fresh && dest.exists() => {
            let _ = fs::remove_dir_all(dest);
        }
        _ => {}
    }
    result
}

fn sync_inner(
    url: &str,
    branch: Option<&str>,
    dest: &Path,
    auth: &GitAuth,
    max_bytes: u64,
) -> Result<Checkout, GitPackError> {
    let repo = open_or_init(dest, url)?;
    let mut remote = repo.find_remote(REMOTE)?;

    // 1. Which branch? (`Connection::list` is avoided on purpose: git2 0.20
    //    builds a slice from a null pointer when the remote has no refs.)
    let branch = match branch {
        Some(b) => b.to_string(),
        None => match remote_default_branch(&mut remote, auth)? {
            Some(b) => b,
            None if remote_is_empty(&repo, &mut remote, auth, max_bytes)? => {
                return empty_checkout(&repo, DEFAULT_BRANCH);
            }
            None => {
                return Err(git_err(
                    "cannot determine the remote default branch; set a branch explicitly",
                ))
            }
        },
    };
    let local = format!("refs/heads/{branch}");
    let tracking = format!("refs/remotes/{REMOTE}/{branch}");

    // 2. Fetch the branch tip. The stale tracking ref is dropped first so a
    //    branch deleted upstream is noticed; objects stay in the ODB and the
    //    local branch still advertises them, so nothing is downloaded twice.
    if let Ok(mut stale) = repo.find_reference(&tracking) {
        stale.delete()?;
    }
    fetch(&mut remote, &format!("+{local}:{tracking}"), auth, max_bytes)?;
    let Ok(target) = repo.refname_to_id(&tracking) else {
        if remote_default_branch(&mut remote, auth)?.is_none() && remote_is_empty(&repo, &mut remote, auth, max_bytes)?
        {
            return empty_checkout(&repo, &branch);
        }
        return Err(git_err(format!("branch {branch:?} not found on the remote")));
    };
    let commit = repo.find_commit(target)?;

    // 3. Size check before touching the worktree.
    if tree_size(&repo, &commit.tree()?, max_bytes)?.is_none() {
        return Err(GitPackError::TooLarge(max_bytes));
    }

    // 4. Point the local branch at the tip and make the worktree match exactly.
    let old = repo.head().ok().and_then(|h| h.target());
    repo.reference(&local, target, true, "stickpile: sync")?;
    repo.set_head(&local)?;
    repo.reset(commit.as_object(), ResetType::Hard, None)?;
    // `reset` overrides the checkout strategy, so cleaning is a second pass
    // (the equivalent of `git clean -ffdx`).
    let mut co = CheckoutBuilder::new();
    co.force().remove_untracked(true).remove_ignored(true);
    repo.checkout_head(Some(&mut co))?;
    let mut cfg = repo.config()?;
    cfg.set_str(&format!("branch.{branch}.remote"), REMOTE)?;
    cfg.set_str(&format!("branch.{branch}.merge"), &local)?;
    tracing::debug!(url = %redact_url(url), %branch, commit = %target, "git pack synced");
    Ok(Checkout {
        commit: target.to_string(),
        changed: old != Some(target),
    })
}

/// `Some(branch)` from the remote HEAD symref, `None` if the remote does not
/// advertise a usable HEAD (empty repository, or HEAD on an unborn branch).
fn remote_default_branch(remote: &mut git2::Remote<'_>, auth: &GitAuth) -> Result<Option<String>, GitPackError> {
    let tries = Cell::new(0);
    let conn = remote.connect_auth(Direction::Fetch, Some(callbacks(auth, &tries)), Some(proxy()))?;
    match conn.default_branch() {
        Ok(buf) => {
            let full = buf.as_str().unwrap_or_default().to_string();
            match full.strip_prefix("refs/heads/") {
                Some(b) => Ok(Some(b.to_string())),
                None => Err(git_err(format!(
                    "remote HEAD points to {full:?}, not a branch; set a branch explicitly"
                ))),
            }
        }
        Err(e) if e.code() == ErrorCode::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// True if the remote has no branches at all (fetches all branch tips, so
/// only used on the rare paths where HEAD gave no answer).
fn remote_is_empty(
    repo: &Repository,
    remote: &mut git2::Remote<'_>,
    auth: &GitAuth,
    max_bytes: u64,
) -> Result<bool, GitPackError> {
    let glob = format!("refs/remotes/{REMOTE}/*");
    for r in repo.references_glob(&glob)? {
        r?.delete()?;
    }
    fetch(remote, &format!("+refs/heads/*:{}", glob), auth, max_bytes)?;
    let empty = repo.references_glob(&glob)?.next().is_none();
    Ok(empty)
}

/// HEAD on `branch` with no commits: the state for pushing into an empty remote.
fn empty_checkout(repo: &Repository, branch: &str) -> Result<Checkout, GitPackError> {
    let old = repo.head().ok().and_then(|h| h.target());
    repo.set_head(&format!("refs/heads/{branch}"))?;
    Ok(Checkout {
        commit: String::new(),
        changed: old.is_some(),
    })
}

fn open_or_init(dest: &Path, url: &str) -> Result<Repository, GitPackError> {
    let repo = if dest.join(".git").exists() {
        Repository::open(dest)?
    } else {
        if dest.exists() && fs::read_dir(dest)?.next().is_some() {
            return Err(GitPackError::Io(std::io::Error::other(format!(
                "{} exists and is not a git repository",
                dest.display()
            ))));
        }
        let mut opts = RepositoryInitOptions::new();
        opts.mkpath(true).external_template(false).initial_head(DEFAULT_BRANCH);
        Repository::init_opts(dest, &opts)?
    };
    let current = repo.find_remote(REMOTE).ok().map(|r| r.url().map(str::to_string));
    match current {
        Some(Some(u)) if u == url => {}
        Some(_) => repo.remote_set_url(REMOTE, url)?,
        None => {
            repo.remote(REMOTE, url)?;
        }
    }
    let mut cfg = repo.config()?;
    cfg.set_bool("core.symlinks", false)?;
    Ok(repo)
}

fn fetch(remote: &mut git2::Remote<'_>, refspec: &str, auth: &GitAuth, max_bytes: u64) -> Result<(), GitPackError> {
    let cap = max_bytes.saturating_mul(TRANSFER_FACTOR);
    let mut depth = 1;
    loop {
        let tries = Cell::new(0);
        let exceeded = Cell::new(false);
        let mut cb = callbacks(auth, &tries);
        cb.transfer_progress(|p| {
            if p.received_bytes() as u64 > cap {
                exceeded.set(true);
                return false;
            }
            true
        });
        let mut opts = FetchOptions::new();
        opts.remote_callbacks(cb)
            .proxy_options(proxy())
            .download_tags(AutotagOption::None)
            .depth(depth);
        let result = remote.fetch(&[refspec], Some(&mut opts), Some("stickpile: fetch"));
        drop(opts);
        if exceeded.get() {
            return Err(GitPackError::TooLarge(max_bytes));
        }
        match result {
            Ok(()) => return Ok(()),
            Err(e) if depth != 0 && shallow_unsupported(&e) => {
                tracing::debug!("shallow fetch not supported by this transport, fetching full history");
                depth = 0;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn shallow_unsupported(e: &git2::Error) -> bool {
    // GIT_ENOTSUPPORTED has no `ErrorCode` variant in git2 0.20.
    const GIT_ENOTSUPPORTED: i32 = -23;
    e.raw_code() == GIT_ENOTSUPPORTED || e.message().to_ascii_lowercase().contains("shallow")
}

/// Sum of blob sizes in `tree`, or `None` once it exceeds `max`.
fn tree_size(repo: &Repository, tree: &git2::Tree<'_>, max: u64) -> Result<Option<u64>, git2::Error> {
    let odb = repo.odb()?;
    let mut total: u64 = 0;
    let mut failure = None;
    let mut over = false;
    let walked = tree.walk(TreeWalkMode::PreOrder, |_, entry| {
        if entry.kind() == Some(ObjectType::Blob) {
            match odb.read_header(entry.id()) {
                Ok((size, _)) => total = total.saturating_add(size as u64),
                Err(e) => {
                    failure = Some(e);
                    return TreeWalkResult::Abort;
                }
            }
            if total > max {
                over = true;
                return TreeWalkResult::Abort;
            }
        }
        TreeWalkResult::Ok
    });
    if let Some(e) = failure {
        return Err(e);
    }
    if over {
        return Ok(None);
    }
    walked?;
    Ok(Some(total))
}

fn proxy() -> ProxyOptions<'static> {
    let mut p = ProxyOptions::new();
    p.auto();
    p
}

/// What to answer a libgit2 credential request with.
#[derive(Debug, PartialEq)]
enum CredChoice<'a> {
    Username(&'a str),
    SshKey { user: &'a str, key: &'a Path },
    Token { user: &'a str, token: &'a str },
    GiveUp(&'static str),
}

/// Credentials: an SSH key file for ssh remotes, username + token for HTTPS.
/// Gives up after a few attempts instead of letting libgit2 loop forever on
/// rejected credentials.
fn choose_credential<'a>(
    auth: &'a GitAuth,
    username_from_url: Option<&'a str>,
    allowed: CredentialType,
    attempt: u32,
) -> CredChoice<'a> {
    if attempt >= 4 {
        return CredChoice::GiveUp("authentication failed (credentials rejected)");
    }
    let user = username_from_url.unwrap_or("git");
    if allowed.contains(CredentialType::USERNAME) {
        return CredChoice::Username(user);
    }
    if allowed.contains(CredentialType::SSH_KEY) {
        if let Some(key) = &auth.ssh_key {
            return CredChoice::SshKey { user, key };
        }
    }
    if allowed.contains(CredentialType::USER_PASS_PLAINTEXT) {
        if let Some(token) = &auth.https_token {
            let user = if auth.https_username.is_empty() {
                DEFAULT_TOKEN_USER
            } else {
                &auth.https_username
            };
            return CredChoice::Token { user, token };
        }
    }
    CredChoice::GiveUp("authentication required but no matching credentials are configured")
}

fn callbacks<'a>(auth: &'a GitAuth, tries: &'a Cell<u32>) -> RemoteCallbacks<'a> {
    let mut cb = RemoteCallbacks::new();
    cb.credentials(move |_url, username_from_url, allowed| {
        let attempt = tries.get();
        tries.set(attempt + 1);
        match choose_credential(auth, username_from_url, allowed, attempt) {
            CredChoice::Username(user) => Cred::username(user),
            CredChoice::SshKey { user, key } => Cred::ssh_key(user, None, key, None),
            CredChoice::Token { user, token } => Cred::userpass_plaintext(user, token),
            CredChoice::GiveUp(why) => Err(git2::Error::from_str(why)),
        }
    });
    cb
}

// ---------------------------------------------------------------------------
// Commit and push
// ---------------------------------------------------------------------------

/// Stage everything under `repo_dir`, commit (if anything changed) and push
/// the current branch. Returns the new HEAD.
///
/// Staging includes deletions and respects `.gitignore`. The push is a normal
/// fast-forward push: if upstream moved since the last [`sync`] it is
/// rejected (an error); the caller re-syncs, re-exports and tries again. A
/// commit left unpushed by an earlier failure is pushed by the next call even
/// when nothing new changed (`changed` is then false).
pub fn commit_and_push(
    repo_dir: &Path,
    message: &str,
    author_name: &str,
    author_email: &str,
    auth: &GitAuth,
) -> Result<Checkout, GitPackError> {
    let repo = Repository::open(repo_dir)?;
    let head = repo.find_reference("HEAD")?;
    let branch_ref = head
        .symbolic_target()
        .ok_or_else(|| git_err("HEAD is detached"))?
        .to_string();
    let branch = branch_ref
        .strip_prefix("refs/heads/")
        .ok_or_else(|| git_err(format!("HEAD points to {branch_ref:?}, not a branch")))?
        .to_string();
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());

    let mut index = repo.index()?;
    index.add_all(["*"], IndexAddOption::DEFAULT, None)?;
    index.update_all(["*"], None)?;
    index.write()?;
    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;
    let changed = match &parent {
        Some(p) => p.tree_id() != tree_id,
        None => !tree.is_empty(),
    };
    let new_head = if changed {
        let sig = Signature::now(author_name, author_email)?;
        let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)?
    } else {
        match &parent {
            Some(p) => p.id(),
            None => {
                return Ok(Checkout {
                    commit: String::new(),
                    changed: false,
                })
            }
        }
    };

    let tracking = format!("refs/remotes/{REMOTE}/{branch}");
    if repo.refname_to_id(&tracking).ok() != Some(new_head) {
        push(&repo, &branch_ref, auth)?;
        repo.reference(&tracking, new_head, true, "stickpile: push")?;
    }
    Ok(Checkout {
        commit: new_head.to_string(),
        changed,
    })
}

fn push(repo: &Repository, branch_ref: &str, auth: &GitAuth) -> Result<(), GitPackError> {
    let mut remote = repo.find_remote(REMOTE)?;
    let tries = Cell::new(0);
    let rejected: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let mut cb = callbacks(auth, &tries);
    cb.push_update_reference(|refname, status| {
        if let Some(msg) = status {
            rejected.borrow_mut().push(format!("{refname}: {msg}"));
        }
        Ok(())
    });
    let mut opts = PushOptions::new();
    opts.remote_callbacks(cb).proxy_options(proxy());
    remote.push(&[format!("{branch_ref}:{branch_ref}")], Some(&mut opts))?;
    drop(opts);
    let rejected = rejected.into_inner();
    if !rejected.is_empty() {
        return Err(git_err(format!("push rejected: {}", rejected.join("; "))));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Web URLs and webhooks
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Forge {
    /// GitHub and unknown hosts: `/tree/<ref>/<path>`.
    GitHub,
    /// GitLab: `/-/tree/<ref>/<path>`, nested groups allowed.
    GitLab,
    /// Gitea, Forgejo, Codeberg: `/src/branch/<ref>/<path>`.
    Gitea,
}

fn forge(host: &str) -> Forge {
    let host = host.to_ascii_lowercase();
    let first = host.split('.').next().unwrap_or("");
    if host == "gitlab.com" || first == "gitlab" || host.ends_with(".gitlab.io") {
        Forge::GitLab
    } else if host == "codeberg.org" || matches!(first, "gitea" | "forgejo") || host == "gitea.com" {
        Forge::Gitea
    } else {
        Forge::GitHub
    }
}

/// Browser URL for "edit upstream" (GitHub, GitLab, Codeberg/Gitea/Forgejo
/// style hosts): `https://host/owner/repo/tree/<branch>/<subdir>`.
///
/// * GitHub and unrecognised hosts: `…/owner/repo/tree/<branch or HEAD>/<subdir>`.
/// * GitLab (`gitlab.com`, `gitlab.*`): `…/group[/subgroup]/repo/-/tree/<branch or HEAD>/<subdir>`.
/// * Codeberg, `gitea.*`, `forgejo.*`: `…/owner/repo/src/branch/<branch>/<subdir>`
///   (without a branch: `…/src/<subdir>`, which Gitea resolves on the default branch).
///
/// With neither branch nor subdir the repository page is returned. ssh and
/// scp-like remotes map to `https://host/…` (the ssh port is dropped); http
/// and https keep scheme and port. `None` for `file://`, local paths and
/// shapes that are not `owner/repo` (more segments are only accepted for
/// GitLab).
pub fn web_url(url: &str, branch: Option<&str>, subdir: Option<&str>) -> Option<String> {
    let (scheme, host, port, path) = if url.contains("://") {
        let parsed = url::Url::parse(url).ok()?;
        let host = parsed.host_str()?.to_string();
        match parsed.scheme() {
            "https" | "http" => (
                parsed.scheme().to_string(),
                host,
                parsed.port(),
                parsed.path().to_string(),
            ),
            "ssh" | "git" | "git+ssh" | "ssh+git" => ("https".to_string(), host, None, parsed.path().to_string()),
            _ => return None,
        }
    } else {
        let (host_part, path) = url.split_once(':')?;
        if host_part.contains('/') {
            return None;
        }
        let host = host_part.rsplit_once('@').map_or(host_part, |(_, h)| h);
        if host.len() < 2 {
            return None;
        }
        ("https".to_string(), host.to_string(), None, path.to_string())
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let kind = forge(&host);
    if segments.len() < 2 || (segments.len() > 2 && kind != Forge::GitLab) {
        return None;
    }
    if segments.iter().any(|s| *s == "." || *s == ".." || s.starts_with('-')) {
        return None;
    }
    let mut out = url::Url::parse(&format!("{scheme}://{host}/")).ok()?;
    if let Some(port) = port {
        out.set_port(Some(port)).ok()?;
    }
    let subdir: Vec<&str> = subdir
        .map(|s| s.split('/').filter(|p| !p.is_empty() && *p != ".").collect())
        .unwrap_or_default();
    if subdir.contains(&"..") {
        return None;
    }
    let branch = branch.filter(|b| !b.is_empty());
    {
        let mut segs = out.path_segments_mut().ok()?;
        segs.pop_if_empty();
        segs.extend(&segments);
        if branch.is_some() || !subdir.is_empty() {
            match kind {
                Forge::GitHub => {
                    segs.push("tree");
                    segs.extend(branch.unwrap_or("HEAD").split('/'));
                }
                Forge::GitLab => {
                    segs.extend(["-", "tree"]);
                    segs.extend(branch.unwrap_or("HEAD").split('/'));
                }
                Forge::Gitea => {
                    segs.push("src");
                    if let Some(b) = branch {
                        segs.push("branch");
                        segs.extend(b.split('/'));
                    }
                }
            }
            segs.extend(&subdir);
        }
    }
    Some(out.to_string())
}

/// Verify a GitHub/Gitea `X-Hub-Signature-256: sha256=<hex>` header in
/// constant time.
///
/// The bare hex form (Gitea/Forgejo `X-Gitea-Signature`,
/// `X-Forgejo-Signature`) is accepted too. An empty secret never verifies.
pub fn verify_hub_signature(secret: &[u8], body: &[u8], header: &str) -> bool {
    if secret.is_empty() {
        return false;
    }
    let header = header.trim();
    let hex_sig = header
        .strip_prefix("sha256=")
        .or_else(|| header.strip_prefix("SHA256="))
        .unwrap_or(header);
    let Ok(sig) = hex::decode(hex_sig) else { return false };
    if sig.len() != 32 {
        return false;
    }
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&sig).is_ok()
}

/// Constant-time comparison of a shared-secret header such as GitLab's
/// `X-Gitlab-Token`. An empty secret never verifies.
pub fn verify_token(secret: &[u8], header: &str) -> bool {
    !secret.is_empty() && bool::from(secret.ct_eq(header.trim().as_bytes()))
}

/// Process-wide libgit2 network timeouts (connect, and per read/write).
/// Call once at startup, before any network operation.
pub fn set_network_timeouts(connect: Duration, io: Duration) -> Result<(), GitPackError> {
    let ms = |d: Duration| i32::try_from(d.as_millis()).unwrap_or(i32::MAX);
    // SAFETY: libgit2 global options; the caller runs this before spawning git work.
    unsafe {
        git2::opts::set_server_connect_timeout_in_milliseconds(ms(connect))?;
        git2::opts::set_server_timeout_in_milliseconds(ms(io))?;
    }
    Ok(())
}

/// The commit id of `HEAD` in an existing checkout, if any.
pub fn head_commit(repo_dir: &Path) -> Option<String> {
    let repo = Repository::open(repo_dir).ok()?;
    let oid: Oid = repo.head().ok()?.target()?;
    Some(oid.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_choice() {
        let none = GitAuth::default();
        let ssh = GitAuth {
            ssh_key: Some(PathBuf::from("/keys/id_ed25519")),
            ..Default::default()
        };
        let token = GitAuth {
            https_token: Some("t0k".into()),
            ..Default::default()
        };
        let named = GitAuth {
            https_username: "alice".into(),
            https_token: Some("t0k".into()),
            ..Default::default()
        };
        let key = Path::new("/keys/id_ed25519");

        assert_eq!(
            choose_credential(&ssh, None, CredentialType::USERNAME, 0),
            CredChoice::Username("git")
        );
        assert_eq!(
            choose_credential(&ssh, Some("deploy"), CredentialType::SSH_KEY, 1),
            CredChoice::SshKey { user: "deploy", key }
        );
        assert_eq!(
            choose_credential(&ssh, Some("git"), CredentialType::SSH_KEY, 1),
            CredChoice::SshKey { user: "git", key }
        );
        assert!(matches!(
            choose_credential(&none, None, CredentialType::SSH_KEY, 0),
            CredChoice::GiveUp(_)
        ));
        assert_eq!(
            choose_credential(&token, None, CredentialType::USER_PASS_PLAINTEXT, 0),
            CredChoice::Token {
                user: DEFAULT_TOKEN_USER,
                token: "t0k"
            }
        );
        assert_eq!(
            choose_credential(&named, None, CredentialType::USER_PASS_PLAINTEXT, 0),
            CredChoice::Token {
                user: "alice",
                token: "t0k"
            }
        );
        assert!(matches!(
            choose_credential(&ssh, None, CredentialType::USER_PASS_PLAINTEXT, 0),
            CredChoice::GiveUp(_)
        ));
        assert!(matches!(
            choose_credential(&token, None, CredentialType::USER_PASS_PLAINTEXT, 4),
            CredChoice::GiveUp(_)
        ));
    }
}
