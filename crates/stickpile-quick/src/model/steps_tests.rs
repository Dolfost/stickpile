//! Results without waiting (docs/offline.md): the offline index's results
//! first with the notice, the server's after them in the same generation,
//! replacing them in place without a jump (rule 6 of the instant-search
//! round), and the server back. How soon the first step comes is the
//! core's (`OFFLINE_AFTER`); the desktop backend's tests time it.

use super::tests::{cfg, item, opened, search_of};
use super::*;

fn hit(id: &str) -> SearchHit {
    SearchHit {
        item: item(id),
        score: 1.0,
        matched: Vec::new(),
        incompatible: None,
    }
}

fn results(ids: &[&str], offline: bool) -> Results {
    Results {
        hits: ids.iter().map(|id| hit(id)).collect(),
        offline,
    }
}

fn ids(m: &Model) -> Vec<&str> {
    m.entries().iter().map(|e| e.item.id.as_str()).collect()
}

fn selected_id(m: &Model) -> Option<&str> {
    m.selected_entry().map(|e| e.item.id.as_str())
}

fn copied(cmds: &[Command]) -> Option<&str> {
    cmds.iter().find_map(|c| match c {
        Command::Copy { item, .. } => Some(item.id.as_str()),
        _ => None,
    })
}

fn loads_thumbnails(cmds: &[Command]) -> bool {
    cmds.iter().any(|c| matches!(c, Command::LoadThumbnails { .. }))
}

/// "cat" typed, and the offline index's `ids` shown first: the server had
/// not answered within the delay.
fn offline_first(ids: &[&str]) -> Model {
    let mut m = opened(cfg(), None);
    m.handle(Event::QueryChanged("cat".into()));
    let generation = m.generation();
    m.handle(Event::Results {
        generation,
        outcome: Ok(results(ids, true)),
    });
    m
}

/// The server's answer to the shown search.
fn server(m: &mut Model, ids: &[&str]) -> Vec<Command> {
    let generation = m.generation();
    m.handle(Event::ServerResults {
        generation,
        outcome: Ok(Some(results(ids, false))),
    })
}

#[test]
fn two_step_results_replace_in_place_keeping_selection() {
    let mut m = offline_first(&["a", "b", "c", "d"]);
    m.handle(Event::Navigate(Nav::Right));
    m.handle(Event::Navigate(Nav::Right));
    assert_eq!(selected_id(&m), Some("c"));

    let cmds = server(&mut m, &["x", "c", "a", "y", "b"]);
    assert_eq!(ids(&m), ["x", "c", "a", "y", "b"], "the server's results, in its order");
    assert_eq!(selected_id(&m), Some("c"), "the selected sticker stays selected");
    assert_eq!(m.selected(), Some(1), "where it is now");
    assert!(!m.is_offline());
    assert!(loads_thumbnails(&cmds), "the stickers the cache did not hold");
    assert_eq!(m.query(), "cat");

    // Not moved by the user: the sticker selected first stays selected too
    // (Enter copies what was seen selected).
    let mut m = offline_first(&["a", "b"]);
    assert_eq!(selected_id(&m), Some("a"));
    server(&mut m, &["b", "a"]);
    assert_eq!(selected_id(&m), Some("a"));
    assert_eq!(m.selected(), Some(1));
}

/// When the selected sticker is not among the server's results: the same
/// place in the grid if the user moved the selection (what fresh data for
/// a shown list does today, `Sel::Refresh`), else the first, the server's
/// best match; nothing selected when nothing is left.
#[test]
fn selected_sticker_gone_from_server_results() {
    let mut m = offline_first(&["a", "b", "c"]);
    m.handle(Event::Navigate(Nav::Right));
    assert_eq!(selected_id(&m), Some("b"));
    server(&mut m, &["w", "x", "y", "z"]);
    assert_eq!(m.selected(), Some(1), "the same place");
    assert_eq!(selected_id(&m), Some("x"));

    let mut m = offline_first(&["a", "b", "c"]);
    m.handle(Event::Navigate(Nav::End));
    assert_eq!(selected_id(&m), Some("c"));
    server(&mut m, &["x", "y"]);
    assert_eq!(m.selected(), Some(1), "the same place, within the new list");

    let mut m = offline_first(&["a", "b"]);
    server(&mut m, &["x", "y"]);
    assert_eq!(m.selected(), Some(0), "not moved: the server's first");

    let mut m = offline_first(&["a"]);
    server(&mut m, &[]);
    assert_eq!(m.selected(), None);
    assert_eq!(m.status(), &Status::NoResults { query: "cat".into() });
}

#[test]
fn offline_notice_shows_at_once_and_goes_when_server_answers() {
    let mut m = opened(cfg(), None);
    m.handle(Event::QueryChanged("cat".into()));
    let generation = m.generation();
    m.handle(Event::Results {
        generation,
        outcome: Ok(results(&["a", "b"], true)),
    });
    // With the offline results, before the server's answer.
    assert_eq!(m.status(), &Status::Offline);
    let f = m.footer();
    assert_eq!(f.status, "Offline: showing saved stickers");
    assert_eq!(f.level, Level::Warning);
    assert!(f.offline);
    assert_eq!(
        stickpile_i18n::t(stickpile_i18n::Lang::Uk, "Offline: showing saved stickers"),
        "Немає зв'язку: показано збережені стікери"
    );

    server(&mut m, &["b", "a"]);
    assert_eq!(m.status(), &Status::Idle, "the notice goes with the offline results");
    let f = m.footer();
    assert!(!f.offline);
    assert_eq!(f.status, "");

    // The server gave none: the offline results and the notice stand.
    let mut m = offline_first(&["a", "b"]);
    let generation = m.generation();
    assert!(m
        .handle(Event::ServerResults {
            generation,
            outcome: Ok(None),
        })
        .is_empty());
    assert_eq!(ids(&m), ["a", "b"]);
    assert_eq!(m.status(), &Status::Offline);

    // An error says why it gave none; the offline results stand.
    m.handle(Event::ServerResults {
        generation,
        outcome: Err("The server did not accept this client".into()),
    });
    assert_eq!(ids(&m), ["a", "b"]);
    assert_eq!(
        m.status(),
        &Status::Error("Search failed: The server did not accept this client".into())
    );
}

/// "Searching..." lasts from the keystroke until the first step: the
/// server's answer still to come is not waited for, nor is a search run
/// again in place. The first step comes within the debounce and the
/// core's delay (300 ms), at once when the server is believed away.
#[test]
fn searching_status_never_longer_than_offline_delay() {
    let mut m = opened(cfg(), None);
    let cmds = m.handle(Event::QueryChanged("cat".into()));
    let (_, _, generation, debounce) = search_of(&cmds).unwrap();
    assert_eq!(debounce, DEFAULT_DEBOUNCE_MS);
    assert_eq!(m.status(), &Status::Searching);
    m.handle(Event::Results {
        generation,
        outcome: Ok(results(&["a", "b"], true)),
    });
    assert_eq!(m.status(), &Status::Offline);
    // Going about while the server is asked says nothing of searching.
    m.handle(Event::Navigate(Nav::Right));
    m.handle(Event::ShowTab(Tab::Recent));
    m.handle(Event::ShowTab(Tab::Search));
    assert_eq!(m.status(), &Status::Offline);
    server(&mut m, &["a"]);
    assert_eq!(m.status(), &Status::Idle);

    // The server back: searched again without "Searching" over the results.
    let mut m = offline_first(&["a", "b"]);
    m.handle(Event::Reachable(false));
    let cmds = m.handle(Event::Reachable(true));
    assert!(search_of(&cmds).is_some());
    assert_eq!(m.status(), &Status::Offline);
    assert_ne!(m.footer().level, Level::Busy);
}

#[test]
fn server_back_reruns_search_and_starts_sync() {
    let mut m = offline_first(&["a", "b", "c"]);
    m.handle(Event::Navigate(Nav::Right));
    let cmds = m.handle(Event::Reachable(false));
    assert!(search_of(&cmds).is_none() && !cmds.contains(&Command::Sync));

    let cmds = m.handle(Event::Reachable(true));
    assert!(cmds.contains(&Command::Sync));
    let (query, _, generation, debounce) = search_of(&cmds).expect("searched again");
    assert_eq!((query.as_str(), debounce), ("cat", 0));
    assert_eq!(generation, m.generation());
    assert_eq!(ids(&m), ["a", "b", "c"], "shown until the new results come");
    assert_eq!(m.status(), &Status::Offline);

    // The server answers: in place, the selection kept, the notice gone.
    m.handle(Event::Results {
        generation,
        outcome: Ok(results(&["c", "b", "x"], false)),
    });
    assert_eq!(ids(&m), ["c", "b", "x"]);
    assert_eq!(selected_id(&m), Some("b"));
    assert_eq!(m.status(), &Status::Idle);
    assert!(!m.is_offline());

    // Nothing was missed: nothing to do.
    assert!(m.handle(Event::Reachable(true)).is_empty());
    // An outage with the server's results shown: a sync only.
    m.handle(Event::Reachable(false));
    assert_eq!(m.handle(Event::Reachable(true)), [Command::Sync]);

    // A search sent while the server was away, not answered yet: again.
    let mut m = opened(cfg(), None);
    m.handle(Event::Reachable(false));
    m.handle(Event::QueryChanged("dog".into()));
    let old = m.generation();
    let cmds = m.handle(Event::Reachable(true));
    let (_, _, generation, _) = search_of(&cmds).expect("searched again");
    assert!(generation > old);
    assert_eq!(m.status(), &Status::Searching, "nothing shown yet");

    // Hidden: a sync, and no search.
    let mut m = offline_first(&["a"]);
    m.handle(Event::Hide);
    m.handle(Event::Reachable(false));
    assert_eq!(m.handle(Event::Reachable(true)), [Command::Sync]);

    // A sync that changed the cache searches offline results again in
    // place too.
    let mut m = offline_first(&["a", "b"]);
    m.handle(Event::Navigate(Nav::Right));
    let cmds = m.handle(Event::LibraryChanged);
    let (_, _, generation, _) = search_of(&cmds).expect("searched again");
    assert_eq!(m.status(), &Status::Offline);
    m.handle(Event::Results {
        generation,
        outcome: Ok(results(&["b", "a", "n"], true)),
    });
    assert_eq!(ids(&m), ["b", "a", "n"]);
    assert_eq!(selected_id(&m), Some("b"));
}

#[test]
fn server_results_for_an_old_generation_are_ignored() {
    let mut m = offline_first(&["a", "b"]);
    let old = m.generation();
    m.handle(Event::QueryChanged("dog".into()));
    let late = |m: &mut Model| {
        m.handle(Event::ServerResults {
            generation: old,
            outcome: Ok(Some(results(&["x"], false))),
        })
    };
    assert!(late(&mut m).is_empty());
    assert_eq!(ids(&m), ["a", "b"], "the shown results wait for the new search's");
    let generation = m.generation();
    m.handle(Event::Results {
        generation,
        outcome: Ok(results(&["d"], true)),
    });
    assert!(late(&mut m).is_empty());
    assert_eq!(ids(&m), ["d"]);
    assert!(m.is_offline());

    // Nor once the query is cleared.
    m.handle(Event::QueryChanged(String::new()));
    let generation = m.generation();
    assert!(m
        .handle(Event::ServerResults {
            generation,
            outcome: Ok(Some(results(&["x"], false))),
        })
        .is_empty());
    assert!(ids(&m).is_empty(), "Recent, empty");

    // Nor the answer of a search before one run again in place.
    let mut m = offline_first(&["a"]);
    let before = m.generation();
    m.handle(Event::Reachable(false));
    m.handle(Event::Reachable(true));
    m.handle(Event::ServerResults {
        generation: before,
        outcome: Ok(Some(results(&["x"], false))),
    });
    assert_eq!(ids(&m), ["a"]);
}

/// The swap waits until the press ends (rather than keeping the pressed
/// tile in place): the click the press makes acts on the tile pressed.
#[test]
fn swap_deferred_while_a_tile_is_pressed() {
    let mut m = offline_first(&["a", "b", "c"]);
    assert!(m.handle(Event::Pressing(true)).is_empty());
    let cmds = server(&mut m, &["c", "x", "a"]);
    assert!(cmds.is_empty());
    assert_eq!(ids(&m), ["a", "b", "c"], "nothing moves under the press");
    assert_eq!(m.status(), &Status::Offline);
    let cmds = m.handle(Event::CopyAt {
        index: 2,
        toggle_sticker: false,
    });
    assert_eq!(copied(&cmds), Some("c"), "the tile it pressed");

    let cmds = m.handle(Event::Pressing(false));
    assert_eq!(ids(&m), ["c", "x", "a"]);
    assert_eq!(selected_id(&m), Some("c"));
    assert!(loads_thumbnails(&cmds));
    assert!(m.is_copying(), "the copy goes on");

    // An open menu of a tile holds them too (its entries act on that tile).
    let mut m = offline_first(&["a", "b"]);
    m.handle(Event::OpenMenu(Some(1)));
    assert!(server(&mut m, &["b", "x"]).is_empty());
    assert_eq!(ids(&m), ["a", "b"]);
    m.handle(Event::CloseMenu);
    assert_eq!(ids(&m), ["b", "x"]);
    assert_eq!(selected_id(&m), Some("b"));

    // What a press held for a search typed over meanwhile is dropped.
    let mut m = offline_first(&["a"]);
    m.handle(Event::Pressing(true));
    server(&mut m, &["x"]);
    m.handle(Event::QueryChanged("dog".into()));
    m.handle(Event::Pressing(false));
    assert_eq!(ids(&m), ["a"]);

    // A press the window lost (hidden) holds nothing after the next opening.
    let mut m = offline_first(&["a"]);
    m.handle(Event::Pressing(true));
    m.handle(Event::Hide);
    m.handle(Event::Open(OpenInfo::default()));
    m.handle(Event::QueryChanged("cat".into()));
    let generation = m.generation();
    m.handle(Event::Results {
        generation,
        outcome: Ok(results(&["a"], true)),
    });
    server(&mut m, &["y"]);
    assert_eq!(ids(&m), ["y"]);
}

#[test]
fn copy_in_progress_not_dropped_by_the_swap() {
    let mut m = offline_first(&["a", "b"]);
    let cmds = m.handle(Event::CopySelected { toggle_sticker: false });
    assert_eq!(copied(&cmds), Some("a"));
    assert_eq!(m.status(), &Status::Copying { name: "item-a".into() });

    server(&mut m, &["x", "y"]);
    assert_eq!(ids(&m), ["x", "y"], "the sticker being copied is not among them");
    assert!(m.is_copying());
    assert_eq!(m.status(), &Status::Copying { name: "item-a".into() });

    let cmds = m.handle(Event::CopyFinished {
        item_id: "a".into(),
        warning: None,
    });
    assert!(matches!(m.status(), Status::Copied { name, .. } if name == "item-a"));
    assert!(cmds.contains(&Command::Hide));
}
