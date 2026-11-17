//! Review of the search round: the suggestion vocabulary after a sync
//! (the W0 claim, also inside an opened pack and for `e:`), and whether
//! the emojis offered depend on the order the items come in (the server
//! builds its vocabulary from its index in slot order, the clients from
//! `OfflineIndex::items()`, a `HashMap`'s order).

use stickpile_api::{SetSource, Visibility};

use super::tests::{cfg, item};
use super::*;

fn with_emojis(id: &str, emojis: &[&str]) -> ItemSummary {
    ItemSummary {
        emojis: emojis.iter().map(|e| e.to_string()).collect(),
        ..item(id)
    }
}

/// Fourteen emojis used once, one used twice in two spellings (with and
/// without its variation selector): ties everywhere, as in a real library.
fn library() -> Vec<ItemSummary> {
    let mut items = vec![with_emojis("a", &["❤️"]), with_emojis("b", &["❤"])];
    for (n, e) in [
        "🐸", "🗿", "🦆", "🔥", "👲", "🩻", "👍", "👍🏽", "👎🏿", "🐱", "😂", "💀", "☠️", "🥲",
    ]
    .iter()
    .enumerate()
    {
        items.push(with_emojis(&format!("i{n}"), &[e]));
    }
    items
}

#[test]
fn the_emoji_offers_do_not_depend_on_the_order_the_items_come_in() {
    let items = library();
    let mut reversed = items.clone();
    reversed.reverse();
    let one = Vocabulary::from_items(&items);
    let other = Vocabulary::from_items(&reversed);
    let offers = |v: &Vocabulary, q: &str| suggest(q, v).map(|s| s.words).unwrap_or_default();
    assert_eq!(
        offers(&one, "e:"),
        offers(&other, "e:"),
        "e: alone offers other emojis (and ❤️ or ❤) for the same items in another order"
    );
    assert_eq!(offers(&one, "e:thumbs"), offers(&other, "e:thumbs"));
    assert_eq!(offers(&one, "e:❤"), offers(&other, "e:❤"));
}

fn sent(cmds: &[Command]) -> Option<String> {
    cmds.iter().find_map(|c| match c {
        Command::Search { query, .. } => Some(query.clone()),
        _ => None,
    })
}

/// W0: `e:` typed before the first sync is over: the emojis come with the
/// vocabulary the sync loads, without typing again.
#[test]
fn a_sync_fills_the_emoji_line_already_typed() {
    let mut m = Model::new(cfg());
    m.handle(Event::Open(OpenInfo::default()));
    m.handle(Event::VocabularyLoaded(Vocabulary::default()));
    m.handle(Event::QueryChanged("cat e:".into()));
    assert_eq!(m.suggestions(), None);
    assert_eq!(m.handle(Event::LibraryChanged), [Command::LoadVocabulary]);
    let skull = Vocabulary::from_items(&[with_emojis("a", &["💀"])]);
    let cmds = m.handle(Event::VocabularyLoaded(skull));
    let line = m.suggestions().expect("offered now");
    assert_eq!(line.words, ["💀"]);
    assert_eq!(line.labels, ["skull"]);
    assert_eq!(m.query(), "cat e:", "the field stays as typed");
    assert_eq!(sent(&cmds), None, "an empty e: is left out either way");
    m.handle(Event::QueryChanged("cat e:sk".into()));
    assert_eq!(m.suggestions().unwrap().words[0], "💀", "the own one first");
}

fn pack_set(visibility: Visibility) -> SetSummary {
    SetSummary {
        id: "set1".into(),
        slug: "cats".into(),
        name: "Cats".into(),
        description: String::new(),
        owner_username: "me".into(),
        visibility,
        source: SetSource::Local,
        item_count: 2,
        cover_item_id: None,
        cover_url: None,
        updated_at: 0,
    }
}

/// W0 inside an opened pack: `v:` typed in the pack's field; the
/// vocabulary event after a sync offers the new values in place.
#[test]
fn a_sync_fills_the_line_inside_an_opened_pack() {
    let mut m = Model::new(ModelConfig {
        start_tab: Some(Tab::Packs),
        ..cfg()
    });
    let cmds = m.handle(Event::Open(OpenInfo::default()));
    let generation = cmds
        .iter()
        .find_map(|c| match c {
            Command::LoadPacks { generation } => Some(*generation),
            _ => None,
        })
        .unwrap();
    m.handle(Event::VocabularyLoaded(Vocabulary::default()));
    m.handle(Event::PacksLoaded {
        generation,
        outcome: Ok(vec![pack_set(Visibility::Private)]),
    });
    let cmds = m.handle(Event::CopySelected { toggle_sticker: false });
    let generation = cmds
        .iter()
        .find_map(|c| match c {
            Command::LoadPackItems { generation, .. } => Some(*generation),
            _ => None,
        })
        .unwrap();
    let items = vec![
        ItemSummary {
            vibe: Some("tired".into()),
            ..item("a")
        },
        item("b"),
    ];
    m.handle(Event::PackItemsLoaded {
        set_id: "set1".into(),
        generation,
        outcome: Ok(items),
    });
    m.handle(Event::QueryChanged("v:".into()));
    assert_eq!(m.suggestions().unwrap().words, ["tired"], "the pack's own values");
    m.handle(Event::LibraryChanged);
    m.handle(Event::VocabularyLoaded(Vocabulary {
        vibes: vec!["salty".into()],
        ..Vocabulary::default()
    }));
    assert_eq!(m.query(), "v:", "the field stays as typed");
    assert_eq!(
        m.suggestions().unwrap().words,
        ["tired", "salty"],
        "the library's values join without typing again"
    );
}
