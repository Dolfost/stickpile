//! The tab and mode an opening starts in, favorite marks and the `z` keys.

use super::tests::{cfg, item, items};
use super::*;
use crate::vim::NormalKey;

fn keys(m: &mut Model, s: &str) -> Vec<Command> {
    s.chars()
        .flat_map(|c| m.handle(Event::Normal(NormalKey::Char(c))))
        .collect()
}

fn open(m: &mut Model) -> Vec<Command> {
    m.handle(Event::Open(OpenInfo::default()))
}

fn favorites_gen(cmds: &[Command]) -> Option<u64> {
    cmds.iter().find_map(|c| match c {
        Command::LoadFavorites { generation, .. } => Some(*generation),
        _ => None,
    })
}

fn liked(m: &Model) -> Vec<String> {
    m.entries()
        .iter()
        .filter(|e| e.liked)
        .map(|e| e.item.id.clone())
        .collect()
}

/// Opened on Recent with marks on, showing items `0`..`3`; the favorites
/// request it sent is unanswered.
fn recent_with_marks() -> (Model, u64) {
    let mut m = Model::new(ModelConfig {
        start_tab: Some(Tab::Recent),
        tiles: Tiles::default(),
        ..cfg()
    });
    let cmds = open(&mut m);
    let generation = favorites_gen(&cmds).expect("marks need the favorites");
    m.handle(Event::RecentLoaded(items(4)));
    (m, generation)
}

// -- start tab and mode --------------------------------------------------------

#[test]
fn reopening_keeps_the_last_tab() {
    let mut m = Model::new(ModelConfig {
        start_tab: None,
        ..cfg()
    });
    assert_eq!(m.tab(), Tab::Search, "nothing remembered yet");
    open(&mut m);
    m.handle(Event::ShowTab(Tab::Packs));
    m.handle(Event::Hide);
    let cmds = open(&mut m);
    assert_eq!(m.tab(), Tab::Packs);
    assert!(cmds.iter().any(|c| matches!(c, Command::LoadPacks { .. })));
}

#[test]
fn a_saved_tab_is_where_the_first_opening_starts() {
    let mut m = Model::new(ModelConfig {
        start_tab: None,
        ..cfg()
    });
    m.restore_tab(Tab::MostUsed);
    open(&mut m);
    assert_eq!(m.tab(), Tab::MostUsed);
    m.handle(Event::Hide);
    m.restore_tab(Tab::Me);
    open(&mut m);
    assert_eq!(m.tab(), Tab::MostUsed, "only before the first opening");

    let mut fixed = Model::new(ModelConfig {
        start_tab: Some(Tab::Favorites),
        ..cfg()
    });
    fixed.restore_tab(Tab::Packs);
    open(&mut fixed);
    assert_eq!(fixed.tab(), Tab::Favorites, "a fixed start tab wins");
}

#[test]
fn start_mode_normal_opens_every_tab_in_normal() {
    for tab in Tab::ALL {
        let mut m = Model::new(ModelConfig {
            start_tab: Some(tab),
            start_mode: Mode::Normal,
            ..cfg()
        });
        open(&mut m);
        assert_eq!(m.mode(), Mode::Normal, "{tab:?}");
    }
    let mut m = Model::new(ModelConfig {
        start_mode: Mode::Normal,
        ..cfg()
    });
    open(&mut m);
    keys(&mut m, "i");
    assert_eq!(m.mode(), Mode::Insert, "i still types");
    m.handle(Event::Hide);
    open(&mut m);
    assert_eq!(m.mode(), Mode::Normal, "every opening");
    m.handle(Event::ShowTab(Tab::Packs));
    assert_eq!(m.mode(), Mode::Insert, "a tab switched to keeps its own mode");
}

#[test]
fn start_mode_insert_types_where_there_is_a_text_field() {
    for (tab, mode) in [(Tab::Search, Mode::Insert), (Tab::Recent, Mode::Normal)] {
        let mut m = Model::new(ModelConfig {
            start_tab: Some(tab),
            start_mode: Mode::Insert,
            ..cfg()
        });
        open(&mut m);
        assert_eq!(m.mode(), mode, "{tab:?}");
    }
    let mut m = Model::new(ModelConfig {
        modal: false,
        start_mode: Mode::Normal,
        ..cfg()
    });
    open(&mut m);
    assert_eq!(m.mode(), Mode::Insert, "no modes: always typing");
}

// -- favorite marks ----------------------------------------------------------------

#[test]
fn loaded_favorites_mark_the_tiles_of_other_tabs() {
    let (mut m, generation) = recent_with_marks();
    assert!(liked(&m).is_empty());
    let revision = m.entries_revision();
    m.handle(Event::FavoritesLoaded {
        generation,
        outcome: Ok(vec![item("1"), item("3"), item("elsewhere")]),
    });
    assert_eq!(liked(&m), ["1", "3"]);
    assert!(m.entries_revision() > revision);
    assert_eq!(m.tab(), Tab::Recent, "loading them changes no tab");
}

#[test]
fn likes_and_unlikes_update_the_marks() {
    let (mut m, generation) = recent_with_marks();
    m.handle(Event::FavoritesLoaded {
        generation,
        outcome: Ok(vec![item("1")]),
    });
    m.handle(Event::FavoriteToggled {
        item_id: "2".into(),
        outcome: Ok(true),
    });
    assert_eq!(liked(&m), ["1", "2"]);
    m.handle(Event::FavoriteToggled {
        item_id: "1".into(),
        outcome: Ok(false),
    });
    assert_eq!(liked(&m), ["2"]);
    m.handle(Event::FavoriteToggled {
        item_id: "2".into(),
        outcome: Err("offline".into()),
    });
    assert_eq!(liked(&m), ["2"], "a failed toggle changes nothing");
}

#[test]
fn a_failed_favorites_load_keeps_the_marks() {
    let (mut m, generation) = recent_with_marks();
    m.handle(Event::FavoritesLoaded {
        generation,
        outcome: Ok(vec![item("0")]),
    });
    m.handle(Event::Hide);
    let generation = favorites_gen(&open(&mut m)).unwrap();
    m.handle(Event::RecentLoaded(items(4)));
    m.handle(Event::FavoritesLoaded {
        generation,
        outcome: Err("server unreachable".into()),
    });
    assert_eq!(liked(&m), ["0"]);
    assert_eq!(m.status(), &Status::Idle, "Recent says nothing about favorites");
}

#[test]
fn without_marks_no_favorites_are_fetched_for_other_tabs() {
    let mut m = Model::new(ModelConfig {
        start_tab: Some(Tab::Recent),
        ..cfg()
    });
    assert!(favorites_gen(&open(&mut m)).is_none());
    let mut m = Model::new(ModelConfig {
        start_tab: Some(Tab::Favorites),
        tiles: Tiles::default(),
        ..cfg()
    });
    let cmds = open(&mut m);
    let loads = cmds
        .iter()
        .filter(|c| matches!(c, Command::LoadFavorites { .. }))
        .count();
    assert_eq!(loads, 1, "the Favorites tab's own request serves the marks");
}

// -- z keys ---------------------------------------------------------------------------

#[test]
fn z_keys_show_and_hide_and_are_saved() {
    let (mut m, _) = recent_with_marks();
    assert_eq!(m.mode(), Mode::Normal);
    let revision = m.entries_revision();
    let cmds = keys(&mut m, "zt");
    assert_eq!(
        cmds,
        [Command::SaveTile {
            tile: Tile::Tags,
            on: false
        }]
    );
    assert!(!m.tiles().tags);
    assert!(m.entries_revision() > revision, "the tiles are drawn again");
    assert_eq!(m.footer().status, "Tags hidden");
    keys(&mut m, "ze");
    assert!(!m.tiles().emojis);
    assert_eq!(m.footer().status, "Emojis hidden");
    keys(&mut m, "zt");
    assert!(m.tiles().tags);
    assert_eq!(m.footer().status, "Tags shown");

    let cmds = keys(&mut m, "zf");
    assert_eq!(
        cmds,
        [Command::SaveTile {
            tile: Tile::Favorites,
            on: false
        }]
    );
    let cmds = keys(&mut m, "zf");
    assert_eq!(
        cmds[..1],
        [Command::SaveTile {
            tile: Tile::Favorites,
            on: true
        }]
    );
    assert_eq!(m.footer().status, "Favorite marks shown");
}

#[test]
fn marks_turned_on_fetch_the_favorites() {
    let mut m = Model::new(ModelConfig {
        start_tab: Some(Tab::Recent),
        ..cfg()
    });
    open(&mut m);
    let cmds = keys(&mut m, "zf");
    assert!(favorites_gen(&cmds).is_some());
    assert!(cmds.contains(&Command::SaveTile {
        tile: Tile::Favorites,
        on: true
    }));
}

#[test]
fn a_changed_setting_overrides_a_z_key_and_others_leave_it() {
    let (mut m, _) = recent_with_marks();
    keys(&mut m, "zt");
    let base = m.config().clone();
    m.handle(Event::Configure(ModelConfig {
        columns: 5,
        ..base.clone()
    }));
    assert!(!m.tiles().tags, "an unrelated change keeps what zt did");
    m.handle(Event::Configure(ModelConfig {
        tiles: Tiles {
            emojis: false,
            ..base.tiles
        },
        ..base.clone()
    }));
    assert!(!m.tiles().emojis);
    assert!(!m.tiles().tags);
    let cmds = m.handle(Event::Configure(ModelConfig {
        tiles: Tiles {
            tags: false,
            ..base.tiles
        },
        ..base
    }));
    assert!(m.tiles().emojis, "emojis setting went back to on");
    assert!(
        cmds.iter().all(|c| !matches!(c, Command::SaveTile { .. })),
        "settings are not saved back"
    );
}
