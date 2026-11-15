//! Safe mode (`S`): items tagged `nsfw` left out of every grid and of the
//! search; and the footer's stats line.

use super::tests::{cfg, hits, item};
use super::*;
use crate::vim::NormalKey;

fn key(m: &mut Model, c: char) -> Vec<Command> {
    m.handle(Event::Normal(NormalKey::Char(c)))
}

fn nsfw(id: &str) -> ItemSummary {
    ItemSummary {
        tags: vec!["fun".into(), "NSFW".into()],
        ..item(id)
    }
}

fn ids(m: &Model) -> Vec<String> {
    m.entries().iter().map(|e| e.item.id.clone()).collect()
}

fn search_query(cmds: &[Command]) -> Option<(String, u64)> {
    cmds.iter().find_map(|c| match c {
        Command::Search { query, debounce_ms, .. } => Some((query.clone(), *debounce_ms)),
        _ => None,
    })
}

/// Normal mode on Recent, showing `a`, `x` (nsfw), `b`.
fn recent(safe_mode: bool) -> Model {
    let mut m = Model::new(ModelConfig {
        start_tab: Some(Tab::Recent),
        start_mode: Mode::Normal,
        safe_mode,
        ..cfg()
    });
    m.handle(Event::Open(OpenInfo::default()));
    m.handle(Event::RecentLoaded(vec![item("a"), nsfw("x"), item("b")]));
    m
}

#[test]
fn s_hides_and_shows_nsfw_items_and_saves_it() {
    let mut m = recent(false);
    assert_eq!(ids(&m), ["a", "x", "b"]);
    assert!(!m.footer().safe);

    let cmds = key(&mut m, 'S');
    assert!(m.safe());
    assert_eq!(ids(&m), ["a", "b"]);
    assert_eq!(m.hidden(), 1);
    assert!(cmds.contains(&Command::SaveSafeMode { on: true }));
    assert_eq!(m.status(), &Status::Safe { on: true, hidden: 1 });
    assert_eq!(m.status().text(), "Safe mode on: 1 item tagged nsfw is hidden");
    let f = m.footer();
    assert!(f.safe);
    assert_eq!(f.hidden, 1);

    let cmds = key(&mut m, 'S');
    assert!(!m.safe());
    assert_eq!(ids(&m), ["a", "x", "b"]);
    assert_eq!(m.hidden(), 0);
    assert!(cmds.contains(&Command::SaveSafeMode { on: false }));
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::LoadThumbnails { items } if items.iter().any(|i| i.id == "x"))),
        "the items shown again need their thumbnails"
    );
    assert_eq!(m.status().text(), "Safe mode off: items tagged nsfw are shown");
}

#[test]
fn safe_mode_holds_across_tabs_and_openings() {
    let mut m = recent(true);
    assert_eq!(ids(&m), ["a", "b"], "on from the config");
    m.handle(Event::Hide);
    m.handle(Event::Open(OpenInfo::default()));
    let cmds = m.handle(Event::ShowTab(Tab::Favorites));
    let generation = cmds
        .iter()
        .find_map(|c| match c {
            Command::LoadFavorites { generation, .. } => Some(*generation),
            _ => None,
        })
        .expect("favorites requested");
    m.handle(Event::FavoritesLoaded {
        generation,
        outcome: Ok(vec![nsfw("y"), item("c")]),
    });
    assert!(m.safe());
    assert_eq!(ids(&m), ["c"]);
    assert_eq!(m.hidden(), 1);
}

#[test]
fn the_search_leaves_nsfw_out_and_runs_again_on_a_switch() {
    let mut m = Model::new(cfg());
    m.handle(Event::Open(OpenInfo::default()));
    let cmds = m.handle(Event::QueryChanged("cat".into()));
    assert_eq!(search_query(&cmds), Some(("cat".into(), cfg().debounce_ms)));
    let generation = m.generation();
    let mut found = hits(2);
    found[1].item.tags = vec!["nsfw".into()];
    m.handle(Event::Results {
        generation,
        outcome: Ok(Results {
            hits: found,
            offline: false,
        }),
    });
    assert_eq!(m.entries().len(), 2);

    let cmds = m.handle(Event::ToggleSafe);
    assert_eq!(m.entries().len(), 1, "filtered at once");
    assert_eq!(
        search_query(&cmds),
        Some(("cat -tag:nsfw".into(), 0)),
        "and searched again without waiting"
    );
    assert!(cmds.contains(&Command::SaveSafeMode { on: true }));

    // Typing on keeps the filter in the query.
    let cmds = m.handle(Event::QueryChanged("cats".into()));
    assert_eq!(search_query(&cmds).unwrap().0, "cats -tag:nsfw");
    let generation = m.generation();
    m.handle(Event::Results {
        generation,
        outcome: Ok(Results::default()),
    });
    assert_eq!(m.status().text(), "No matches for “cats” with safe mode on");
}

#[test]
fn a_list_of_nsfw_items_only_says_why_it_looks_empty() {
    let mut m = recent(true);
    m.handle(Event::RecentLoaded(vec![nsfw("x"), nsfw("y")]));
    assert!(m.entries().is_empty());
    assert_eq!(m.status(), &Status::AllHidden { hidden: 2 });
    assert_eq!(m.status().text(), "2 items hidden by safe mode (tagged nsfw)");
}

#[test]
fn a_changed_setting_wins_over_the_key() {
    let mut m = recent(false);
    key(&mut m, 'S');
    assert!(m.safe());
    // Saving other settings leaves the key's choice alone.
    m.handle(Event::Configure(ModelConfig {
        start_tab: Some(Tab::Recent),
        start_mode: Mode::Normal,
        columns: 5,
        ..cfg()
    }));
    assert!(m.safe());
    // The setting itself switched (the Settings tab): it wins.
    m.handle(Event::Configure(ModelConfig {
        start_tab: Some(Tab::Recent),
        start_mode: Mode::Normal,
        safe_mode: true,
        ..cfg()
    }));
    m.handle(Event::Configure(ModelConfig {
        start_tab: Some(Tab::Recent),
        start_mode: Mode::Normal,
        safe_mode: false,
        ..cfg()
    }));
    assert!(!m.safe());
    assert_eq!(ids(&m), ["a", "x", "b"]);
}

#[test]
fn nsfw_tags_compare_without_case() {
    assert!(is_nsfw(&nsfw("x")));
    assert!(!is_nsfw(&item("a")));
    assert!(is_nsfw(&ItemSummary {
        tags: vec![" nsfw ".into()],
        ..item("z")
    }));
}

#[test]
fn the_footer_shows_the_selection_stats() {
    let mut m = recent(false);
    let mut gif = item("g");
    gif.name = "dance".into();
    gif.kind = MediaKind::Animation;
    gif.format = MediaFormat::Gif;
    gif.width = 320;
    gif.height = 240;
    gif.copies = 12;
    gif.likes = 3;
    gif.quality = Some(87);
    m.handle(Event::RecentLoaded(vec![gif, item("b")]));
    assert_eq!(
        m.footer().details,
        "dance · Cats · GIF 320×240 · copied 12 times · ♥ 3 · quality 87"
    );
    key(&mut m, 'l');
    assert_eq!(
        m.footer().details,
        "item-b · Cats · PNG 512×512 · never copied · quality 80"
    );

    let mut once = item("o");
    once.copies = 1;
    once.quality = None;
    assert_eq!(item_details(&once), "item-o · Cats · PNG 512×512 · copied 1 time");

    // Pages show none; an empty list shows none.
    m.handle(Event::ShowTab(Tab::Me));
    assert_eq!(m.footer().details, "");
    let mut empty = recent(false);
    empty.handle(Event::RecentLoaded(Vec::new()));
    assert_eq!(empty.footer().details, "");
}

#[test]
fn a_pack_shows_its_size_and_owner() {
    let set = SetSummary {
        id: "s".into(),
        slug: "cats".into(),
        name: "Cats".into(),
        description: String::new(),
        owner_username: "alice".into(),
        visibility: stickpile_api::Visibility::Private,
        source: stickpile_api::SetSource::Local,
        item_count: 39,
        cover_item_id: None,
        cover_url: None,
        updated_at: 0,
    };
    assert_eq!(pack_details(&set), "Cats · 39 items · by alice");
}
