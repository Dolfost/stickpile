//! Independent review of the text-search round: the Android `hints()` and
//! `suggestions()` for `x:` against what the server's suggest endpoint
//! computes (`suggest_with` over an index of every user's items as the
//! viewer searches it, `ViewedIndex`), on the same items.

use std::collections::HashMap;
use std::fmt::Write as _;

use stickpile_api::{FieldSource, Visibility};
use stickpile_core::cache::doc_for;
use stickpile_quick::suggest::{self, Vocabulary};
use stickpile_search::{Index, ViewedIndex, Viewer};

use super::*;

#[tokio::test]
async fn review_ts_android_hints_agree_with_the_server_offers() {
    let server = MockServer::start();
    let mut vis: HashMap<String, Visibility> = HashMap::new();
    {
        let mut st = server.state();
        st.add_user("bob", "hunter2", stickpile_api::Role::User);
        let sets = [
            st.add_set("alice", "mine", "Mine", Visibility::Private),
            st.add_set("alice", "apub", "Apub", Visibility::Public),
            st.add_set("bob", "bobs", "Bobs", Visibility::Public),
            st.add_set("bob", "bsecret", "Bsecret", Visibility::Private),
            st.add_set("bob", "bunl", "Bunl", Visibility::Unlisted),
        ];
        for s in &sets {
            vis.insert(s.id.clone(), s.visibility);
        }
        let texts: &[(usize, &str)] = &[
            (0, "LOL"),
            (0, "lol what"),
            (0, "what is love"),
            (0, "Literally me"),
            (0, "hello there"),
            (0, "NO THOUGHTS\nHEAD EMPTY"),
            (0, "КОЛИВ ДОМА"),
            (0, "коли ти вдома"),
            (0, "П'ЯТНИЦЯ"),
            (0, "п\u{2019}ятниця вечір"),
            (0, "п`ятниця"),
            (0, "Їжак ґанок Єнот"),
            (0, "404 not found 2024"),
            (0, "1st place"),
            (0, "lmao lmao lmao"),
            (0, "lmfao"),
            (0, "lmfao x"),
            (0, "glorp"),
            (0, "glorb"),
            (1, "glorb"),
            (1, "no thoughts"),
            (2, "quack кря lol"),
            (3, "zyzzyva lol lol секретик"),
            (3, "glorp hidden"),
            (3, "glorp hidden"),
            (3, "glorp hidden"),
            (4, "quokka unlisted"),
        ];
        for (n, (set, text)) in texts.iter().enumerate() {
            let it = st.add_item(&sets[*set].id, &format!("item {n}"), &[], png_bytes(8, 8, n as u8));
            st.set_text(&it.id, text, FieldSource::User);
        }
    }
    // The server's side: every item, each with its set's visibility, as
    // alice searches them.
    let mut index = Index::new();
    {
        let st = server.state();
        for it in &st.items {
            let summary = st.summary(&it.item);
            let mut doc = doc_for(&summary, Some("alice"));
            doc.visibility = vis[&summary.set_id];
            index.upsert(doc);
        }
    }
    let viewer = Viewer {
        user_id: "alice".into(),
        ..Viewer::default()
    };
    let viewed = ViewedIndex {
        index: &index,
        viewer: &viewer,
    };

    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    lib.sync_now().await.unwrap();
    let mut table = String::from(
        "| typed | server (suggest_with) | Android hints | Android suggestions | agree |\n|---|---|---|---|---|\n",
    );
    let mut problems = Vec::new();
    for p in [
        "x:",
        "x:l",
        "x:lo",
        "x:lol",
        "x:LOL",
        "x:lool",
        "x:ко",
        "x:п'ят",
        "x:П’ЯТНИЦЯ",
        "-x:",
        "text:",
        "cat -x:lo",
        "x:оли",
        "x:lm",
        "x:glo",
        "x:2",
        "x:1",
        "x:zy",
        "x:qu",
        "x:hlelo",
        "x:thoguhts",
        "x:ґ",
        "x:a-b",
        "x:😂",
    ] {
        let srv = suggest::suggest_with(p, &Vocabulary::default(), Some(&viewed));
        let show = |words: &[String], chosen: Option<usize>| -> String {
            if words.is_empty() {
                return "none".into();
            }
            words
                .iter()
                .enumerate()
                .map(|(i, w)| if chosen == Some(i) { format!("*{w}") } else { w.clone() })
                .collect::<Vec<_>>()
                .join(" ")
        };
        let s = srv.as_ref().map_or("none".into(), |s| show(&s.words, s.chosen()));
        let hints = lib.hints(p.into());
        let hw: Vec<String> = hints.iter().map(|h| h.label.clone()).collect();
        let hc = hints.iter().position(|h| h.chosen);
        let h = show(&hw, hc);
        let sg = lib.suggestions(p.into());
        let g = sg
            .as_ref()
            .map_or("none".into(), |s| show(&s.words, s.chosen.map(|c| c as usize)));
        let agree = s == h && h == g;
        if !agree {
            problems.push(format!("{p:?}: server {s} / hints {h} / suggestions {g}"));
        }
        // A hint puts the word in with a space, the key as typed.
        if let Some(srv) = &srv {
            for (hint, word) in hints.iter().zip(&srv.words) {
                let want = format!("{}{word} ", &p[..srv.start]);
                if hint.query != want {
                    problems.push(format!("{p:?}: hint query {:?}, want {want:?}", hint.query));
                }
            }
        }
        writeln!(
            table,
            "| `{p}` | {s} | {h} | {g} | {} |",
            if agree { "yes" } else { "NO" }
        )
        .unwrap();
    }
    println!("{table}");
    std::fs::write("/data/stickpile/.claude/scratch/review-ts/android_table.md", &table).ok();
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
