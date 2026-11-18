//! Review of the text-search round: the words `x:` offers
//! (`Index::text_words`) per call on 50,000 docs, for a viewer who can
//! search almost none of them (a new account on a server without public
//! packs), the case where the counting cannot stop early.
//! `cargo test -p stickpile-search --release --features synthetic --test review_ts_perf -- --ignored --nocapture`
#![cfg(feature = "synthetic")]

use std::time::Instant;

use stickpile_api::Visibility;
use stickpile_search::{Index, Query, SearchOptions, Viewer};

#[test]
#[ignore]
fn review_ts_text_words_for_a_viewer_who_sees_few_items() {
    let mut docs = stickpile_search::synthetic::library(50_000, 42);
    for (i, d) in docs.iter_mut().enumerate() {
        d.visibility = Visibility::Private;
        if i < 10 {
            d.owner_id = "newbie".into();
        }
    }
    let mut ix = Index::new();
    for d in docs {
        ix.upsert(d);
    }
    for (who, sees) in [("newbie", "10 of 50k"), ("user0", "own only, no public")] {
        let v = Viewer {
            user_id: who.into(),
            ..Viewer::default()
        };
        println!("viewer {who} ({sees}):");
        for typed in ["", "l", "lo", "lol", "lool", "ко", "оли", "zzzz", "thoguhts"] {
            let runs = 20;
            let mut words = ix.text_words(typed, &v, 12);
            let start = Instant::now();
            for _ in 0..runs {
                words = ix.text_words(typed, &v, 12);
            }
            println!("  x:{typed:<10} {:>10.2?}  {}", start.elapsed() / runs, words.join(" "));
        }
        for q in ["x:l", "x:lo", "-x:lo"] {
            let query = Query::parse(q);
            let runs = 20;
            let start = Instant::now();
            let mut total = 0;
            for _ in 0..runs {
                total = ix.search(&query, &v, &SearchOptions::default()).total;
            }
            println!("  search {q:<8} {:>10.2?}  {total} hits", start.elapsed() / runs);
        }
    }
}
