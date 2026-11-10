use std::collections::HashSet;

use stickpile_api::{TargetApp, User, Visibility};

use super::*;
use crate::cache::{self, Cache};

fn user(id: &str, name: &str) -> User {
    User {
        id: id.into(),
        username: name.into(),
        display_name: name.into(),
        role: stickpile_api::Role::User,
        totp_enabled: false,
        tailscale_login: None,
        created_at: 0,
    }
}

#[tokio::test]
async fn full_then_incremental_sync_with_pages_and_removals() {
    let (server, items) = seeded_server();
    server.state().items_page_size = 2;
    // A public set of another user is visible; a private one is not.
    {
        let mut st = server.state();
        st.add_user("bob", "pw", stickpile_api::Role::User);
        let pub_set = st.add_set("bob", "frogs", "Frogs", Visibility::Public);
        st.add_item(&pub_set.id, "frog", &["green"], png_bytes(5, 5, 50));
        let priv_set = st.add_set("bob", "secret", "Secret", Visibility::Private);
        st.add_item(&priv_set.id, "hidden", &[], png_bytes(5, 5, 51));
        let unlisted = st.add_set("bob", "link", "Link", Visibility::Unlisted);
        st.add_item(&unlisted.id, "unlisted", &[], png_bytes(5, 5, 52));
    }
    let token = server.state().issue_token("alice", "t");
    let client = client_for(&server.url, Some(token));
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(dir.path(), 16).unwrap();

    let r1 = cache::sync(&cache, &client, 500).await.unwrap();
    assert!(r1.full);
    assert_eq!(r1.pages, 3, "5 visible items in pages of 2");
    assert_eq!(r1.total, 5);
    let names: HashSet<String> = cache.items().unwrap().into_iter().map(|i| i.name).collect();
    assert!(names.contains("frog") && !names.contains("hidden") && !names.contains("unlisted"));
    assert_eq!(cache.sync_cursor().unwrap(), Some(r1.server_time));
    let gen1 = cache.generation().unwrap();

    // Changes: delete one, rename one, add one.
    {
        let mut st = server.state();
        st.delete_item(&items[0].id);
        st.rename_item(&items[1].id, "naahh bro");
        let set_id = items[2].set_id.clone();
        st.add_item(&set_id, "new one", &["fresh"], png_bytes(6, 6, 60));
    }
    let r2 = cache::sync(&cache, &client, 500).await.unwrap();
    assert!(!r2.full);
    assert_eq!(r2.removed, 1);
    assert_eq!(r2.total, 5);
    assert!(cache.item(&items[0].id).unwrap().is_none());
    assert_eq!(cache.item(&items[1].id).unwrap().unwrap().name, "naahh bro");
    assert!(r2.upserted >= 2);
    assert!(cache.generation().unwrap() > gen1);
    let item_requests = server
        .state()
        .requests
        .iter()
        .filter(|r| r.starts_with("GET /api/v1/items"))
        .count();
    assert_eq!(item_requests, 3 + r2.pages);

    // Nothing changed: one cheap page.
    let r3 = cache::sync(&cache, &client, 500).await.unwrap();
    assert_eq!(r3.pages, 1);
    assert_eq!(r3.removed, 0);
}

#[tokio::test]
async fn full_sync_sweeps_items_the_server_no_longer_lists() {
    let (server, items) = seeded_server();
    let client = client_for(&server.url, Some(server.state().issue_token("alice", "t")));
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(dir.path(), 16).unwrap();
    let mut ghost = items[0].clone();
    ghost.id = "ghostghostghostghostghost2".into();
    cache.upsert_items(&[ghost.clone()]).unwrap();
    let r = cache::sync(&cache, &client, 500).await.unwrap();
    assert!(r.full);
    assert_eq!(r.removed, 1);
    assert!(cache.item(&ghost.id).unwrap().is_none());
    assert_eq!(cache.item_count().unwrap(), 4);
}

#[test]
fn server_or_account_change_resets_account_data() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(dir.path(), 16).unwrap();
    let (_server, items) = seeded_server();
    assert!(!cache.bind_server("http://a.example").unwrap());
    cache.upsert_items(&items).unwrap();
    cache.set_sync_cursor(Some(42)).unwrap();
    assert!(!cache.set_me(&user("u1", "alice")).unwrap());
    let ev = crate::usage::copy_event(&items[0], None, None, "test");
    cache.queue_push(&ev).unwrap();
    cache.add_history(&items[0], &ev).unwrap();

    assert!(
        !cache.bind_server("http://a.example/").unwrap(),
        "trailing slash is the same server"
    );
    assert!(!cache.set_me(&user("u1", "alice")).unwrap());
    assert_eq!(cache.item_count().unwrap(), 4);

    assert!(cache.set_me(&user("u2", "bob")).unwrap());
    assert_eq!(cache.item_count().unwrap(), 0);
    assert_eq!(cache.sync_cursor().unwrap(), None);
    assert_eq!(cache.queue_len().unwrap(), 0);
    assert!(cache.recent(10).unwrap().is_empty());

    cache.upsert_items(&items).unwrap();
    assert!(cache.bind_server("http://b.example").unwrap());
    assert_eq!(cache.item_count().unwrap(), 0);
    assert!(cache.me().unwrap().is_none());
}

#[test]
fn lru_evicts_least_recently_used_files() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(dir.path(), 1).unwrap(); // 1 MiB
    let write = |rel: &str, size: usize| {
        let p = cache.abs(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, vec![0u8; size]).unwrap();
    };
    write("files/thumb/a.webp", 400 * 1024);
    cache.register_file("files/thumb/a.webp").unwrap();
    write("clip/m1.original/b.png", 400 * 1024);
    cache.register_file("clip/m1.original/b.png").unwrap();
    // Use `a` so `b` becomes the least recently used.
    assert!(cache.lookup_file("files/thumb/a.webp", None).unwrap().is_some());
    write("files/thumb/c.webp", 400 * 1024);
    let report = cache.register_file("files/thumb/c.webp").unwrap();
    assert_eq!(report.removed_files, 1);
    assert_eq!(report.freed_bytes, 400 * 1024);
    assert!(!cache.abs("clip/m1.original/b.png").exists());
    assert!(!cache.abs("clip/m1.original").exists(), "empty clip dir pruned");
    assert!(cache.abs("files/thumb/a.webp").exists());
    assert!(cache.abs("files/thumb/c.webp").exists());
    assert_eq!(cache.files_total_bytes().unwrap(), 800 * 1024);

    // A single file larger than the cap is kept (it is the one just used).
    write("clip/big.original/big.mp4", 2 * 1024 * 1024);
    cache.register_file("clip/big.original/big.mp4").unwrap();
    assert!(cache.abs("clip/big.original/big.mp4").exists());
    assert!(!cache.abs("files/thumb/a.webp").exists());
    assert!(!cache.abs("files/thumb/c.webp").exists());
}

#[test]
fn lookup_drops_missing_or_wrong_size_files() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(dir.path(), 16).unwrap();
    let p = cache.abs("files/thumb/x.webp");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, b"12345").unwrap();
    cache.register_file("files/thumb/x.webp").unwrap();
    assert!(cache.lookup_file("files/thumb/x.webp", Some(5)).unwrap().is_some());
    assert!(
        cache.lookup_file("files/thumb/x.webp", Some(6)).unwrap().is_none(),
        "partial/stale file"
    );
    assert!(
        cache.lookup_file("files/thumb/x.webp", None).unwrap().is_none(),
        "row dropped"
    );
    std::fs::remove_file(&p).unwrap();
    assert!(cache.lookup_file("files/thumb/x.webp", None).unwrap().is_none());
    assert!(cache.lookup_file("never/registered", None).unwrap().is_none());
}

#[tokio::test]
async fn offline_index_uses_query_syntax_and_target_hints() {
    let (server, items) = seeded_server();
    let client = client_for(&server.url, Some(server.state().issue_token("alice", "t")));
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(dir.path(), 16).unwrap();
    cache.set_me(&client.me().await.unwrap()).unwrap();
    cache::sync(&cache, &client, 500).await.unwrap();
    let idx = cache.offline_index().unwrap();
    assert_eq!(idx.len(), 4);

    let hits = idx.search("cat", None, 10);
    let ids: Vec<&str> = hits.iter().map(|h| h.item.id.as_str()).collect();
    assert!(
        ids.contains(&items[0].id.as_str()) && ids.contains(&items[1].id.as_str()),
        "{ids:?}"
    );
    assert!(!ids.contains(&items[3].id.as_str()));

    let hits = idx.search("set:memes", None, 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].item.name, "this is fine");

    let hits = idx.search("type:gif", None, 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].item.id, items[1].id);

    // WhatsApp accepts no WebP stills and the item has no PNG variant.
    let hits = idx.search("sleepy", Some(TargetApp::Whatsapp), 10);
    assert_eq!(hits[0].item.id, items[2].id);
    assert!(hits[0].incompatible.is_some());
    let hits = idx.search("sleepy", Some(TargetApp::Discord), 10);
    assert!(hits[0].incompatible.is_none());
}
