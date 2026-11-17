//! A sync that changed the offline cache ([`Event::LibraryChanged`]): the
//! values the search suggests load again, shown or hidden, and a
//! suggestion line on screen takes them in place.

use super::tests::{cfg, hits};
use super::*;

fn vibes() -> Vocabulary {
    Vocabulary {
        vibes: ["sad", "sarcastic", "happy"].map(String::from).to_vec(),
        ..Vocabulary::default()
    }
}

fn sent(cmds: &[Command]) -> Option<String> {
    cmds.iter().find_map(|c| match c {
        Command::Search { query, .. } => Some(query.clone()),
        _ => None,
    })
}

/// The first opening after a start: the cache is still empty, so the
/// values are none.
fn first_opening() -> Model {
    let mut m = Model::new(cfg());
    let cmds = m.handle(Event::Open(OpenInfo::default()));
    assert!(cmds.contains(&Command::LoadVocabulary));
    assert!(m.handle(Event::VocabularyLoaded(Vocabulary::default())).is_empty());
    m
}

#[test]
fn a_sync_fills_the_line_of_a_filter_already_typed() {
    let mut m = first_opening();
    let cmds = m.handle(Event::QueryChanged("cat v:".into()));
    assert_eq!(m.suggestions(), None, "nothing known yet");
    assert_eq!(sent(&cmds).as_deref(), Some("cat"));
    m.handle(Event::Results {
        generation: m.generation(),
        outcome: Ok(Results {
            hits: hits(3),
            offline: false,
        }),
    });
    m.handle(Event::Navigate(Nav::Right));
    assert_eq!(m.selected(), Some(1));

    assert_eq!(m.handle(Event::LibraryChanged), [Command::LoadVocabulary]);
    let cmds = m.handle(Event::VocabularyLoaded(vibes()));
    let line = m.suggestions().expect("the values are offered now");
    assert_eq!(line.key, "v:");
    assert_eq!(line.words, ["sad", "sarcastic", "happy"]);
    assert_eq!(line.chosen, None);
    assert_eq!(m.query(), "cat v:", "the query stays as typed");
    assert_eq!(
        sent(&cmds),
        None,
        "an empty filter is left out either way: no new search"
    );
    assert_eq!(m.selected(), Some(1), "the results and their selection stay");
    assert_eq!(m.entries().len(), 3);
}

#[test]
fn results_the_cache_answered_are_searched_again() {
    let mut m = first_opening();
    m.handle(Event::QueryChanged("cat v:".into()));
    m.handle(Event::Results {
        generation: m.generation(),
        outcome: Ok(Results {
            hits: Vec::new(),
            offline: true,
        }),
    });
    let cmds = m.handle(Event::LibraryChanged);
    assert!(cmds.contains(&Command::LoadVocabulary));
    assert_eq!(sent(&cmds).as_deref(), Some("cat"));
    assert_eq!(m.query(), "cat v:");
    // An answer to the search before is dropped.
    m.handle(Event::Results {
        generation: m.generation() - 1,
        outcome: Ok(Results {
            hits: hits(2),
            offline: true,
        }),
    });
    assert!(m.entries().is_empty());
}

#[test]
fn a_value_typed_before_the_sync_is_searched_as_it_resolves_now() {
    let mut m = first_opening();
    let cmds = m.handle(Event::QueryChanged("v:sarc".into()));
    assert_eq!(sent(&cmds).as_deref(), Some("vibe:sarc"), "nothing known yet: as typed");
    m.handle(Event::LibraryChanged);
    let cmds = m.handle(Event::VocabularyLoaded(vibes()));
    assert_eq!(sent(&cmds).as_deref(), Some("vibe:sarcastic"));
    assert_eq!(m.query(), "v:sarc");
    let line = m.suggestions().unwrap();
    assert_eq!(
        (line.words.as_slice(), line.chosen),
        (&["sarcastic".to_string()][..], Some(0))
    );
}

#[test]
fn a_sync_while_hidden_loads_the_values_too() {
    let mut m = Model::new(cfg());
    assert_eq!(
        m.handle(Event::LibraryChanged),
        [Command::LoadVocabulary],
        "never opened yet"
    );
    assert!(
        m.handle(Event::VocabularyLoaded(vibes())).is_empty(),
        "hidden: nothing searched"
    );

    m.handle(Event::Open(OpenInfo::default()));
    m.handle(Event::Hide);
    assert!(!m.is_visible());
    assert_eq!(m.handle(Event::LibraryChanged), [Command::LoadVocabulary]);
    let more = Vocabulary {
        vibes: vec!["tired".into()],
        ..vibes()
    };
    assert!(m.handle(Event::VocabularyLoaded(more)).is_empty());

    // The next opening offers them before its own answer comes.
    m.handle(Event::Open(OpenInfo::default()));
    m.handle(Event::QueryChanged("vibe:".into()));
    assert_eq!(m.suggestions().unwrap().words, ["tired"]);
}

#[test]
fn tab_going_through_the_line_keeps_it() {
    let mut m = first_opening();
    m.handle(Event::VocabularyLoaded(vibes()));
    m.handle(Event::QueryChanged("vibe:sa".into()));
    m.handle(Event::CycleTab { backwards: false });
    assert_eq!(m.query(), "vibe:sad");
    m.handle(Event::LibraryChanged);
    let more = Vocabulary {
        vibes: vec!["salty".into()],
        ..Vocabulary::default()
    };
    assert!(sent(&m.handle(Event::VocabularyLoaded(more))).is_none());
    let line = m.suggestions().unwrap();
    assert_eq!(
        (line.words.as_slice(), line.chosen),
        (&["sad".to_string(), "sarcastic".to_string()][..], Some(0))
    );
    assert_eq!(m.query(), "vibe:sad");
}
