//! The offers under the search field per keystroke on 50,000 cached
//! items (rule 8 of the text-search round), through
//! [`StickpileLibrary::hints`] as the app and the keyboard call it.
//! `cargo test -p stickpile-android --release --test library -- --ignored --nocapture perf_hints`

use std::time::Instant;

use stickpile_api::{ItemSummary, Role, User};
use stickpile_core::cache::Cache;
use stickpile_core::mock::dead_url;
use stickpile_search::synthetic;

use crate::{dirs, open};

#[test]
#[ignore]
fn perf_hints() {
    let dir = tempfile::tempdir().unwrap();
    let url = dead_url();
    let items: Vec<ItemSummary> = synthetic::library(50_000, 42).iter().map(synthetic::summary).collect();
    {
        let cache = Cache::open(std::path::Path::new(&dirs(dir.path()).cache_dir), 2048).unwrap();
        cache.bind_server(&url).unwrap();
        cache
            .set_me(&User {
                id: "user0".into(),
                username: "User0".into(),
                display_name: "User0".into(),
                role: Role::User,
                totp_enabled: false,
                tailscale_login: None,
                created_at: 0,
                telegram_user_id: None,
                theme: Default::default(),
                language: None,
            })
            .unwrap();
        cache.upsert_items(&items).unwrap();
    }
    let lib = open(dir.path(), &url);
    let start = Instant::now();
    let first = lib.hints("vibe:".into());
    println!(
        "first hints (cache index and words built): {:.2?}, {} offers",
        start.elapsed(),
        first.len()
    );
    for typed in [
        "vibe:",
        "vibe:sa",
        "t:",
        "t:ko",
        "e:",
        "e:sk",
        "x:",
        "x:l",
        "x:lo",
        "x:lol",
        "x:lool",
        "x:ко",
        "x:оли",
        "x:п'ят",
        "lo",
    ] {
        let runs = 20;
        let start = Instant::now();
        let mut hints = Vec::new();
        for _ in 0..runs {
            hints = lib.hints(typed.into());
        }
        let per = start.elapsed() / runs;
        let labels: Vec<&str> = hints.iter().map(|h| h.label.as_str()).collect();
        println!("  {typed:<10} {per:>10.2?}  {}", labels.join(" | "));
    }
}
