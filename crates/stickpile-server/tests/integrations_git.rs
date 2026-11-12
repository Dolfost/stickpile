//! Git packs against local `file://` repositories: clone and import,
//! webhooks (secret, GitLab token, HMAC), re-pull, read-only enforcement,
//! push mode, size limit, URL policy, scheduling and checkout cleanup.

mod common;
mod integrations_support;

use std::path::Path;

use common::{png, TestServer, TestUser};
use integrations_support::*;
use reqwest::Method;
use serde_json::{json, Value};
use stickpile_api::{routes, GitPackCreated, Job, JobKind, JobStatus, SetDetail, SetSource, Visibility};
use stickpile_server::service::gitpack;

const MB: usize = 1 << 20;

async fn server() -> TestServer {
    TestServer::with_config(|c| {
        c.git.allowed_schemes = vec!["file".into(), "https".into()];
        c.git.author_name = "stickpile-bot".into();
        c.git.author_email = "bot@stickpile.test".into();
    })
    .await
}

fn manifest(name: &str, version: u32, visibility: &str, cat_keywords: &str) -> String {
    format!(
        "version = {version}\nname = \"{name}\"\nvisibility = \"{visibility}\"\nlicense = \"CC0\"\n\n\
         [items.\"001-cat.png\"]\nname = \"cat\"\nkeywords = [{cat_keywords}]\nemojis = [\"😼\"]\n\n\
         [items.\"002-dog.png\"]\nname = \"dog\"\n"
    )
}

async fn create(srv: &TestServer, u: &TestUser, body: Value) -> GitPackCreated {
    srv.post(u, routes::SETS_GITPACK, body).await.expect(201).json()
}

async fn detail(srv: &TestServer, u: &TestUser, id: &str) -> SetDetail {
    srv.get(u, &routes::path(routes::SET, &[id])).await.expect(200).json()
}

async fn job(srv: &TestServer, u: &TestUser, id: &str) -> Job {
    srv.get(u, &routes::path(routes::JOB, &[id])).await.expect(200).json()
}

async fn hook(srv: &TestServer, set_id: &str, headers: &[(&str, String)], body: &str) -> common::Resp {
    let mut rb = srv
        .client
        .post(srv.url(&routes::path(routes::GIT_WEBHOOK, &[set_id])))
        .header("content-type", "application/json")
        .body(body.to_string());
    for (k, v) in headers {
        rb = rb.header(*k, v);
    }
    common::read(rb.send().await.unwrap()).await
}

fn names(d: &SetDetail) -> Vec<String> {
    d.items.iter().map(|i| i.name.clone()).collect()
}

#[tokio::test]
async fn clone_import_webhooks_and_repull() {
    let srv = server().await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let tmp = tempfile::tempdir().unwrap();
    let up = work_repo(&tmp.path().join("upstream"));
    let c1 = commit(
        &up,
        &[
            ("README.md", b"cats" as &[u8]),
            (
                "packs/cats/set.toml",
                manifest("Cats", 3, "public", "\"cat\"").as_bytes(),
            ),
            ("packs/cats/001-cat.png", &png(1)),
            ("packs/cats/002-dog.png", &png(2)),
        ],
        &[],
        "first",
    );
    let url = file_url(up.workdir().unwrap());

    let created = create(
        &srv,
        &alice,
        json!({"url": url, "subdir": "/packs/cats/", "mode": "read_only", "branch": null, "slug": null}),
    )
    .await;
    let set_id = created.set.id.clone();
    assert_eq!(created.set.slug, "cats", "slug from the subdir");
    assert_eq!(created.set.source, SetSource::Git);
    assert!(!created.set.editable);
    assert_eq!(created.job.kind, JobKind::GitPull);
    assert!(created.webhook_url.ends_with(&format!("/api/v1/hooks/git/{set_id}")));
    assert!(created.webhook_url.starts_with(&srv.base));
    assert!(created.webhook_secret.len() >= 32);
    let secret = created.webhook_secret.clone();

    assert_eq!(run_jobs_now(&srv).await, 3, "pull + 2 variants");
    let j = job(&srv, &alice, &created.job.id).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    assert_eq!(j.result.as_ref().unwrap()["created"], 2);
    let d = detail(&srv, &alice, &set_id).await;
    assert_eq!(d.set.name, "Cats");
    assert_eq!(d.set.visibility, Visibility::Public);
    assert_eq!(d.set.version, 3, "upstream version");
    assert_eq!(names(&d), ["cat", "dog"]);
    assert_eq!(d.items[0].keywords, ["cat"]);
    let git = d.set.git.clone().unwrap();
    assert_eq!(git.last_commit.as_deref(), Some(c1.as_str()));
    assert!(git.last_pull_at.is_some() && git.last_error.is_none());
    assert_eq!(git.web_url, None, "file:// has no web page");
    assert!(srv.state.config.gitpacks_dir().join(&set_id).join(".git").is_dir());

    // Upstream: new item, changed keywords, version bump.
    let c2 = commit(
        &up,
        &[
            (
                "packs/cats/set.toml",
                manifest("Cats", 7, "public", "\"cat\", \"stare\"").as_bytes(),
            ),
            ("packs/cats/003-cow.png", &png(3)),
        ],
        &[],
        "second",
    );
    // Webhook: unknown set, missing credentials, wrong secret, bad HMAC.
    hook(
        &srv,
        "aaaaaaaaaaaaaaaaaaaaaaaaaa",
        &[("x-stickpile-webhook-secret", secret.clone())],
        "{}",
    )
    .await
    .expect(404);
    hook(&srv, &set_id, &[], "{}").await.expect(401);
    hook(&srv, &set_id, &[("x-stickpile-webhook-secret", "nope".into())], "{}")
        .await
        .expect(403);
    let body = r#"{"ref":"refs/heads/main"}"#;
    let wrong = format!("sha256={}", hmac_sha256_hex(b"other secret", body.as_bytes()));
    hook(&srv, &set_id, &[("x-hub-signature-256", wrong)], body)
        .await
        .expect(403);
    // A signature over a different body does not verify either.
    let sig = format!("sha256={}", hmac_sha256_hex(secret.as_bytes(), body.as_bytes()));
    hook(&srv, &set_id, &[("x-hub-signature-256", sig.clone())], "{}")
        .await
        .expect(403);
    // The secret header works; a second call while queued reuses the job.
    let j1: Job = hook(&srv, &set_id, &[("x-stickpile-webhook-secret", secret.clone())], "{}")
        .await
        .expect(202)
        .json();
    let j2: Job = hook(&srv, &set_id, &[("x-gitlab-token", secret.clone())], "{}")
        .await
        .expect(202)
        .json();
    assert_eq!(j1.id, j2.id, "a queued pull is not duplicated");
    run_jobs_now(&srv).await;
    let d = detail(&srv, &alice, &set_id).await;
    assert_eq!(names(&d), ["cat", "dog", "cow"]);
    assert_eq!(d.items[0].keywords, ["cat", "stare"]);
    assert_eq!(d.set.version, 7, "max(stored + 1, upstream)");
    assert_eq!(d.set.git.as_ref().unwrap().last_commit.as_deref(), Some(c2.as_str()));

    // HMAC variant: upstream removes an item.
    let c3 = commit(&up, &[], &["packs/cats/002-dog.png"], "third");
    let j3: Job = hook(&srv, &set_id, &[("x-hub-signature-256", sig)], body)
        .await
        .expect(202)
        .json();
    assert_ne!(j3.id, j1.id);
    run_jobs_now(&srv).await;
    let r = job(&srv, &alice, &j3.id).await;
    assert_eq!(r.result.as_ref().unwrap()["removed"], 1, "{r:?}");
    let d = detail(&srv, &alice, &set_id).await;
    assert_eq!(names(&d), ["cat", "cow"]);
    assert_eq!(d.set.version, 8);
    assert_eq!(d.set.git.as_ref().unwrap().last_commit.as_deref(), Some(c3.as_str()));
    // Unknown keys survive the import (exported again).
    let tar = srv
        .get(&alice, &routes::path(routes::SET_EXPORT, &[&set_id]))
        .await
        .expect(200);
    let bundle = stickpile_setfmt::read_tar(&tar.body, 1 << 30).unwrap();
    assert_eq!(bundle.manifest.extra["license"].as_str(), Some("CC0"));

    // Pull now with nothing new upstream: nothing to import.
    let p: Job = srv
        .post(&alice, &routes::path(routes::SET_PULL, &[&set_id]), json!({}))
        .await
        .expect(200)
        .json();
    run_jobs_now(&srv).await;
    let p = job(&srv, &alice, &p.id).await;
    assert_eq!(p.result.unwrap()["changed"], false);
    assert_eq!(detail(&srv, &alice, &set_id).await.set.version, 8);

    // Others: may read the public pack, but neither pull nor edit it.
    srv.post(&bob, &routes::path(routes::SET_PULL, &[&set_id]), json!({}))
        .await
        .expect(403);
    srv.patch(&bob, &routes::path(routes::SET, &[&set_id]), json!({"name": "x"}))
        .await
        .expect(403);
    let anon = srv
        .send(
            None,
            Method::POST,
            &routes::path(routes::SET_PULL, &[&set_id]),
            Some(json!({})),
        )
        .await;
    assert_eq!(anon.status, 401);
    // A job of alice is not visible to bob.
    srv.get(&bob, &routes::path(routes::JOB, &[&p.id])).await.expect(404);
}

#[tokio::test]
async fn read_only_packs_reject_every_edit() {
    let srv = server().await;
    let alice = srv.user("alice").await;
    let tmp = tempfile::tempdir().unwrap();
    let up = work_repo(&tmp.path().join("up"));
    commit(
        &up,
        &[
            ("set.toml", manifest("Pack", 1, "private", "\"cat\"").as_bytes()),
            ("001-cat.png", &png(10)),
            ("002-dog.png", &png(11)),
        ],
        &[],
        "init",
    );
    let created = create(
        &srv,
        &alice,
        json!({"url": file_url(up.workdir().unwrap()), "mode": "read_only", "slug": "pack"}),
    )
    .await;
    let id = created.set.id;
    run_jobs_now(&srv).await;
    let d = detail(&srv, &alice, &id).await;
    assert_eq!(d.items.len(), 2);
    assert!(!d.set.editable);
    let item = d.items[0].id.clone();
    let other = srv.create_set(&alice, "mine", Visibility::Private).await;
    let mine = srv.upload_one(&alice, &other.id, "x.png", png(12)).await;

    let forbidden = |r: common::Resp, what: &str| {
        assert_eq!(r.status, 403, "{what}: {}", r.text());
        assert!(r.text().contains("upstream"), "{what}: {}", r.text());
    };
    forbidden(
        srv.patch(&alice, &routes::path(routes::SET, &[&id]), json!({"name": "x"}))
            .await,
        "set PATCH",
    );
    forbidden(
        srv.patch(
            &alice,
            &routes::path(routes::ITEM, &[&item]),
            json!({"keywords": ["x"]}),
        )
        .await,
        "item PATCH",
    );
    forbidden(
        srv.delete(&alice, &routes::path(routes::ITEM, &[&item])).await,
        "item DELETE",
    );
    forbidden(srv.upload(&alice, &id, vec![("n.png", png(13))], None).await, "upload");
    forbidden(srv.put_cover(&alice, &id, "c.png", png(15)).await, "cover image");
    forbidden(
        srv.delete(&alice, &routes::path(routes::SET_COVER, &[&id])).await,
        "cover image DELETE",
    );
    let order: Vec<String> = d.items.iter().rev().map(|i| i.id.clone()).collect();
    forbidden(
        srv.put(
            &alice,
            &routes::path(routes::SET_ORDER, &[&id]),
            json!({"item_ids": order}),
        )
        .await,
        "reorder",
    );
    // Moving an item in or out.
    forbidden(
        srv.patch(&alice, &routes::path(routes::ITEM, &[&mine.id]), json!({"set_id": id}))
            .await,
        "move in",
    );
    forbidden(
        srv.patch(
            &alice,
            &routes::path(routes::ITEM, &[&item]),
            json!({"set_id": other.id}),
        )
        .await,
        "move out",
    );
    // Import into.
    let form = reqwest::multipart::Form::new()
        .part(
            "set.toml",
            reqwest::multipart::Part::text("name = \"x\"\n").file_name("set.toml"),
        )
        .part(
            "file",
            reqwest::multipart::Part::bytes(png(14)).file_name("x/001-a.png"),
        );
    let r = common::read(
        srv.client
            .post(srv.url(&format!("{}?into={id}", routes::SETS_IMPORT)))
            .bearer_auth(&alice.token)
            .multipart(form)
            .send()
            .await
            .unwrap(),
    )
    .await;
    forbidden(r, "import into");
    // Still unchanged, and nothing queued to push.
    let after = detail(&srv, &alice, &id).await;
    assert_eq!(after.set.version, d.set.version);
    assert_eq!(names(&after), names(&d));
    let pushes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = 'git_push'")
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(pushes, 0);
    // The owner may still unsubscribe; the checkout is cleaned up later.
    srv.delete(&alice, &routes::path(routes::SET, &[&id])).await.expect(204);
    let checkout = srv.state.config.gitpacks_dir().join(&id);
    assert!(checkout.is_dir());
    assert_eq!(gitpack::gc_checkouts(&srv.state).await.unwrap(), 1);
    assert!(!checkout.exists());
}

#[tokio::test]
async fn push_mode_commits_edits_upstream() {
    let srv = server().await;
    let alice = srv.user("alice").await;
    let tmp = tempfile::tempdir().unwrap();
    let bare = tmp.path().join("pack.git");
    bare_repo(&bare);
    let maint = work_repo(&tmp.path().join("maintainer"));
    commit(
        &maint,
        &[
            ("README.md", b"# my pack" as &[u8]),
            ("set.toml", manifest("Pushy", 2, "private", "\"cat\"").as_bytes()),
            ("001-cat.png", &png(20)),
            ("002-dog.png", &png(21)),
        ],
        &[],
        "init",
    );
    push_main(&maint, &bare);

    let created = create(&srv, &alice, json!({"url": file_url(&bare), "mode": "push"})).await;
    let id = created.set.id.clone();
    assert_eq!(created.set.slug, "pack", "slug from the repository name");
    run_jobs_now(&srv).await;
    let d = detail(&srv, &alice, &id).await;
    assert!(d.set.editable);
    assert_eq!(names(&d), ["cat", "dog"]);
    let (imported, _) = tip(&bare, "main");
    assert_eq!(
        d.set.git.as_ref().unwrap().last_commit.as_deref(),
        Some(imported.as_str())
    );
    // The import itself is not pushed back.
    assert_eq!(tip(&bare, "main").1, "Maintainer");

    // Edit in stickpile -> debounced push job -> commit upstream.
    srv.patch(
        &alice,
        &routes::path(routes::ITEM, &[&d.items[0].id]),
        json!({"keywords": ["cat", "grumpy"]}),
    )
    .await
    .expect(200);
    let queued: (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), MIN(run_after - created_at) FROM jobs WHERE kind = 'git_push' AND status = 'queued'",
    )
    .fetch_one(&srv.state.db)
    .await
    .unwrap();
    assert_eq!(queued.0, 1);
    assert!(queued.1 >= gitpack::PUSH_DEBOUNCE_SECS, "debounced");
    // More edits move the same job instead of queueing more.
    srv.upload_one(&alice, &id, "003-cow.png", png(22)).await;
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = 'git_push' AND status = 'queued'")
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(n, 1);
    run_jobs_now(&srv).await;
    let (head, author) = tip(&bare, "main");
    assert_eq!(author, "stickpile-bot");
    let files = tree_files(&bare, "main");
    assert_eq!(files["README.md"], b"# my pack", "non-media files are kept");
    assert!(files.contains_key("003-cow.png"), "{:?}", files.keys());
    let m = stickpile_setfmt::Manifest::parse(std::str::from_utf8(&files["set.toml"]).unwrap()).unwrap();
    assert_eq!(m.items["001-cat.png"].keywords, ["cat", "grumpy"]);
    assert_eq!(m.extra["license"].as_str(), Some("CC0"));
    let d = detail(&srv, &alice, &id).await;
    assert_eq!(m.version, d.set.version, "the export carries the server's version");
    let git = d.set.git.clone().unwrap();
    assert_eq!(git.last_commit.as_deref(), Some(head.as_str()));
    assert!(git.last_error.is_none());

    // Upstream change while nothing is pending: imported, nothing pushed back.
    pull_main(&maint, &bare);
    let text = std::str::from_utf8(&files["set.toml"])
        .unwrap()
        .replace("\"grumpy\"", "\"angry\"");
    commit(&maint, &[("set.toml", text.as_bytes())], &[], "maintainer edit");
    push_main(&maint, &bare);
    let (upstream_head, _) = tip(&bare, "main");
    srv.post(&alice, &routes::path(routes::SET_PULL, &[&id]), json!({}))
        .await
        .expect(200);
    run_jobs_now(&srv).await;
    let d = detail(&srv, &alice, &id).await;
    assert_eq!(d.items[0].keywords, ["cat", "angry"]);
    assert_eq!(tip(&bare, "main").0, upstream_head, "no echo commit after a pull");

    // Conflict: a local edit is pending when upstream moves; the pull pushes
    // the local state (stickpile wins, upstream's change stays in history).
    srv.patch(
        &alice,
        &routes::path(routes::SET, &[&id]),
        json!({"description": "local"}),
    )
    .await
    .expect(200);
    pull_main(&maint, &bare);
    commit(
        &maint,
        &[("NOTES.md", b"upstream notes" as &[u8])],
        &[],
        "upstream notes",
    );
    push_main(&maint, &bare);
    let res = gitpack::run_pull(&srv.state, &id).await.unwrap().unwrap();
    assert_eq!(res["pushed"], true, "{res}");
    let files = tree_files(&bare, "main");
    assert!(files.contains_key("NOTES.md"), "upstream's other files survive");
    let m = stickpile_setfmt::Manifest::parse(std::str::from_utf8(&files["set.toml"]).unwrap()).unwrap();
    assert_eq!(m.description, "local");
    run_jobs_now(&srv).await; // the debounced push finds nothing left to do
    let r: Value = sqlx::query_scalar::<_, String>(
        "SELECT result FROM jobs WHERE kind = 'git_push' ORDER BY created_at DESC, rowid DESC LIMIT 1",
    )
    .fetch_one(&srv.state.db)
    .await
    .map(|s| serde_json::from_str(&s).unwrap())
    .unwrap();
    assert_eq!(r["skipped"], "nothing to push");
}

#[tokio::test]
async fn git_packs_carry_the_cover_image() {
    let srv = server().await;
    let alice = srv.user("alice").await;
    let tmp = tempfile::tempdir().unwrap();
    let bare = tmp.path().join("covered.git");
    bare_repo(&bare);
    let maint = work_repo(&tmp.path().join("maintainer"));
    let with_cover = manifest("Covered", 1, "private", "\"cat\"").replace(
        "license = \"CC0\"\n",
        "license = \"CC0\"\ncover_image = \"cover.png\"\n",
    );
    commit(
        &maint,
        &[
            ("set.toml", with_cover.as_bytes()),
            ("001-cat.png", &png(40)),
            ("002-dog.png", &png(41)),
            ("cover.png", &png(42)),
        ],
        &[],
        "init",
    );
    push_main(&maint, &bare);
    let created = create(&srv, &alice, json!({"url": file_url(&bare), "mode": "push"})).await;
    let id = created.set.id.clone();
    run_jobs_now(&srv).await;
    let d = detail(&srv, &alice, &id).await;
    assert_eq!(names(&d), ["cat", "dog"], "the cover image is not an item");
    let first = d.set.cover_image.clone().expect("cover image from set.toml");

    // A new image is pushed as cover.png; set.toml names it.
    srv.put_cover(&alice, &id, "new.png", png(43)).await.expect(200);
    run_jobs_now(&srv).await;
    let files = tree_files(&bare, "main");
    let m = stickpile_setfmt::Manifest::parse(std::str::from_utf8(&files["set.toml"]).unwrap()).unwrap();
    assert_eq!(m.cover_image.as_deref(), Some("cover.png"));
    assert_ne!(files["cover.png"], png(42));
    assert!(!m.items.contains_key("cover.png"));
    // Removed: the file goes too.
    srv.delete(&alice, &routes::path(routes::SET_COVER, &[&id]))
        .await
        .expect(200);
    run_jobs_now(&srv).await;
    let files = tree_files(&bare, "main");
    assert!(!files.contains_key("cover.png"), "{:?}", files.keys());
    let m = stickpile_setfmt::Manifest::parse(std::str::from_utf8(&files["set.toml"]).unwrap()).unwrap();
    assert_eq!(m.cover_image, None);

    // Upstream brings one back with the next pull.
    pull_main(&maint, &bare);
    let text = std::str::from_utf8(&files["set.toml"]).unwrap().replacen(
        "\n[items.",
        "cover_image = \"cover.png\"\n\n[items.",
        1,
    );
    commit(
        &maint,
        &[("set.toml", text.as_bytes()), ("cover.png", &png(42))],
        &[],
        "cover again",
    );
    push_main(&maint, &bare);
    srv.post(&alice, &routes::path(routes::SET_PULL, &[&id]), json!({}))
        .await
        .expect(200);
    run_jobs_now(&srv).await;
    let d = detail(&srv, &alice, &id).await;
    assert_eq!(names(&d), ["cat", "dog"]);
    assert_eq!(d.set.cover_image.map(|c| c.sha256), Some(first.sha256));
}

#[tokio::test]
async fn push_mode_into_an_empty_repository_and_failed_pushes() {
    let srv = server().await;
    let alice = srv.user("alice").await;
    let tmp = tempfile::tempdir().unwrap();
    let bare = tmp.path().join("empty.git");
    bare_repo(&bare);
    let created = create(
        &srv,
        &alice,
        json!({"url": file_url(&bare), "mode": "push", "slug": "fresh"}),
    )
    .await;
    let id = created.set.id;
    run_jobs_now(&srv).await;
    let j = job(&srv, &alice, &created.job.id).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    assert_eq!(j.result.unwrap()["empty"], true);
    srv.upload_one(&alice, &id, "first.png", png(30)).await;
    run_jobs_now(&srv).await;
    let files = tree_files(&bare, "main");
    assert!(
        files.contains_key("set.toml") && files.contains_key("001-first.png"),
        "{:?}",
        files.keys()
    );

    // The remote becomes unwritable: the push is retried once, then fails
    // with the error recorded on the set.
    let lock_down = |mode: u32| {
        for dir in ["objects", "refs"] {
            chmod_dirs(&bare.join(dir), mode);
        }
    };
    lock_down(0o555);
    if std::fs::write(bare.join("objects/probe"), b"x").is_ok() {
        // Running as root: permissions do not apply; skip this part.
        std::fs::remove_file(bare.join("objects/probe")).unwrap();
        lock_down(0o755);
        return;
    }
    srv.upload_one(&alice, &id, "second.png", png(31)).await;
    run_jobs_now(&srv).await;
    lock_down(0o755);
    let failed: (String, String) = sqlx::query_as(
        "SELECT status, error FROM jobs WHERE kind = 'git_push' ORDER BY created_at DESC, rowid DESC LIMIT 1",
    )
    .fetch_one(&srv.state.db)
    .await
    .unwrap();
    assert_eq!(failed.0, "failed");
    assert!(failed.1.starts_with("push failed"), "{}", failed.1);
    let d = detail(&srv, &alice, &id).await;
    assert!(d.set.git.unwrap().last_error.unwrap().starts_with("push failed"));
    // The next scheduled pull notices the unpushed edit and pushes it.
    gitpack::run_pull(&srv.state, &id).await.unwrap();
    assert!(tree_files(&bare, "main").contains_key("002-second.png"));
    assert!(detail(&srv, &alice, &id).await.set.git.unwrap().last_error.is_none());
}

#[tokio::test]
async fn url_policy_size_limit_and_disabled() {
    // Default policy: https only.
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let tmp = tempfile::tempdir().unwrap();
    let up = work_repo(&tmp.path().join("up"));
    commit(&up, &[("001-a.png", &png(40))], &[], "x");
    for (url, why) in [
        (file_url(up.workdir().unwrap()), "scheme"),
        ("https://user:token@example.org/a/b.git".to_string(), "credentials"),
        ("ext::sh -c touch% /tmp/pwned".to_string(), "transport"),
        ("/srv/repo".to_string(), "local path"),
        ("https://-oProxyCommand=x/a.git".to_string(), "option"),
    ] {
        let r = srv
            .post(&alice, routes::SETS_GITPACK, json!({"url": url, "mode": "read_only"}))
            .await
            .expect(400);
        assert!(r.text().contains("URL not allowed"), "{why}: {}", r.text());
        assert!(!r.text().contains("token@"), "credentials are redacted");
    }
    for (body, why) in [
        (
            json!({"url": "https://example.org/a.git", "mode": "push", "subdir": "../etc"}),
            "subdir",
        ),
        (
            json!({"url": "https://example.org/a.git", "mode": "push", "subdir": ".git"}),
            "subdir .git",
        ),
        (
            json!({"url": "https://example.org/a.git", "mode": "push", "branch": "--upload-pack=x"}),
            "branch",
        ),
        (
            json!({"url": "https://example.org/a.git", "mode": "push", "slug": "Not A Slug"}),
            "slug",
        ),
    ] {
        srv.post(&alice, routes::SETS_GITPACK, body).await.expect(400);
        let _ = why;
    }
    assert_eq!(
        srv.get(&alice, &routes::path(routes::SETS, &[]))
            .await
            .json::<Vec<Value>>()
            .len(),
        0
    );

    // Size limit: 1 MiB, repository with a 2 MiB blob.
    let srv = TestServer::with_config(|c| {
        c.git.allowed_schemes = vec!["file".into()];
        c.git.max_repo_mb = 1;
    })
    .await;
    let alice = srv.user("alice").await;
    let big: Vec<u8> = (0..2 * MB).map(|i| (i * 7919 % 251) as u8).collect();
    let up = work_repo(&tmp.path().join("big"));
    commit(&up, &[("001-a.png", &png(41)), ("blob.bin", &big)], &[], "big");
    let created = create(
        &srv,
        &alice,
        json!({"url": file_url(up.workdir().unwrap()), "mode": "read_only"}),
    )
    .await;
    run_jobs_now(&srv).await;
    let j = job(&srv, &alice, &created.job.id).await;
    assert_eq!(j.status, JobStatus::Failed);
    assert_eq!(j.attempts, 1, "a size violation is not retried");
    assert!(j.error.as_deref().unwrap().contains("size limit"), "{j:?}");
    let d = detail(&srv, &alice, &created.set.id).await;
    assert!(d.set.git.unwrap().last_error.unwrap().contains("size limit"));
    assert!(d.items.is_empty());
    assert!(!srv.state.config.gitpacks_dir().join(&created.set.id).exists());

    // git.enabled = false: everything git is "not found".
    let srv = TestServer::with_config(|c| c.git.enabled = false).await;
    let alice = srv.user("alice").await;
    let r = srv
        .post(
            &alice,
            routes::SETS_GITPACK,
            json!({"url": "https://example.org/a.git", "mode": "read_only"}),
        )
        .await
        .expect(404);
    assert!(r.text().contains("disabled"));
    let s = srv.create_set(&alice, "s", Visibility::Private).await;
    srv.post(&alice, &routes::path(routes::SET_PULL, &[&s.id]), json!({}))
        .await
        .expect(404);
    hook(&srv, &s.id, &[("x-stickpile-webhook-secret", "x".into())], "{}")
        .await
        .expect(404);
    assert_eq!(gitpack::schedule_due(&srv.state).await.unwrap(), 0);
}

#[tokio::test]
async fn scheduler_queues_due_pulls_once() {
    let srv = TestServer::with_config(|c| {
        c.git.allowed_schemes = vec!["file".into()];
        c.git.pull_interval_minutes = 30;
    })
    .await;
    let alice = srv.user("alice").await;
    let tmp = tempfile::tempdir().unwrap();
    let up = work_repo(&tmp.path().join("up"));
    commit(&up, &[("001-a.png", &png(50))], &[], "x");
    let created = create(
        &srv,
        &alice,
        json!({"url": file_url(up.workdir().unwrap()), "mode": "read_only"}),
    )
    .await;
    // The first pull is already queued: nothing more.
    assert_eq!(gitpack::schedule_due(&srv.state).await.unwrap(), 0);
    run_jobs_now(&srv).await;
    // Pulled just now: not due.
    assert_eq!(gitpack::schedule_due(&srv.state).await.unwrap(), 0);
    sqlx::query("UPDATE sets SET git_last_pull_at = git_last_pull_at - 31 * 60 WHERE id = ?")
        .bind(&created.set.id)
        .execute(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(gitpack::schedule_due(&srv.state).await.unwrap(), 1);
    assert_eq!(gitpack::schedule_due(&srv.state).await.unwrap(), 0, "no duplicates");
    run_jobs_now(&srv).await;
    assert_eq!(gitpack::schedule_due(&srv.state).await.unwrap(), 0);
}

#[tokio::test]
async fn webhooks_are_rate_limited() {
    let srv = server().await;
    let fake = "aaaaaaaaaaaaaaaaaaaaaaaaaa";
    let quota = stickpile_server::security::WEBHOOKS_PER_MINUTE;
    // The bucket refills one call per second, so a slow machine may get a
    // few more through; the limit must still hit soon after the quota.
    let mut passed = 0;
    let mut limited = false;
    for _ in 0..quota + 30 {
        let r = hook(&srv, fake, &[], "{}").await;
        if r.status == 429 {
            assert!(r.header("retry-after").is_some());
            limited = true;
            break;
        }
        assert_eq!(r.status, 404);
        passed += 1;
    }
    assert!(limited, "never rate limited");
    assert!(passed >= quota, "limited after {passed} calls");
}

/// Set the mode of `dir` and every directory below it.
fn chmod_dirs(dir: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    // Children first while the parent is still writable/listable.
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            chmod_dirs(&entry.path(), mode);
        }
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode)).unwrap();
}
