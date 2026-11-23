//! Review of the instant-search round: rule 6, "the grid is not reordered
//! under a pointer or a finger pressing a tile" (docs/offline.md, "What the
//! shells do").

use super::tests::{cfg, item, opened};
use super::*;

fn results(ids: &[&str], offline: bool) -> Results {
    Results {
        hits: ids
            .iter()
            .map(|id| SearchHit {
                item: item(id),
                score: 1.0,
                matched: Vec::new(),
                incompatible: None,
            })
            .collect(),
        offline,
    }
}

fn ids(m: &Model) -> Vec<&str> {
    m.entries().iter().map(|e| e.item.id.as_str()).collect()
}

/// The results of a search typed further ("ca", then "cat") come while a
/// button is held on a tile of the shown ones: the grid changes under the
/// press, and the click it makes (GTK activates on the release) copies
/// whatever tile came under the pointer. The app and the keyboard on
/// Android wait for the finger for a refined search (`LibraryViewModel.load`
/// with `refines`, `PickerView.loadSteps` with `keep`); the popup holds only
/// the server's results that replace offline ones.
#[test]
fn a_refined_search_does_not_reorder_the_grid_under_a_press() {
    let mut m = opened(cfg(), None);
    m.handle(Event::QueryChanged("ca".into()));
    let generation = m.generation();
    m.handle(Event::Results {
        generation,
        outcome: Ok(results(&["a", "b", "c"], false)),
    });
    assert_eq!(ids(&m), ["a", "b", "c"]);

    // A press on "c"; meanwhile "t" was typed and its results come.
    m.handle(Event::Pressing(true));
    m.handle(Event::QueryChanged("cat".into()));
    let generation = m.generation();
    m.handle(Event::Results {
        generation,
        outcome: Ok(results(&["x", "y", "z"], true)),
    });
    assert_eq!(ids(&m), ["a", "b", "c"], "the grid changed under the press");
}
