//! Clone, re-pull, push and size limits against local `file://` repositories.

use std::fs;
use std::path::{Path, PathBuf};

use git2::{IndexAddOption, Repository, Signature};
use stickpile_gitpack::{commit_and_push, head_commit, sync, Checkout, GitAuth, GitPackError};

const MB: u64 = 1 << 20;

fn url(path: &Path) -> String {
    format!("file://{}", path.display())
}

/// A maintainer's working repository on branch `main`.
fn work_repo(path: &Path) -> Repository {
    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    Repository::init_opts(path, &opts).unwrap()
}

/// Write/delete files in the work tree, stage everything and commit.
fn commit(repo: &Repository, files: &[(&str, &[u8])], delete: &[&str], msg: &str) -> String {
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

fn push_to(repo: &Repository, bare: &Path, branch: &str) {
    let mut remote = repo.remote_anonymous(&url(bare)).unwrap();
    let spec = format!("refs/heads/{branch}:refs/heads/{branch}");
    remote.push(&[spec.as_str()], None).unwrap();
}

fn bare_repo(path: &Path) -> Repository {
    let mut opts = git2::RepositoryInitOptions::new();
    opts.bare(true).initial_head("main");
    Repository::init_opts(path, &opts).unwrap()
}

fn read(path: PathBuf) -> String {
    fs::read_to_string(path).unwrap()
}

#[test]
fn clone_then_repull() {
    let tmp = tempfile::tempdir().unwrap();
    let upstream = work_repo(&tmp.path().join("upstream"));
    let c1 = commit(
        &upstream,
        &[("set.toml", b"name = \"Cats\"\nversion = 1\n"), ("001-a.png", b"a")],
        &[],
        "one",
    );
    let dest = tmp.path().join("checkout");
    let auth = GitAuth::default();

    let co = sync(&url(upstream.workdir().unwrap()), None, &dest, &auth, MB).unwrap();
    assert_eq!(
        co,
        Checkout {
            commit: c1.clone(),
            changed: true
        }
    );
    assert_eq!(read(dest.join("001-a.png")), "a");
    assert_eq!(head_commit(&dest).as_deref(), Some(c1.as_str()));

    // Nothing new upstream.
    let co = sync(&url(upstream.workdir().unwrap()), None, &dest, &auth, MB).unwrap();
    assert_eq!(
        co,
        Checkout {
            commit: c1.clone(),
            changed: false
        }
    );

    // Upstream adds, modifies and deletes.
    let c2 = commit(
        &upstream,
        &[("set.toml", b"name = \"Cats\"\nversion = 2\n"), ("002-b.gif", b"b")],
        &["001-a.png"],
        "two",
    );
    let co = sync(&url(upstream.workdir().unwrap()), None, &dest, &auth, MB).unwrap();
    assert_eq!(
        co,
        Checkout {
            commit: c2.clone(),
            changed: true
        }
    );
    assert!(read(dest.join("set.toml")).contains("version = 2"));
    assert!(dest.join("002-b.gif").exists());
    assert!(!dest.join("001-a.png").exists());
}

#[test]
fn local_changes_are_discarded() {
    let tmp = tempfile::tempdir().unwrap();
    let upstream = work_repo(&tmp.path().join("upstream"));
    let c1 = commit(
        &upstream,
        &[
            ("set.toml", b"name = \"x\""),
            ("a.png", b"a"),
            (".gitignore", b"*.log\n"),
        ],
        &[],
        "1",
    );
    let dest = tmp.path().join("checkout");
    let u = url(upstream.workdir().unwrap());
    sync(&u, None, &dest, &GitAuth::default(), MB).unwrap();

    fs::write(dest.join("a.png"), b"edited").unwrap();
    fs::remove_file(dest.join("set.toml")).unwrap();
    fs::write(dest.join("untracked.png"), b"u").unwrap();
    fs::write(dest.join("ignored.log"), b"i").unwrap();
    fs::create_dir_all(dest.join("newdir/deeper")).unwrap();
    fs::write(dest.join("newdir/deeper/x.png"), b"x").unwrap();

    let co = sync(&u, None, &dest, &GitAuth::default(), MB).unwrap();
    assert_eq!(
        co,
        Checkout {
            commit: c1,
            changed: false
        }
    );
    assert_eq!(read(dest.join("a.png")), "a");
    assert!(dest.join("set.toml").exists());
    assert!(!dest.join("untracked.png").exists());
    assert!(!dest.join("ignored.log").exists());
    assert!(!dest.join("newdir").exists());
}

#[test]
fn explicit_branch_and_missing_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let upstream = work_repo(&tmp.path().join("upstream"));
    commit(&upstream, &[("a.png", b"main")], &[], "main");
    let head = upstream.head().unwrap().peel_to_commit().unwrap();
    upstream.branch("dev", &head, false).unwrap();
    upstream.set_head("refs/heads/dev").unwrap();
    let dev = commit(&upstream, &[("a.png", b"dev")], &[], "dev");
    upstream.set_head("refs/heads/main").unwrap();
    let u = url(upstream.workdir().unwrap());

    let dest = tmp.path().join("dev-checkout");
    let co = sync(&u, Some("dev"), &dest, &GitAuth::default(), MB).unwrap();
    assert_eq!(co.commit, dev);
    assert_eq!(read(dest.join("a.png")), "dev");

    let dest = tmp.path().join("main-checkout");
    sync(&u, None, &dest, &GitAuth::default(), MB).unwrap();
    assert_eq!(read(dest.join("a.png")), "main");

    let missing = tmp.path().join("missing");
    let err = sync(&u, Some("nope"), &missing, &GitAuth::default(), MB).unwrap_err();
    assert!(err.to_string().contains("nope"), "{err}");
    assert!(!missing.exists(), "a failed first clone leaves nothing behind");

    assert!(sync(&u, Some("../evil"), &missing, &GitAuth::default(), MB).is_err());
    assert!(sync(&u, Some("-x"), &missing, &GitAuth::default(), MB).is_err());
}

#[test]
fn size_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let upstream = work_repo(&tmp.path().join("upstream"));
    // Incompressible-ish content so the pack is not tiny.
    let big: Vec<u8> = (0..200_000u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    commit(&upstream, &[("a.png", b"small")], &[], "1");
    let u = url(upstream.workdir().unwrap());

    let dest = tmp.path().join("checkout");
    sync(&u, None, &dest, &GitAuth::default(), 100_000).unwrap();
    assert!(dest.join("a.png").exists());

    // Upstream grows past the limit: the checkout is removed.
    commit(&upstream, &[("big.webm", &big)], &[], "2");
    match sync(&u, None, &dest, &GitAuth::default(), 100_000) {
        Err(GitPackError::TooLarge(limit)) => assert_eq!(limit, 100_000),
        other => panic!("{other:?}"),
    }
    assert!(!dest.exists());

    // Fresh clone over the limit.
    let fresh = tmp.path().join("fresh");
    assert!(matches!(
        sync(&u, None, &fresh, &GitAuth::default(), 1000),
        Err(GitPackError::TooLarge(_))
    ));
    assert!(!fresh.exists());

    // Within the limit it works.
    sync(&u, None, &fresh, &GitAuth::default(), MB).unwrap();
    assert!(fresh.join("big.webm").exists());
}

#[test]
fn upstream_symlinks_become_plain_files() {
    let tmp = tempfile::tempdir().unwrap();
    let upstream = work_repo(&tmp.path().join("upstream"));
    let root = upstream.workdir().unwrap().to_path_buf();
    std::os::unix::fs::symlink("/etc/passwd", root.join("evil.png")).unwrap();
    std::os::unix::fs::symlink("/etc", root.join("evildir")).unwrap();
    commit(&upstream, &[("a.png", b"a")], &[], "links");
    let dest = tmp.path().join("checkout");
    sync(&url(&root), None, &dest, &GitAuth::default(), MB).unwrap();
    let meta = fs::symlink_metadata(dest.join("evil.png")).unwrap();
    assert!(meta.is_file(), "symlink was checked out as a link");
    assert_eq!(read(dest.join("evil.png")), "/etc/passwd");
    assert!(fs::symlink_metadata(dest.join("evildir")).unwrap().is_file());
}

#[test]
fn remote_url_change_is_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let one = work_repo(&tmp.path().join("one"));
    commit(&one, &[("a.png", b"one")], &[], "1");
    let two = work_repo(&tmp.path().join("two"));
    let c2 = commit(&two, &[("a.png", b"two")], &[], "1");
    let dest = tmp.path().join("checkout");
    sync(&url(one.workdir().unwrap()), None, &dest, &GitAuth::default(), MB).unwrap();
    let co = sync(&url(two.workdir().unwrap()), None, &dest, &GitAuth::default(), MB).unwrap();
    assert_eq!(
        co,
        Checkout {
            commit: c2,
            changed: true
        }
    );
    assert_eq!(read(dest.join("a.png")), "two");
}

#[test]
fn existing_non_repo_destination_is_not_touched() {
    let tmp = tempfile::tempdir().unwrap();
    let upstream = work_repo(&tmp.path().join("upstream"));
    commit(&upstream, &[("a.png", b"a")], &[], "1");
    let dest = tmp.path().join("occupied");
    fs::create_dir(&dest).unwrap();
    fs::write(dest.join("keep.txt"), b"keep").unwrap();
    assert!(sync(&url(upstream.workdir().unwrap()), None, &dest, &GitAuth::default(), MB).is_err());
    assert!(dest.join("keep.txt").exists());
}

#[test]
fn unreachable_remote_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("checkout");
    let err = sync(
        &url(&tmp.path().join("does-not-exist")),
        None,
        &dest,
        &GitAuth::default(),
        MB,
    )
    .unwrap_err();
    assert!(matches!(err, GitPackError::Git(_)), "{err}");
    assert!(!dest.exists());
}

// ---------------------------------------------------------------------------
// Push mode
// ---------------------------------------------------------------------------

fn seeded_bare(tmp: &Path) -> (PathBuf, Repository) {
    let bare = tmp.join("pack.git");
    bare_repo(&bare);
    let maintainer = work_repo(&tmp.join("maintainer"));
    commit(
        &maintainer,
        &[
            ("set.toml", b"name = \"P\"\n"),
            ("001-a.png", b"a"),
            ("002-b.png", b"b"),
        ],
        &[],
        "init",
    );
    push_to(&maintainer, &bare, "main");
    (bare, maintainer)
}

#[test]
fn commit_and_push_to_bare_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let (bare, _) = seeded_bare(tmp.path());
    let dest = tmp.path().join("server-checkout");
    let auth = GitAuth::default();
    let first = sync(&url(&bare), None, &dest, &auth, MB).unwrap();

    // Nothing changed yet.
    let co = commit_and_push(&dest, "noop", "stickpile", "bot@example.org", &auth).unwrap();
    assert_eq!(
        co,
        Checkout {
            commit: first.commit.clone(),
            changed: false
        }
    );

    // Export: modify, add, delete.
    fs::write(dest.join("set.toml"), b"name = \"P\"\nversion = 2\n").unwrap();
    fs::write(dest.join("003-c.png"), b"c").unwrap();
    fs::remove_file(dest.join("002-b.png")).unwrap();
    let co = commit_and_push(&dest, "stickpile: export", "stickpile", "bot@example.org", &auth).unwrap();
    assert!(co.changed);
    assert_ne!(co.commit, first.commit);

    let upstream = Repository::open_bare(&bare).unwrap();
    let tip = upstream
        .find_reference("refs/heads/main")
        .unwrap()
        .peel_to_commit()
        .unwrap();
    assert_eq!(tip.id().to_string(), co.commit);
    assert_eq!(tip.author().name(), Some("stickpile"));
    assert_eq!(tip.author().email(), Some("bot@example.org"));
    assert_eq!(tip.message(), Some("stickpile: export"));
    assert_eq!(tip.parent_id(0).unwrap().to_string(), first.commit);
    let tree = tip.tree().unwrap();
    assert!(tree.get_name("003-c.png").is_some());
    assert!(tree.get_name("002-b.png").is_none(), "deletions are staged");

    // Re-sync sees our own commit: nothing to import.
    let co2 = sync(&url(&bare), None, &dest, &auth, MB).unwrap();
    assert_eq!(
        co2,
        Checkout {
            commit: co.commit.clone(),
            changed: false
        }
    );

    // And a second identical export is a no-op.
    let co3 = commit_and_push(&dest, "again", "stickpile", "bot@example.org", &auth).unwrap();
    assert_eq!(
        co3,
        Checkout {
            commit: co.commit,
            changed: false
        }
    );
}

#[test]
fn push_is_rejected_when_upstream_moved() {
    let tmp = tempfile::tempdir().unwrap();
    let (bare, maintainer) = seeded_bare(tmp.path());
    let dest = tmp.path().join("server-checkout");
    let auth = GitAuth::default();
    sync(&url(&bare), None, &dest, &auth, MB).unwrap();

    // Someone else pushes first.
    let theirs = commit(&maintainer, &[("004-d.png", b"d")], &[], "upstream edit");
    push_to(&maintainer, &bare, "main");

    fs::write(dest.join("005-e.png"), b"e").unwrap();
    let err = commit_and_push(&dest, "ours", "stickpile", "bot@example.org", &auth).unwrap_err();
    assert!(matches!(err, GitPackError::Git(_)), "{err}");
    let upstream = Repository::open_bare(&bare).unwrap();
    assert_eq!(upstream.refname_to_id("refs/heads/main").unwrap().to_string(), theirs);

    // The documented recovery: re-sync (drops our commit), re-export, push.
    let co = sync(&url(&bare), None, &dest, &auth, MB).unwrap();
    assert_eq!(
        co,
        Checkout {
            commit: theirs.clone(),
            changed: true
        }
    );
    assert!(!dest.join("005-e.png").exists());
    fs::write(dest.join("005-e.png"), b"e").unwrap();
    let co = commit_and_push(&dest, "ours", "stickpile", "bot@example.org", &auth).unwrap();
    assert!(co.changed);
    assert_eq!(
        upstream.refname_to_id("refs/heads/main").unwrap().to_string(),
        co.commit
    );
}

#[test]
fn push_into_an_empty_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let bare = tmp.path().join("empty.git");
    bare_repo(&bare);
    let dest = tmp.path().join("server-checkout");
    let auth = GitAuth::default();
    let co = sync(&url(&bare), None, &dest, &auth, MB).unwrap();
    assert_eq!(
        co,
        Checkout {
            commit: String::new(),
            changed: false
        }
    );

    // Nothing to commit yet.
    let co = commit_and_push(&dest, "empty", "stickpile", "bot@example.org", &auth).unwrap();
    assert_eq!(
        co,
        Checkout {
            commit: String::new(),
            changed: false
        }
    );

    fs::write(dest.join("set.toml"), b"name = \"New\"\n").unwrap();
    fs::write(dest.join("001-a.png"), b"a").unwrap();
    let co = commit_and_push(&dest, "first export", "stickpile", "bot@example.org", &auth).unwrap();
    assert!(co.changed);
    let upstream = Repository::open_bare(&bare).unwrap();
    assert_eq!(
        upstream.refname_to_id("refs/heads/main").unwrap().to_string(),
        co.commit
    );

    // Another server checkout of the now non-empty repo.
    let other = tmp.path().join("other");
    let co2 = sync(&url(&bare), None, &other, &auth, MB).unwrap();
    assert_eq!(co2.commit, co.commit);
    assert_eq!(read(other.join("001-a.png")), "a");
}

#[test]
fn unpushed_commit_is_pushed_later() {
    let tmp = tempfile::tempdir().unwrap();
    let (bare, _) = seeded_bare(tmp.path());
    let dest = tmp.path().join("server-checkout");
    let auth = GitAuth::default();
    sync(&url(&bare), None, &dest, &auth, MB).unwrap();

    // Simulate a commit whose push failed: point origin somewhere unreachable.
    let repo = Repository::open(&dest).unwrap();
    repo.remote_set_url("origin", &url(&tmp.path().join("gone.git")))
        .unwrap();
    fs::write(dest.join("006-f.png"), b"f").unwrap();
    assert!(commit_and_push(&dest, "lost push", "stickpile", "bot@example.org", &auth).is_err());

    repo.remote_set_url("origin", &url(&bare)).unwrap();
    let co = commit_and_push(&dest, "retry", "stickpile", "bot@example.org", &auth).unwrap();
    assert!(!co.changed, "nothing new to commit");
    let upstream = Repository::open_bare(&bare).unwrap();
    assert_eq!(
        upstream.refname_to_id("refs/heads/main").unwrap().to_string(),
        co.commit
    );
}

#[test]
fn push_from_a_shallow_checkout() {
    // libgit2's local transport cannot fetch shallow, so make the checkout
    // shallow by hand: the synced tip becomes the shallow boundary, exactly
    // what a depth-1 fetch over https/ssh produces.
    let tmp = tempfile::tempdir().unwrap();
    let (bare, maintainer) = seeded_bare(tmp.path());
    commit(&maintainer, &[("002-b.png", b"b2")], &[], "second");
    push_to(&maintainer, &bare, "main");
    let dest = tmp.path().join("server-checkout");
    let auth = GitAuth::default();
    let synced = sync(&url(&bare), None, &dest, &auth, MB).unwrap();
    fs::write(dest.join(".git/shallow"), format!("{}\n", synced.commit)).unwrap();
    assert!(Repository::open(&dest).unwrap().is_shallow());

    fs::write(dest.join("007-g.png"), b"g").unwrap();
    let co = commit_and_push(&dest, "from shallow", "stickpile", "bot@example.org", &auth).unwrap();
    assert!(co.changed);
    let upstream = Repository::open_bare(&bare).unwrap();
    let tip = upstream
        .find_commit(upstream.refname_to_id("refs/heads/main").unwrap())
        .unwrap();
    assert_eq!(tip.id().to_string(), co.commit);
    assert_eq!(tip.parent_id(0).unwrap().to_string(), synced.commit);
    assert!(tip.tree().unwrap().get_name("007-g.png").is_some());

    // And a later sync of the shallow checkout still works.
    let co2 = sync(&url(&bare), None, &dest, &auth, MB).unwrap();
    assert_eq!(
        co2,
        Checkout {
            commit: co.commit,
            changed: false
        }
    );
}
