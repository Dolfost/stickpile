//! Shared helpers of the `integrations_*` tests: local git repositories, an
//! HMAC for webhook signatures, and a compact in-process mock of the
//! Telegram Bot API.
#![allow(dead_code)]

pub mod mock_bot;

use std::fs;
use std::path::{Path, PathBuf};

use git2::{IndexAddOption, Repository, Signature};

use crate::common::TestServer;

pub fn file_url(path: &Path) -> String {
    format!("file://{}", path.display())
}

/// A maintainer's working repository on branch `main`.
pub fn work_repo(path: &Path) -> Repository {
    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    Repository::init_opts(path, &opts).unwrap()
}

pub fn bare_repo(path: &Path) -> Repository {
    let mut opts = git2::RepositoryInitOptions::new();
    opts.bare(true).initial_head("main");
    Repository::init_opts(path, &opts).unwrap()
}

/// Write/delete files (paths relative to the work tree), stage all, commit.
pub fn commit(repo: &Repository, files: &[(&str, &[u8])], delete: &[&str], msg: &str) -> String {
    let root = repo.workdir().unwrap().to_path_buf();
    for (name, bytes) in files {
        let p = root.join(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }
    for name in delete {
        fs::remove_file(root.join(name)).unwrap();
    }
    let mut index = repo.index().unwrap();
    index.add_all(["*"], IndexAddOption::DEFAULT, None).unwrap();
    index.update_all(["*"], None).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = Signature::now("Maintainer", "m@example.org").unwrap();
    let parent = repo.head().ok().map(|h| h.peel_to_commit().unwrap());
    let parents: Vec<_> = parent.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents)
        .unwrap()
        .to_string()
}

/// Push `main` of `repo` to the bare repository at `bare`.
pub fn push_main(repo: &Repository, bare: &Path) {
    let mut remote = repo.remote_anonymous(&file_url(bare)).unwrap();
    remote.push(&["refs/heads/main:refs/heads/main"], None).unwrap();
}

/// Fetch `main` of `bare` into `repo` and hard-reset to it.
pub fn pull_main(repo: &Repository, bare: &Path) {
    let mut remote = repo.remote_anonymous(&file_url(bare)).unwrap();
    remote
        .fetch(&["refs/heads/main:refs/remotes/upstream/main"], None, None)
        .unwrap();
    let oid = repo.refname_to_id("refs/remotes/upstream/main").unwrap();
    let obj = repo.find_object(oid, None).unwrap();
    repo.reset(&obj, git2::ResetType::Hard, None).unwrap();
}

/// Tip of `branch` in a (bare) repository: (commit id, author name).
pub fn tip(repo_path: &Path, branch: &str) -> (String, String) {
    let repo = Repository::open(repo_path).unwrap();
    let c = repo
        .find_reference(&format!("refs/heads/{branch}"))
        .unwrap()
        .peel_to_commit()
        .unwrap();
    let author = c.author().name().unwrap_or_default().to_string();
    (c.id().to_string(), author)
}

/// Files (path -> bytes) of the tree at the tip of `branch`.
pub fn tree_files(repo_path: &Path, branch: &str) -> std::collections::BTreeMap<String, Vec<u8>> {
    let repo = Repository::open(repo_path).unwrap();
    let tree = repo
        .find_reference(&format!("refs/heads/{branch}"))
        .unwrap()
        .peel_to_tree()
        .unwrap();
    let mut out = std::collections::BTreeMap::new();
    tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
        if entry.kind() == Some(git2::ObjectType::Blob) {
            let blob = repo.find_blob(entry.id()).unwrap();
            out.insert(format!("{dir}{}", entry.name().unwrap()), blob.content().to_vec());
        }
        git2::TreeWalkResult::Ok
    })
    .unwrap();
    out
}

/// HMAC-SHA256 as hex (what GitHub/Gitea put in `X-Hub-Signature-256`).
pub fn hmac_sha256_hex(key: &[u8], msg: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    hex::encode(outer.finalize())
}

/// Make every queued job runnable now (skips push debouncing and retry
/// backoff), then run them all.
pub async fn run_jobs_now(srv: &TestServer) -> usize {
    let mut total = 0;
    loop {
        sqlx::query("UPDATE jobs SET run_after = 0 WHERE status = 'queued'")
            .execute(&srv.state.db)
            .await
            .unwrap();
        let n = srv.run_jobs().await;
        if n == 0 {
            return total;
        }
        total += n;
    }
}

pub fn tempdir_path(dir: &tempfile::TempDir, rel: &str) -> PathBuf {
    dir.path().join(rel)
}
