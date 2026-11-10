//! Git packs (docs/git-packs.md): a set bound to a remote repository. The
//! server clones it into `<data_dir>/gitpacks/<set id>`, imports the set
//! directory, re-pulls on a schedule or webhook, and in `push` mode commits
//! exported edits back.
//!
//! Remote URLs are user input: [`validate_url`] enforces the configured scheme
//! allow-list (no `file://` unless explicitly allowed, which only tests do) and
//! rejects credentials embedded in URLs.

use std::path::{Path, PathBuf};

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
    /// HEAD commit after the operation (hex).
    pub commit: String,
    /// False if HEAD did not move (nothing to import).
    pub changed: bool,
}

/// Check a remote URL against an allow-list of schemes (`https`, `http`,
/// `ssh` — which also admits scp-like `git@host:path` — and `file`).
pub fn validate_url(url: &str, allowed_schemes: &[String]) -> Result<(), GitPackError> {
    let _ = (url, allowed_schemes);
    todo!("gitpack agent")
}

/// Clone into `dest` if absent, else fetch and hard-reset to the remote
/// branch (local changes are discarded: the database is the source of truth).
/// `branch = None` uses the remote HEAD. Shallow when supported.
pub fn sync(url: &str, branch: Option<&str>, dest: &Path, auth: &GitAuth, max_bytes: u64) -> Result<Checkout, GitPackError> {
    let _ = (url, branch, dest, auth, max_bytes);
    todo!("gitpack agent")
}

/// Stage everything under `repo_dir`, commit (if anything changed) and push
/// the current branch. Returns the new HEAD.
pub fn commit_and_push(repo_dir: &Path, message: &str, author_name: &str, author_email: &str, auth: &GitAuth) -> Result<Checkout, GitPackError> {
    let _ = (repo_dir, message, author_name, author_email, auth);
    todo!("gitpack agent")
}

/// Browser URL for "edit upstream" (GitHub, GitLab, Codeberg/Gitea/Forgejo
/// style hosts): `https://host/owner/repo/tree/<branch>/<subdir>`.
pub fn web_url(url: &str, branch: Option<&str>, subdir: Option<&str>) -> Option<String> {
    let _ = (url, branch, subdir);
    todo!("gitpack agent")
}

/// Verify a GitHub/Gitea `X-Hub-Signature-256: sha256=<hex>` header in
/// constant time.
pub fn verify_hub_signature(secret: &[u8], body: &[u8], header: &str) -> bool {
    let _ = (secret, body, header);
    todo!("gitpack agent")
}
