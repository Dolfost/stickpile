//! The values offered under the search field, Tab completion, filters
//! resolved for the search, the syntax help (`g/`, F2) and the stats row
//! (`zi`).

use super::tests::{cfg, items, opened};
use super::*;
use crate::vim::NormalKey;

fn vocab() -> Vocabulary {
    Vocabulary {
        vibes: ["sad", "sarcastic", "happy"].map(String::from).to_vec(),
        tags: ["reaction", "nsfw"].map(String::from).to_vec(),
        ..Vocabulary::default()
    }
}

/// Opened on Search in Insert mode, with the vocabulary known.
fn search() -> Model {
    let mut m = opened(cfg(), None);
    m.handle(Event::VocabularyLoaded(vocab()));
    assert_eq!((m.tab(), m.mode()), (Tab::Search, Mode::Insert));
    m
}

fn sent(cmds: &[Command]) -> Option<(String, u64)> {
    cmds.iter().find_map(|c| match c {
        Command::Search { query, debounce_ms, .. } => Some((query.clone(), *debounce_ms)),
        _ => None,
    })
}

fn tab(m: &mut Model, backwards: bool) -> Vec<Command> {
    m.handle(Event::CycleTab { backwards })
}

#[test]
fn a_filter_being_typed_offers_its_values_and_searches_the_best() {
    let mut m = search();
    assert_eq!(m.suggestions(), None);
    let cmds = m.handle(Event::QueryChanged("cat vibe:".into()));
    assert_eq!(
        m.suggestions(),
        Some(SuggestionLine {
            key: "vibe:".into(),
            words: vec!["sad".into(), "sarcastic".into(), "happy".into()],
            chosen: None,
            labels: vec![String::new(); 3],
        })
    );
    assert_eq!(sent(&cmds).unwrap().0, "cat", "an empty filter is left out");

    let cmds = m.handle(Event::QueryChanged("cat vibe:sarc".into()));
    assert_eq!(m.suggestions().unwrap().words, ["sarcastic"]);
    assert_eq!(sent(&cmds), Some(("cat vibe:sarcastic".into(), cfg().debounce_ms)));
    assert_eq!(m.query(), "cat vibe:sarc", "the field keeps what was typed");

    let cmds = m.handle(Event::QueryChanged("cat vibe:sarcstic".into()));
    assert_eq!(sent(&cmds).unwrap().0, "cat vibe:sarcastic", "a typo finds it too");

    m.handle(Event::QueryChanged("cat vibe:sarcastic ".into()));
    assert_eq!(m.suggestions(), None, "a space finishes the filter");
}

#[test]
fn emojis_are_offered_with_their_names_and_names_searched_as_typed() {
    let mut m = opened(cfg(), None);
    m.handle(Event::VocabularyLoaded(Vocabulary {
        emojis: vec!["😂".into(), "💀".into()],
        ..vocab()
    }));
    let cmds = m.handle(Event::QueryChanged("cat e:".into()));
    assert_eq!(
        m.suggestions(),
        Some(SuggestionLine {
            key: "e:".into(),
            words: vec!["😂".into(), "💀".into()],
            chosen: None,
            labels: vec!["face with tears of joy".into(), "skull".into()],
        })
    );
    assert_eq!(sent(&cmds).unwrap().0, "cat", "an empty filter is left out");

    let cmds = m.handle(Event::QueryChanged("cat e:sk".into()));
    let line = m.suggestions().unwrap();
    assert_eq!(line.words[..2], ["💀", "☠️"]);
    assert_eq!(line.labels[..2], ["skull", "skull and crossbones"]);
    assert_eq!(line.chosen, None, "a name is searched as typed");
    assert_eq!(sent(&cmds).unwrap().0, "cat e:sk");

    let cmds = tab(&mut m, false);
    assert_eq!(m.query(), "cat e:💀", "Tab puts the emoji in");
    assert_eq!(sent(&cmds).unwrap().0, "cat e:💀");
    tab(&mut m, false);
    assert_eq!(m.query(), "cat e:☠️");

    m.handle(Event::QueryChanged("cat e:skul".into()));
    let cmds = m.handle(Event::AcceptSuggestion(1));
    assert_eq!(m.query(), "cat e:☠️ ", "a click puts it in and finishes the filter");
    assert_eq!(sent(&cmds).unwrap().0, "cat e:☠️");
    m.handle(Event::QueryChanged("cat e:💀".into()));
    assert_eq!(m.suggestions().unwrap().chosen, Some(0), "the emoji typed is chosen");
}

#[test]
fn tab_puts_the_values_in_one_after_another() {
    let mut m = search();
    m.handle(Event::QueryChanged("vibe:sa".into()));
    let cmds = tab(&mut m, false);
    assert_eq!(m.tab(), Tab::Search, "Tab completes instead of switching tabs");
    assert_eq!(m.query(), "vibe:sad");
    assert_eq!(sent(&cmds), Some(("vibe:sad".into(), 0)), "searched at once");
    assert_eq!(m.suggestions().unwrap().chosen, Some(0));
    tab(&mut m, false);
    assert_eq!(m.query(), "vibe:sarcastic");
    assert_eq!(
        m.suggestions().unwrap().words,
        ["sad", "sarcastic"],
        "the line stays as it was while Tab goes through it"
    );
    tab(&mut m, false);
    assert_eq!(m.query(), "vibe:sad", "wraps");
    tab(&mut m, true);
    assert_eq!(m.query(), "vibe:sarcastic", "Shift+Tab goes back");

    // Typing ends it: the line follows the text again.
    m.handle(Event::QueryChanged("vibe:sarcastic t".into()));
    assert_eq!(m.suggestions(), None);
    tab(&mut m, false);
    assert_eq!(m.tab(), Tab::Recent, "nothing offered: Tab switches tabs");
}

#[test]
fn tab_on_a_value_already_typed_moves_on() {
    let mut m = search();
    m.handle(Event::QueryChanged("x vibe:sad".into()));
    tab(&mut m, false);
    assert_eq!(m.query(), "x vibe:sarcastic");
}

#[test]
fn tab_after_an_empty_filter_starts_at_either_end() {
    let mut m = search();
    m.handle(Event::QueryChanged("vibe:".into()));
    assert_eq!(m.suggestions().unwrap().chosen, None);
    tab(&mut m, false);
    assert_eq!(m.query(), "vibe:sad");
    let mut m = search();
    m.handle(Event::QueryChanged("vibe:".into()));
    tab(&mut m, true);
    assert_eq!(m.query(), "vibe:happy", "Shift+Tab: the last");
}

#[test]
fn tab_in_normal_mode_still_switches_tabs() {
    let mut m = search();
    m.handle(Event::QueryChanged("vibe:sa".into()));
    m.handle(Event::Escape);
    assert_eq!(m.mode(), Mode::Normal);
    tab(&mut m, false);
    assert_eq!(m.tab(), Tab::Recent);
    assert_eq!(m.suggestions(), None, "only on the Search tab");
}

#[test]
fn a_click_puts_the_value_in_and_finishes_the_filter() {
    let mut m = search();
    m.handle(Event::QueryChanged("cat -tag:".into()));
    assert_eq!(m.suggestions().unwrap().key, "-tag:");
    let cmds = m.handle(Event::AcceptSuggestion(1));
    assert_eq!(m.query(), "cat -tag:nsfw ");
    assert_eq!(sent(&cmds), Some(("cat -tag:nsfw".into(), 0)));
    assert_eq!(m.suggestions(), None);
    assert!(m.handle(Event::AcceptSuggestion(0)).is_empty(), "nothing offered now");
}

#[test]
fn values_known_later_send_the_search_again() {
    let mut m = opened(cfg(), None);
    let cmds = m.handle(Event::QueryChanged("vibe:sarc".into()));
    assert_eq!(sent(&cmds).unwrap().0, "vibe:sarc", "nothing known yet: as typed");
    let cmds = m.handle(Event::VocabularyLoaded(vocab()));
    assert_eq!(sent(&cmds), Some(("vibe:sarcastic".into(), 0)));
    assert!(
        m.handle(Event::VocabularyLoaded(vocab())).is_empty(),
        "the same values: no new search"
    );
}

#[test]
fn every_opening_fetches_the_values() {
    let mut m = Model::new(cfg());
    let cmds = m.handle(Event::Open(OpenInfo::default()));
    assert!(cmds.contains(&Command::LoadVocabulary));
}

#[test]
fn safe_mode_joins_a_resolved_query() {
    let mut m = search();
    m.handle(Event::ToggleSafe);
    let cmds = m.handle(Event::QueryChanged("vibe:".into()));
    assert_eq!(sent(&cmds).unwrap().0, "-tag:nsfw");
    let cmds = m.handle(Event::QueryChanged("vibe:ha".into()));
    assert_eq!(sent(&cmds).unwrap().0, "vibe:happy -tag:nsfw");
}

#[test]
fn f2_and_g_slash_show_the_search_syntax() {
    let mut m = search();
    m.handle(Event::ToggleSyntax);
    assert_eq!(m.help_page(), Some(HelpPage::Syntax));
    assert!(m.help_visible());
    m.handle(Event::ToggleHelp);
    assert_eq!(m.help_page(), Some(HelpPage::Keys), "F1 switches to the keys");
    m.handle(Event::ToggleHelp);
    assert_eq!(m.help_page(), None);

    m.handle(Event::ToggleSyntax);
    m.handle(Event::Escape);
    assert_eq!(
        (m.help_page(), m.mode()),
        (None, Mode::Insert),
        "Esc closes it first and typing goes on"
    );
    m.handle(Event::Escape);
    for c in "g/".chars() {
        m.handle(Event::Normal(NormalKey::Char(c)));
    }
    assert_eq!(m.help_page(), Some(HelpPage::Syntax));
    m.handle(Event::Normal(NormalKey::Escape));
    assert_eq!(m.help_page(), None, "Esc closes it");
    m.handle(Event::ToggleSyntax);
    m.handle(Event::Hide);
    m.handle(Event::Open(OpenInfo::default()));
    assert_eq!(m.help_page(), None, "an opening starts without it");
}

#[test]
fn zi_hides_the_stats_row_and_saves_it() {
    let mut m = Model::new(ModelConfig {
        start_tab: Some(Tab::Recent),
        ..cfg()
    });
    m.handle(Event::Open(OpenInfo::default()));
    m.handle(Event::RecentLoaded(items(2)));
    let f = m.footer();
    assert!(f.details_row && !f.details.is_empty());
    let cmds: Vec<_> = "zi"
        .chars()
        .flat_map(|c| m.handle(Event::Normal(NormalKey::Char(c))))
        .collect();
    assert!(cmds.contains(&Command::SaveTile {
        tile: Tile::Details,
        on: false
    }));
    let f = m.footer();
    assert!(!f.details_row);
    assert_eq!(f.details, "");
    assert_eq!(f.status, "Item stats hidden");
    for c in "zi".chars() {
        m.handle(Event::Normal(NormalKey::Char(c)));
    }
    assert!(m.footer().details_row);
    assert_eq!(m.footer().status, "Item stats shown");
}
