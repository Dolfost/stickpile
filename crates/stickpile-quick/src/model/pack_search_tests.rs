//! The search inside an opened pack: the whole search syntax over the
//! pack's items, the suggestions and Tab completion in its field, and the
//! placeholder of each field.

use stickpile_api::{SetSource, Visibility};

use super::tests::{cfg, ids, item};
use super::*;

fn set(id: &str, name: &str, visibility: Visibility) -> SetSummary {
    SetSummary {
        id: id.into(),
        slug: name.to_lowercase().replace(' ', "-"),
        name: name.into(),
        description: String::new(),
        owner_username: "me".into(),
        visibility,
        source: SetSource::Local,
        item_count: 4,
        cover_item_id: None,
        cover_url: None,
        updated_at: 0,
    }
}

/// An item of the pack "Cats" (`s1`), the viewer's.
fn cat(id: &str, name: &str, vibe: Option<&str>, tags: &[&str], format: MediaFormat) -> ItemSummary {
    ItemSummary {
        name: name.into(),
        vibe: vibe.map(str::to_string),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        keywords: vec![],
        emojis: vec![],
        kind: if format == MediaFormat::Gif {
            MediaKind::Animation
        } else {
            MediaKind::Still
        },
        format,
        ..item(id)
    }
}

fn pack_items() -> Vec<ItemSummary> {
    vec![
        ItemSummary {
            keywords: vec!["grumpy".into()],
            quality: Some(40),
            copies: 1,
            ..cat("a", "grumpy cat", Some("angry"), &["reaction"], MediaFormat::Png)
        },
        ItemSummary {
            quality: Some(90),
            copies: 7,
            ..cat("b", "sleepy cat", Some("tired"), &["x"], MediaFormat::Gif)
        },
        ItemSummary {
            quality: Some(70),
            copies: 3,
            emojis: vec!["😴".into()],
            ..cat("c", "yawn", Some("tired"), &["reaction"], MediaFormat::Webp)
        },
        ItemSummary {
            quality: None,
            ..cat("d", "big chungus", None, &["nsfw"], MediaFormat::Gif)
        },
    ]
}

/// Opened on Packs, with "Cats" (`s1`, `visibility`) opened and its items
/// loaded; the vocabulary names the viewer `me`.
fn pack_with(cfg: ModelConfig, visibility: Visibility) -> Model {
    let mut m = Model::new(ModelConfig {
        start_tab: Some(Tab::Packs),
        ..cfg
    });
    let cmds = m.handle(Event::Open(OpenInfo::default()));
    let generation = cmds
        .iter()
        .find_map(|c| match c {
            Command::LoadPacks { generation } => Some(*generation),
            _ => None,
        })
        .unwrap();
    m.handle(Event::VocabularyLoaded(Vocabulary {
        vibes: vec!["sad".into(), "tired".into()],
        tags: vec!["reaction".into()],
        sets: vec!["cats".into(), "dogs".into()],
        owners: vec!["me".into(), "bob".into()],
        me: Some("me".into()),
    }));
    m.handle(Event::PacksLoaded {
        generation,
        outcome: Ok(vec![set("s1", "Cats", visibility), set("s2", "Dogs", visibility)]),
    });
    let cmds = m.handle(Event::CopySelected { toggle_sticker: false });
    let generation = cmds
        .iter()
        .find_map(|c| match c {
            Command::LoadPackItems { set_id, generation } if set_id == "s1" => Some(*generation),
            _ => None,
        })
        .unwrap();
    m.handle(Event::PackItemsLoaded {
        set_id: "s1".into(),
        generation,
        outcome: Ok(pack_items()),
    });
    assert_eq!((m.view(), m.mode()), (View::Items, Mode::Insert));
    m
}

fn pack() -> Model {
    let m = pack_with(cfg(), Visibility::Private);
    assert_eq!(ids(&m), ["a", "b", "c", "d"]);
    m
}

fn typed(m: &mut Model, q: &str) -> Vec<String> {
    let cmds = m.handle(Event::QueryChanged(q.into()));
    assert!(
        !cmds.iter().any(|c| matches!(c, Command::Search { .. })),
        "{q}: the pack is searched here, not by the server"
    );
    ids(m)
}

#[test]
fn filters_narrow_the_pack() {
    let mut m = pack();
    for (q, want) in [
        ("vibe:tired", vec!["b", "c"]),
        ("v:tired", vec!["b", "c"]),
        ("type:gif", vec!["b", "d"]),
        ("-tag:x", vec!["a", "c", "d"]),
        ("-t:x -t:nsfw", vec!["a", "c"]),
        ("t:reaction", vec!["a", "c"]),
        ("q>60", vec!["b", "c"]),
        ("type:animated -v:tired", vec!["d"]),
        ("is:mine", vec!["a", "b", "c", "d"]),
        ("is:public", vec![]),
        ("s:cats", vec!["a", "b", "c", "d"]),
        ("set:dogs", vec![]),
        ("b:bob", vec![]),
        ("sort:copies", vec!["b", "c", "a", "d"]),
        ("sort:quality t:reaction", vec!["c", "a"]),
        ("", vec!["a", "b", "c", "d"]),
    ] {
        assert_eq!(typed(&mut m, q), want, "{q}");
    }
    assert_eq!(m.status(), &Status::Idle);
    typed(&mut m, "vibe:happy");
    assert_eq!(
        m.status(),
        &Status::NoResults {
            query: "vibe:happy".into()
        }
    );
    assert_eq!(m.query(), "vibe:happy", "the field keeps what was typed");
}

#[test]
fn words_still_match_and_rank() {
    let mut m = pack();
    assert_eq!(typed(&mut m, "grumpy"), ["a"]);
    assert_eq!(typed(&mut m, "gr"), ["a"], "half typed");
    assert_eq!(typed(&mut m, "g"), ["a", "d"], "one letter: anywhere in the name");
    assert_eq!(typed(&mut m, "😴"), ["c"]);
    assert_eq!(typed(&mut m, "sleepy cat")[0], "b", "both words first");
    assert_eq!(typed(&mut m, "cat type:gif"), ["b"], "a word and a filter");
    assert_eq!(typed(&mut m, "cat -sleepy"), ["a"]);
    let mut tired = typed(&mut m, "tired");
    tired.sort();
    assert_eq!(tired, ["b", "c"], "the vibe as a word");
    let entry = |m: &Model| m.entries()[0].matched.clone();
    typed(&mut m, "grumpy");
    assert!(entry(&m).contains(&"name".to_string()), "{:?}", entry(&m));
}

#[test]
fn half_typed_values_find_their_best_match() {
    let mut m = pack();
    assert_eq!(typed(&mut m, "v:tir"), ["b", "c"]);
    assert_eq!(typed(&mut m, "vibe:tierd"), ["b", "c"], "a typo");
    assert_eq!(typed(&mut m, "v:"), ["a", "b", "c", "d"], "an empty filter is left out");
    assert_eq!(typed(&mut m, "v:sad"), Vec::<String>::new(), "a known value stays");
}

#[test]
fn safe_mode_still_hides_inside_the_search() {
    let mut m = pack_with(
        ModelConfig {
            safe_mode: true,
            ..cfg()
        },
        Visibility::Private,
    );
    assert_eq!(ids(&m), ["a", "b", "c"]);
    assert_eq!(typed(&mut m, "type:gif"), ["b"]);
    assert_eq!(m.hidden(), 1);
    typed(&mut m, "t:nsfw");
    assert!(m.entries().is_empty());
    assert_eq!(m.status(), &Status::NoSafeResults { query: "t:nsfw".into() });
}

#[test]
fn is_liked_waits_for_the_likes() {
    // Favorite marks off: the opening did not ask for the favorites.
    let mut m = pack();
    let cmds = m.handle(Event::QueryChanged("is:liked".into()));
    let generation = cmds
        .iter()
        .find_map(|c| match c {
            Command::LoadFavorites { generation, .. } => Some(*generation),
            _ => None,
        })
        .expect("the likes are fetched");
    assert!(m.entries().is_empty(), "none known yet");
    let cmds = m.handle(Event::FavoritesLoaded {
        generation,
        outcome: Ok(vec![item("c")]),
    });
    assert_eq!(ids(&m), ["c"]);
    assert!(matches!(&cmds[..], [Command::LoadThumbnails { .. }]));
    assert_eq!(typed(&mut m, "-is:liked"), ["a", "b", "d"]);
    let cmds = m.handle(Event::QueryChanged("is:liked v:tired".into()));
    assert!(
        !cmds.iter().any(|c| matches!(c, Command::LoadFavorites { .. })),
        "fetched once"
    );
    assert_eq!(ids(&m), ["c"]);
}

#[test]
fn public_packs_answer_is_public() {
    let mut m = pack_with(cfg(), Visibility::Public);
    assert_eq!(typed(&mut m, "is:public"), ["a", "b", "c", "d"]);
    assert_eq!(typed(&mut m, "-is:public"), Vec::<String>::new());
}

#[test]
fn the_field_suggests_the_packs_values_first() {
    let mut m = pack();
    m.handle(Event::QueryChanged("cat v:".into()));
    let line = m.suggestions().unwrap();
    assert_eq!(line.key, "v:");
    assert_eq!(
        line.words,
        ["tired", "angry", "sad"],
        "the pack's vibes, then the others"
    );
    assert_eq!(line.chosen, None);

    // Tab puts them in, in the pack's field; the search follows at once.
    let cmds = m.handle(Event::CycleTab { backwards: false });
    assert_eq!(m.tab(), Tab::Packs, "Tab completes instead of switching tabs");
    assert_eq!(m.query(), "cat v:tired");
    assert_eq!(ids(&m), ["b"]);
    assert!(matches!(&cmds[..], [Command::LoadThumbnails { .. }]));
    m.handle(Event::CycleTab { backwards: false });
    assert_eq!(m.query(), "cat v:angry");
    assert_eq!(ids(&m), ["a"]);

    // A click puts the value in and finishes the filter.
    m.handle(Event::QueryChanged("-t:re".into()));
    assert_eq!(m.suggestions().unwrap().words, ["reaction"]);
    m.handle(Event::AcceptSuggestion(0));
    assert_eq!(m.query(), "-t:reaction ");
    assert_eq!(ids(&m), ["b", "d"]);
    assert_eq!(m.suggestions(), None);
    assert_eq!(m.search_query(), "", "the search tab's query is untouched");

    // Out of the pack the line goes away with the completion.
    m.handle(Event::QueryChanged("v:t".into()));
    m.handle(Event::CycleTab { backwards: false });
    m.handle(Event::PackBack);
    assert_eq!(m.view(), View::Packs);
    assert_eq!(m.suggestions(), None, "the pack list's filter suggests nothing");
}

#[test]
fn placeholders_say_what_the_field_does() {
    let mut m = pack();
    assert_eq!(m.placeholder(), "Search in Cats, or try v:tired t:reaction type:gif");
    m.handle(Event::PackBack);
    assert_eq!(m.placeholder(), "Filter packs by name");
    m.handle(Event::ShowTab(Tab::Search));
    assert_eq!(m.placeholder(), "Search, or try e:😂 v:tired t:reaction type:gif");
}
