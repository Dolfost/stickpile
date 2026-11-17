//! Keeping offline: `K`, the menu of a cell, the question before keeping a
//! pack or the library or removing the kept files, and the offline marks.

use stickpile_api::{SetSource, Visibility};

use super::tests::{cfg, hits, items, with_results};
use super::*;
use crate::offline::MenuEntry;

fn keys(m: &mut Model, s: &str) -> Vec<Command> {
    s.chars()
        .flat_map(|c| m.handle(Event::Normal(NormalKey::Char(c))))
        .collect()
}

fn normal(m: &mut Model) {
    m.handle(Event::Escape);
    assert_eq!(m.mode(), Mode::Normal);
}

fn set(id: &str, name: &str) -> SetSummary {
    SetSummary {
        id: id.into(),
        slug: name.to_lowercase(),
        name: name.into(),
        description: String::new(),
        owner_username: "me".into(),
        visibility: Visibility::Public,
        source: SetSource::Local,
        item_count: 4,
        cover_item_id: None,
        cover_url: None,
        updated_at: 0,
    }
}

/// On the pack list with "Cats" (`s1`) and "Dogs" (`s2`), in Normal mode.
fn on_packs() -> Model {
    let mut m = Model::new(cfg());
    m.handle(Event::Open(OpenInfo::default()));
    let cmds = m.handle(Event::ShowTab(Tab::Packs));
    let generation = cmds
        .iter()
        .find_map(|c| match c {
            Command::LoadPacks { generation } => Some(*generation),
            _ => None,
        })
        .unwrap();
    m.handle(Event::PacksLoaded {
        generation,
        outcome: Ok(vec![set("s1", "Cats"), set("s2", "Dogs")]),
    });
    assert_eq!(m.view(), View::Packs);
    normal(&mut m);
    m
}

fn set_keep(cmds: &[Command]) -> Option<(KeepScope, bool)> {
    cmds.iter().find_map(|c| match c {
        Command::SetKeep { scope, keep } => Some((scope.clone(), *keep)),
        _ => None,
    })
}

fn check_of(cmds: &[Command]) -> Option<(u64, Vec<String>, TargetApp, bool)> {
    cmds.iter().find_map(|c| match c {
        Command::CheckOffline {
            items,
            target,
            sticker,
            generation,
        } => Some((
            *generation,
            items.iter().map(|i| i.id.clone()).collect(),
            *target,
            *sticker,
        )),
        _ => None,
    })
}

fn marks(m: &Model) -> Vec<OfflineMark> {
    m.entries().iter().map(|e| e.offline).collect()
}

fn labels(menu: &Menu) -> Vec<(Option<MenuAction>, String)> {
    menu.entries
        .iter()
        .map(|MenuEntry { action, label, .. }| (*action, label.clone()))
        .collect()
}

// -- K -------------------------------------------------------------------------------

#[test]
fn k_keeps_the_selected_sticker_and_k_again_releases_it() {
    let mut m = with_results(3);
    normal(&mut m);
    m.handle(Event::Navigate(Nav::Right));
    let cmds = keys(&mut m, "K");
    assert_eq!(set_keep(&cmds), Some((KeepScope::Item("h1".into()), true)));
    m.handle(Event::KeepDone {
        scope: KeepScope::Item("h1".into()),
        keep: true,
        outcome: Ok(()),
    });
    assert_eq!(m.keeps(), [KeepScope::Item("h1".into())]);
    assert_eq!(m.status().text(), "item-h1 is kept offline");
    assert_eq!(m.status().level(), Level::Info);

    let cmds = keys(&mut m, "K");
    assert_eq!(set_keep(&cmds), Some((KeepScope::Item("h1".into()), false)));
    m.handle(Event::KeepDone {
        scope: KeepScope::Item("h1".into()),
        keep: false,
        outcome: Ok(()),
    });
    assert!(m.keeps().is_empty());
    assert_eq!(m.status().text(), "item-h1 is no longer kept offline");

    // What the shell read wins over what the model did.
    m.handle(Event::OfflineKeeps(vec![KeepScope::Item("h1".into())]));
    assert_eq!(
        set_keep(&keys(&mut m, "K")),
        Some((KeepScope::Item("h1".into()), false))
    );
    m.handle(Event::KeepDone {
        scope: KeepScope::Item("h1".into()),
        keep: false,
        outcome: Err("cache: disk I/O error".into()),
    });
    assert_eq!(m.status().text(), "Keeping offline failed: cache: disk I/O error");
    assert_eq!(m.status().level(), Level::Error);
    assert_eq!(m.keeps(), [KeepScope::Item("h1".into())], "unchanged after a failure");
}

#[test]
fn k_without_a_selection_says_so() {
    let mut m = Model::new(ModelConfig {
        start_tab: Some(Tab::Recent),
        ..cfg()
    });
    m.handle(Event::Open(OpenInfo::default()));
    assert!(keys(&mut m, "K").is_empty());
    assert_eq!(*m.status(), Status::NothingSelected);
}

#[test]
fn k_on_a_pack_asks_with_the_estimate_first() {
    let mut m = on_packs();
    keys(&mut m, "l");
    let cmds = keys(&mut m, "K");
    assert_eq!(
        cmds,
        [Command::EstimateKeep {
            scope: KeepScope::Set("s2".into())
        }]
    );
    assert_eq!(m.status().text(), "Counting what Dogs takes...");
    assert_eq!(m.status().level(), Level::Busy);
    assert!(m.confirm().is_none());

    // An answer for something else is not this one's.
    m.handle(Event::Estimated {
        scope: KeepScope::Set("s1".into()),
        outcome: Ok(Estimate::default()),
    });
    assert!(m.confirm().is_none());
    m.handle(Event::Estimated {
        scope: KeepScope::Set("s2".into()),
        outcome: Ok(Estimate {
            items: 4,
            files: 9,
            bytes: 2 * 1024 * 1024,
            kept_bytes_after: 2 * 1024 * 1024,
            budget: Some(1_073_741_824),
            ..Estimate::default()
        }),
    });
    let c = m.confirm().expect("asked");
    assert_eq!(c.title, "Keep Dogs offline?");
    assert!(c.body.starts_with("4 stickers, 9 files, 2.0 MiB."), "{}", c.body);
    assert_eq!(c.accept, "Keep offline");
    assert_eq!(*m.status(), Status::Idle);

    // The question has the keys: no moving, no tab switches under it.
    assert!(keys(&mut m, "h3").is_empty());
    assert_eq!(m.selected(), Some(1));
    assert_eq!(m.tab(), Tab::Packs);
    let cmds = m.handle(Event::Normal(NormalKey::Enter { shift: false }));
    assert_eq!(set_keep(&cmds), Some((KeepScope::Set("s2".into()), true)));
    assert!(m.confirm().is_none());
    m.handle(Event::KeepDone {
        scope: KeepScope::Set("s2".into()),
        keep: true,
        outcome: Ok(()),
    });
    assert!(m.pack_kept("s2") && !m.pack_kept("s1"));
    assert_eq!(m.status().text(), "Pack Dogs is kept offline");

    // A kept pack is released without a question.
    let cmds = keys(&mut m, "K");
    assert_eq!(set_keep(&cmds), Some((KeepScope::Set("s2".into()), false)));
    assert!(m.confirm().is_none());
}

#[test]
fn the_question_can_be_declined_and_typing_goes_on() {
    let mut m = with_results(2);
    assert_eq!(m.mode(), Mode::Insert);
    m.handle(Event::Keep {
        scope: KeepScope::Library,
        keep: true,
    });
    m.handle(Event::Estimated {
        scope: KeepScope::Library,
        outcome: Ok(Estimate::default()),
    });
    assert_eq!(m.confirm().unwrap().title, "Keep the whole library offline?");
    assert_eq!(m.mode(), Mode::Normal, "out of the text field");
    for no in [
        Event::Normal(NormalKey::Char('n')),
        Event::Normal(NormalKey::Char('q')),
        Event::Normal(NormalKey::Escape),
        Event::Escape,
        Event::Confirm(false),
    ] {
        m.handle(Event::Keep {
            scope: KeepScope::Library,
            keep: true,
        });
        m.handle(Event::Estimated {
            scope: KeepScope::Library,
            outcome: Ok(Estimate::default()),
        });
        assert!(m.confirm().is_some());
        assert!(m.handle(no.clone()).is_empty(), "{no:?}");
        assert!(m.confirm().is_none(), "{no:?}");
        assert_eq!(m.mode(), Mode::Insert, "back to typing after {no:?}");
        assert!(m.is_visible(), "{no:?} answers, it does not hide");
    }
    // Hiding drops it.
    m.handle(Event::Keep {
        scope: KeepScope::Library,
        keep: true,
    });
    m.handle(Event::Estimated {
        scope: KeepScope::Library,
        outcome: Ok(Estimate::default()),
    });
    m.handle(Event::Hide);
    assert!(m.confirm().is_none());
}

#[test]
fn an_estimate_that_fails_says_so() {
    let mut m = on_packs();
    keys(&mut m, "K");
    m.handle(Event::Estimated {
        scope: KeepScope::Set("s1".into()),
        outcome: Err("cache: locked".into()),
    });
    assert!(m.confirm().is_none());
    assert_eq!(m.status().text(), "Keeping offline failed: cache: locked");
}

#[test]
fn the_library_switch_keeps_after_a_yes_and_releases_at_once() {
    let mut m = with_results(1);
    let cmds = m.handle(Event::Keep {
        scope: KeepScope::Library,
        keep: true,
    });
    assert_eq!(
        cmds,
        [Command::EstimateKeep {
            scope: KeepScope::Library
        }]
    );
    assert_eq!(m.status().text(), "Counting what the whole library takes...");
    m.handle(Event::Estimated {
        scope: KeepScope::Library,
        outcome: Ok(Estimate {
            kept_bytes_after: 3 << 30,
            budget: Some(1 << 30),
            ..Estimate::default()
        }),
    });
    assert!(m.confirm().unwrap().body.contains("over the 1.0 GiB allowed"));
    let cmds = m.handle(Event::Confirm(true));
    assert_eq!(set_keep(&cmds), Some((KeepScope::Library, true)));
    m.handle(Event::KeepDone {
        scope: KeepScope::Library,
        keep: true,
        outcome: Ok(()),
    });
    assert!(m.library_kept());
    assert_eq!(m.status().text(), "The whole library is kept offline");

    let cmds = m.handle(Event::Keep {
        scope: KeepScope::Library,
        keep: false,
    });
    assert_eq!(set_keep(&cmds), Some((KeepScope::Library, false)));
    m.handle(Event::KeepDone {
        scope: KeepScope::Library,
        keep: false,
        outcome: Ok(()),
    });
    assert!(!m.library_kept());
    assert_eq!(m.status().text(), "The library is no longer kept offline");
}

#[test]
fn removing_the_kept_files_asks_first() {
    let mut m = with_results(1);
    m.handle(Event::OfflineKeeps(vec![
        KeepScope::Library,
        KeepScope::Item("h0".into()),
    ]));
    assert!(m
        .handle(Event::AskRemoveKept {
            files: 12,
            bytes: 3 << 20
        })
        .is_empty());
    let c = m.confirm().unwrap();
    assert!(c.destructive);
    assert_eq!(c.title, "Remove kept files?");
    assert_eq!(m.handle(Event::Normal(NormalKey::Char('y'))), [Command::RemoveKept]);
    m.handle(Event::KeptRemoved(Ok((12, 3 << 20))));
    assert!(m.keeps().is_empty());
    assert_eq!(m.status().text(), "Removed 12 kept files (3.0 MiB)");
}

// -- the menu -------------------------------------------------------------------------

#[test]
fn the_menu_of_a_sticker_says_why_it_is_kept() {
    let mut m = with_results(3);
    let cmds = m.handle(Event::OpenMenu(Some(2)));
    assert!(matches!(&cmds[..], [Command::LoadKeptReasons { item }] if item.id == "h2"));
    assert_eq!(m.selected(), Some(2));
    assert!(m.menu().is_none(), "shown once the reasons are known");

    m.handle(Event::KeptReasons {
        item_id: "h2".into(),
        outcome: Ok(vec![KeptFor::Set]),
    });
    let menu = m.menu().unwrap();
    assert_eq!(menu.index, 2);
    assert_eq!(
        labels(&menu),
        [
            (Some(MenuAction::Copy), "Copy".to_string()),
            (Some(MenuAction::Favorite), "add to / remove from favorites".into()),
            (Some(MenuAction::Browser), "Open in the browser".into()),
            (None, "Kept offline: pack Cats".into()),
            (Some(MenuAction::Keep), "Keep offline".into()),
        ]
    );
    assert_eq!(menu.entries[4].keys, "K");
    let cmds = m.handle(Event::MenuChosen(MenuAction::Keep));
    assert_eq!(set_keep(&cmds), Some((KeepScope::Item("h2".into()), true)));
    assert!(m.menu().is_none());

    // Kept by itself and liked: release, and the other reason.
    m.handle(Event::OpenMenu(None));
    m.handle(Event::KeptReasons {
        item_id: "h2".into(),
        outcome: Ok(vec![KeptFor::Item, KeptFor::Liked]),
    });
    let menu = m.menu().unwrap();
    assert!(labels(&menu).contains(&(Some(MenuAction::Release), "Stop keeping offline".into())));
    assert!(labels(&menu).contains(&(None, "Also kept offline: liked".into())));
    let cmds = m.handle(Event::MenuChosen(MenuAction::Release));
    assert_eq!(set_keep(&cmds), Some((KeepScope::Item("h2".into()), false)));

    // A choice the menu does not offer does nothing.
    m.handle(Event::OpenMenu(Some(0)));
    m.handle(Event::KeptReasons {
        item_id: "h0".into(),
        outcome: Ok(vec![]),
    });
    assert!(m.handle(Event::MenuChosen(MenuAction::Release)).is_empty());
}

#[test]
fn the_menu_copies_and_closes_on_other_input() {
    let mut m = with_results(3);
    m.handle(Event::SetTarget(TargetApp::Telegram));
    m.handle(Event::OpenMenu(Some(1)));
    // An answer for another item is not this menu's.
    m.handle(Event::KeptReasons {
        item_id: "h0".into(),
        outcome: Ok(vec![]),
    });
    assert!(m.menu().is_none());
    m.handle(Event::KeptReasons {
        item_id: "h1".into(),
        outcome: Err("cache: busy".into()),
    });
    let menu = m.menu().unwrap();
    assert!(labels(&menu).contains(&(Some(MenuAction::CopyFlipped), "Copy as a sticker".into())));
    assert!(labels(&menu).contains(&(Some(MenuAction::Keep), "Keep offline".into())));
    let cmds = m.handle(Event::MenuChosen(MenuAction::CopyFlipped));
    assert!(matches!(&cmds[..], [Command::Copy { item, sticker: true, .. }] if item.id == "h1"));

    let mut m = with_results(3);
    m.handle(Event::OpenMenu(Some(1)));
    m.handle(Event::KeptReasons {
        item_id: "h1".into(),
        outcome: Ok(vec![]),
    });
    assert!(m.menu().is_some());
    m.handle(Event::Navigate(Nav::Right));
    assert!(m.menu().is_none(), "moving closes it");
    m.handle(Event::OpenMenu(Some(1)));
    m.handle(Event::CloseMenu);
    m.handle(Event::KeptReasons {
        item_id: "h1".into(),
        outcome: Ok(vec![]),
    });
    assert!(m.menu().is_none(), "closed stays closed");
}

#[test]
fn the_menu_of_a_pack_keeps_it_after_the_estimate() {
    let mut m = on_packs();
    assert!(m.handle(Event::OpenMenu(Some(0))).is_empty());
    let menu = m.menu().unwrap();
    assert_eq!(
        labels(&menu),
        [
            (Some(MenuAction::OpenPack), "Open the pack".to_string()),
            (Some(MenuAction::KeepPack), "Keep Cats offline...".into()),
        ]
    );
    let cmds = m.handle(Event::MenuChosen(MenuAction::KeepPack));
    assert_eq!(
        cmds,
        [Command::EstimateKeep {
            scope: KeepScope::Set("s1".into())
        }]
    );

    m.handle(Event::OfflineKeeps(vec![KeepScope::Set("s1".into())]));
    m.handle(Event::OpenMenu(Some(0)));
    let menu = m.menu().unwrap();
    assert_eq!(menu.entries[1].action, Some(MenuAction::ReleasePack));
    assert_eq!(menu.entries[1].label, "Stop keeping Cats offline");
    let cmds = m.handle(Event::MenuChosen(MenuAction::ReleasePack));
    assert_eq!(set_keep(&cmds), Some((KeepScope::Set("s1".into()), false)));

    // Inside the pack, the item's menu has the pack too.
    m.handle(Event::OpenMenu(Some(0)));
    let cmds = m.handle(Event::MenuChosen(MenuAction::OpenPack));
    let generation = cmds
        .iter()
        .find_map(|c| match c {
            Command::LoadPackItems { generation, .. } => Some(*generation),
            _ => None,
        })
        .unwrap();
    m.handle(Event::PackItemsLoaded {
        set_id: "s1".into(),
        generation,
        outcome: Ok(items(2)),
    });
    m.handle(Event::OpenMenu(Some(0)));
    m.handle(Event::KeptReasons {
        item_id: "0".into(),
        outcome: Ok(vec![KeptFor::Set]),
    });
    let menu = m.menu().unwrap();
    assert_eq!(
        menu.entries.last().map(|e| (e.action, e.label.as_str())),
        Some((Some(MenuAction::ReleasePack), "Stop keeping Cats offline"))
    );
}

// -- marks ---------------------------------------------------------------------------

fn marked() -> Model {
    let mut m = Model::new(ModelConfig {
        tiles: Tiles {
            favorites: false,
            ..Tiles::default()
        },
        ..cfg()
    });
    m.handle(Event::Open(OpenInfo::default()));
    m
}

#[test]
fn marks_show_what_is_on_the_device_and_zo_hides_them() {
    let mut m = marked();
    m.handle(Event::QueryChanged("cat".into()));
    let generation = m.generation();
    let cmds = m.handle(Event::Results {
        generation,
        outcome: Ok(Results {
            hits: hits(3),
            offline: false,
        }),
    });
    let (check, ids, target, sticker) = check_of(&cmds).expect("the marks are asked for");
    assert_eq!(ids, ["h0", "h1", "h2"]);
    assert_eq!((target, sticker), (TargetApp::Generic, false));
    assert_eq!(marks(&m), [OfflineMark::None; 3], "unknown until answered");
    let revision = m.entries_revision();
    m.handle(Event::OfflineChecked {
        generation: check,
        outcome: Ok(vec![("h0".into(), true), ("h1".into(), false), ("h2".into(), true)]),
    });
    assert!(m.entries_revision() > revision, "drawn again");
    assert_eq!(
        marks(&m),
        [OfflineMark::Available, OfflineMark::None, OfflineMark::Available]
    );
    assert!(m.entries()[0].tooltip().contains("\nAvailable offline"));

    normal(&mut m);
    let cmds = keys(&mut m, "zo");
    assert!(cmds.contains(&Command::SaveTile {
        tile: Tile::Offline,
        on: false
    }));
    assert_eq!(m.status().text(), "Offline marks hidden");
    assert_eq!(marks(&m), [OfflineMark::None; 3]);
    keys(&mut m, "zo");
    assert_eq!(marks(&m)[0], OfflineMark::Available, "known still");
}

/// The mark of what is available offline is for while the server answers;
/// without it, the tiles whose file is not here are dimmed and the others
/// are plain (as on Android).
#[test]
fn the_available_mark_shows_only_while_the_server_answers() {
    let mut m = marked();
    m.handle(Event::QueryChanged("cat".into()));
    let generation = m.generation();
    let cmds = m.handle(Event::Results {
        generation,
        outcome: Ok(Results {
            hits: hits(2),
            offline: false,
        }),
    });
    let (check, ..) = check_of(&cmds).unwrap();
    m.handle(Event::OfflineChecked {
        generation: check,
        outcome: Ok(vec![("h0".into(), true), ("h1".into(), false)]),
    });
    assert_eq!(marks(&m), [OfflineMark::Available, OfflineMark::None]);
    m.handle(Event::Reachable(false));
    assert_eq!(marks(&m), [OfflineMark::None, OfflineMark::Unavailable]);
    assert!(!m.entries()[0].tooltip().contains("Available offline"));
    m.handle(Event::Reachable(true));
    assert_eq!(marks(&m), [OfflineMark::Available, OfflineMark::None]);
}

#[test]
fn unreachable_dims_what_is_not_on_the_device_whatever_zo_says() {
    let mut m = with_results(3);
    assert!(m.tiles().offline.eq(&false));
    // Marks off and the server answering: nothing is asked.
    assert!(check_of(&m.handle(Event::OfflineFiles)).is_none());
    let cmds = m.handle(Event::Reachable(false));
    let (check, ..) = check_of(&cmds).expect("asked once the server is gone");
    m.handle(Event::OfflineChecked {
        generation: check,
        outcome: Ok(vec![("h0".into(), true), ("h1".into(), false), ("h2".into(), false)]),
    });
    assert_eq!(
        marks(&m),
        [OfflineMark::None, OfflineMark::Unavailable, OfflineMark::Unavailable]
    );
    assert!(m.entries()[1]
        .tooltip()
        .contains("\nNot on this device, and the server is unreachable"));
    assert!(!m.reachable());

    // The server back: no dimming.
    m.handle(Event::Reachable(true));
    assert_eq!(marks(&m), [OfflineMark::None; 3]);

    // Results from the cache mean the server did not answer.
    m.handle(Event::QueryChanged("dog".into()));
    let generation = m.generation();
    let cmds = m.handle(Event::Results {
        generation,
        outcome: Ok(Results {
            hits: hits(2),
            offline: true,
        }),
    });
    assert!(!m.reachable());
    let (check, ..) = check_of(&cmds).expect("asked");
    m.handle(Event::OfflineChecked {
        generation: check,
        outcome: Ok(vec![("h0".into(), true), ("h1".into(), false)]),
    });
    assert_eq!(marks(&m), [OfflineMark::None, OfflineMark::Unavailable]);
}

#[test]
fn one_check_at_a_time_and_stale_answers_are_dropped() {
    let mut m = marked();
    m.handle(Event::QueryChanged("cat".into()));
    let generation = m.generation();
    let cmds = m.handle(Event::Results {
        generation,
        outcome: Ok(Results {
            hits: hits(2),
            offline: false,
        }),
    });
    let (first, ..) = check_of(&cmds).unwrap();
    // Files arrive and the target changes meanwhile: no second check yet.
    assert!(check_of(&m.handle(Event::OfflineFiles)).is_none());
    assert!(check_of(&m.handle(Event::SetTarget(TargetApp::Telegram))).is_none());
    // The answer is for the old target: not used, and the check goes again.
    let cmds = m.handle(Event::OfflineChecked {
        generation: first,
        outcome: Ok(vec![("h0".into(), true)]),
    });
    assert_eq!(marks(&m), [OfflineMark::None; 2]);
    let (second, _, target, _) = check_of(&cmds).expect("checked again");
    assert_eq!(target, TargetApp::Telegram);
    // An answer to an older check does nothing.
    assert!(m
        .handle(Event::OfflineChecked {
            generation: first,
            outcome: Ok(vec![("h1".into(), true)]),
        })
        .is_empty());
    m.handle(Event::OfflineChecked {
        generation: second,
        outcome: Ok(vec![("h1".into(), true)]),
    });
    assert_eq!(marks(&m), [OfflineMark::None, OfflineMark::Available]);
    // A failure (the demo, a broken cache) leaves the marks out.
    let (third, ..) = check_of(&m.handle(Event::ToggleSticker)).unwrap();
    m.handle(Event::OfflineChecked {
        generation: third,
        outcome: Err("not in demo mode".into()),
    });
    assert_eq!(marks(&m), [OfflineMark::None; 2]);
}

#[test]
fn a_hidden_popup_asks_nothing() {
    let mut m = marked();
    m.handle(Event::RecentLoaded(items(3)));
    m.handle(Event::Hide);
    assert!(m.handle(Event::OfflineFiles).is_empty());
    assert!(m.handle(Event::Reachable(false)).is_empty());
}

#[test]
fn the_help_lists_the_keys() {
    let all: Vec<&str> = crate::vim::HELP
        .iter()
        .flat_map(|s| s.keys.iter().map(|(k, _)| *k))
        .collect();
    assert!(all.contains(&"K"));
    assert!(all.contains(&"zo"));
    assert!(all.iter().any(|k| k.contains("Shift+F10")));
}
