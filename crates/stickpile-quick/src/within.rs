//! Search inside a list the client already holds, such as an opened pack:
//! the whole syntax of `stickpile_api::query` (`vibe:tired`, `t:reaction`,
//! `type:gif`, `-tag:nsfw`, `is:liked`, `q>70`, `sort:likes`, ...), matched
//! by the in-memory index the server and the offline search use
//! (`stickpile_search`), over these items only. No network.
//!
//! - Words are ranked as the search ranks them (typos forgiven, `naahh`
//!   finds `nah`), best first. As the pack filter always did, a word also
//!   finds the items with it anywhere in their name, a keyword, an emoji or
//!   a tag, so `c` and `at` already find "cat" while it is typed; those
//!   come after the ranked ones, in list order.
//! - The pack's own name is no word to search for (every item has it):
//!   only `set:` (its slug, name or id) looks at it.
//! - A query of filters only keeps the list's order; `sort:` other than
//!   relevance orders as the search does.
//! - `is:mine` compares the owner's username with the viewer's; `is:liked`
//!   looks in the ids given; `is:public` at the pack's visibility.

use std::collections::{HashMap, HashSet};

use stickpile_api::query::{Filter, Query, Sort};
use stickpile_api::{ItemSummary, Visibility};
use stickpile_search::{normalize, Doc, Index, SearchOptions, Viewer};

/// An item a search found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Its position in the list given to [`Within::new`].
    pub index: usize,
    /// The fields a word matched, as the search labels them (`name`,
    /// `keyword`, `emoji`, `vibe`, `tag`, `fuzzy`); empty for a filter.
    pub matched: Vec<String>,
}

/// Items indexed for [`Within::search`].
#[derive(Debug, Clone, Default)]
pub struct Within {
    index: Index,
    /// Item id to its position in the list.
    positions: HashMap<String, usize>,
    /// Per item, in list order: its name, keywords, emojis and tags, in
    /// lower case (the words found anywhere inside).
    texts: Vec<Vec<String>>,
    /// Per item, in list order: its set's slug and name normalised, and id.
    sets: Vec<[String; 3]>,
    /// The ids of the sets the items are in: all of them are visible here.
    set_ids: HashSet<String>,
}

impl Within {
    /// Index `items`. `visibility` is their pack's, for `is:public`; when it
    /// is not known an item counts as private when it is `me`'s and as
    /// public otherwise (as the offline cache counts them).
    pub fn new(items: &[ItemSummary], visibility: Option<Visibility>, me: Option<&str>) -> Within {
        let mut index = Index::new();
        for item in items {
            let mine = me.is_some_and(|m| m == item.owner_username);
            index.upsert(Doc {
                item_id: item.id.clone(),
                set_id: item.set_id.clone(),
                // Every item of a pack has its name: not a word to rank by.
                set_slug: String::new(),
                set_name: String::new(),
                owner_id: item.owner_username.clone(),
                owner_username: item.owner_username.clone(),
                visibility: visibility.unwrap_or(if mine { Visibility::Private } else { Visibility::Public }),
                name: item.name.clone(),
                keywords: item.keywords.clone(),
                emojis: item.emojis.clone(),
                vibe: item.vibe.clone(),
                tags: item.tags.clone(),
                kind: item.kind,
                format: item.format,
                quality: item.quality,
                likes: item.likes,
                copies: item.copies,
                media_id: item.media_id.clone(),
                embedding: None,
                updated_at: item.updated_at,
            });
        }
        Within {
            index,
            positions: items.iter().enumerate().map(|(i, it)| (it.id.clone(), i)).collect(),
            texts: items
                .iter()
                .map(|it| {
                    std::iter::once(&it.name)
                        .chain(&it.keywords)
                        .chain(&it.emojis)
                        .chain(&it.tags)
                        .map(|f| f.to_lowercase())
                        .collect()
                })
                .collect(),
            sets: items
                .iter()
                .map(|it| {
                    [
                        normalize(it.set_slug.trim()),
                        normalize(it.set_name.trim()),
                        it.set_id.clone(),
                    ]
                })
                .collect(),
            set_ids: items.iter().map(|it| it.set_id.clone()).collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.texts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.texts.is_empty()
    }

    /// The items `query` finds, in the order described in the module
    /// docs. `me`: the viewer's username (`is:mine`); `liked`: the ids of
    /// the items the viewer liked (`is:liked`).
    pub fn search(&self, query: &str, me: Option<&str>, liked: &HashSet<String>) -> Vec<Found> {
        if self.is_empty() {
            return Vec::new();
        }
        let mut q = Query::parse(query);
        // The docs have no set names (see `new`): `set:` is checked here.
        let sets = std::mem::take(&mut q.sets);
        let viewer = Viewer {
            user_id: me.unwrap_or_default().to_string(),
            liked: liked.clone(),
            member_sets: self.set_ids.clone(),
        };
        let opts = SearchOptions {
            limit: self.len(),
            ..Default::default()
        };
        let set_ok = |i: usize| sets_accept(&sets, &self.sets[i]);
        let position = |id: &str| self.positions.get(id).copied().filter(|i| set_ok(*i));

        // Every item the filters let through, in the order `sort:` asks for.
        let words = std::mem::take(&mut q.text);
        let passing: Vec<usize> = self
            .index
            .search(&q, &viewer, &opts)
            .hits
            .iter()
            .filter_map(|h| position(&h.item_id))
            .collect();
        if words.is_empty() {
            let mut found: Vec<Found> = passing
                .into_iter()
                .map(|index| Found {
                    index,
                    matched: Vec::new(),
                })
                .collect();
            if q.sort == Sort::Relevance {
                found.sort_by_key(|f| f.index);
            }
            return found;
        }

        q.text = words;
        let mut found: Vec<Found> = self
            .index
            .search(&q, &viewer, &opts)
            .hits
            .into_iter()
            .filter_map(|h| {
                Some(Found {
                    index: position(&h.item_id)?,
                    matched: h.matched.iter().map(|m| m.to_string()).collect(),
                })
            })
            .collect();
        let ranked: HashSet<usize> = found.iter().map(|f| f.index).collect();
        let mut inside: Vec<usize> = passing
            .iter()
            .copied()
            .filter(|i| !ranked.contains(i) && self.contains_all(*i, &q.text))
            .collect();
        if q.sort == Sort::Relevance {
            inside.sort_unstable();
            found.extend(inside.into_iter().map(|index| Found {
                index,
                matched: Vec::new(),
            }));
            return found;
        }
        // Sorted: the order of the items let through, for both kinds.
        let mut matched: HashMap<usize, Vec<String>> = found.into_iter().map(|f| (f.index, f.matched)).collect();
        let inside: HashSet<usize> = inside.into_iter().collect();
        passing
            .into_iter()
            .filter_map(|index| match matched.remove(&index) {
                Some(matched) => Some(Found { index, matched }),
                None => inside.contains(&index).then(|| Found {
                    index,
                    matched: Vec::new(),
                }),
            })
            .collect()
    }

    /// Every word is inside the item's name, a keyword, an emoji or a tag.
    fn contains_all(&self, i: usize, words: &[String]) -> bool {
        let texts = &self.texts[i];
        words.iter().all(|w| texts.iter().any(|t| t.contains(w.as_str())))
    }
}

/// `set:` against an item's set: its slug or name (normalised, as the
/// search compares them) or its id.
fn sets_accept(filter: &Filter<String>, set: &[String; 3]) -> bool {
    filter.accepts(|v| {
        let v = normalize(v.trim());
        set[0] == v || set[1] == v || set[2] == v
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tests::item;
    use stickpile_api::{MediaFormat, MediaKind};

    /// A pack "Cats" (slug `cats`) of alice's.
    fn pack() -> Vec<ItemSummary> {
        let it = |id: &str, name: &str, vibe: Option<&str>, tags: &[&str], format: MediaFormat| ItemSummary {
            name: name.into(),
            vibe: vibe.map(str::to_string),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            kind: if format == MediaFormat::Gif {
                MediaKind::Animation
            } else {
                MediaKind::Still
            },
            format,
            set_id: "s-cats".into(),
            set_slug: "cats".into(),
            set_name: "Cats".into(),
            owner_username: "alice".into(),
            keywords: vec![],
            emojis: vec![],
            quality: None,
            ..item(id)
        };
        vec![
            ItemSummary {
                keywords: vec!["grumpy".into()],
                emojis: vec!["😾".into()],
                quality: Some(40),
                copies: 1,
                ..it("a", "grumpy cat", Some("angry"), &["reaction"], MediaFormat::Png)
            },
            ItemSummary {
                quality: Some(90),
                copies: 7,
                likes: 3,
                ..it("b", "sleepy cat", Some("tired"), &["nsfw"], MediaFormat::Gif)
            },
            ItemSummary {
                quality: Some(70),
                copies: 3,
                ..it("c", "yawn", Some("tired"), &["reaction"], MediaFormat::Webp)
            },
            ItemSummary {
                emojis: vec!["😂".into()],
                ..it("d", "big chungus", None, &[], MediaFormat::Gif)
            },
        ]
    }

    fn ids(items: &[ItemSummary], found: &[Found]) -> Vec<String> {
        found.iter().map(|f| items[f.index].id.clone()).collect()
    }

    fn find(q: &str) -> Vec<String> {
        let items = pack();
        let w = Within::new(&items, Some(Visibility::Public), Some("alice"));
        ids(&items, &w.search(q, Some("alice"), &HashSet::from(["c".to_string()])))
    }

    /// [`find`] where the order of equally good matches does not matter.
    fn found(q: &str) -> Vec<String> {
        let mut v = find(q);
        v.sort();
        v
    }

    #[test]
    fn filters_narrow_the_pack_in_its_order() {
        assert_eq!(find(""), ["a", "b", "c", "d"], "nothing typed: all of it");
        assert_eq!(find("vibe:tired"), ["b", "c"]);
        assert_eq!(find("v:tired"), ["b", "c"], "the short key");
        assert_eq!(find("type:gif"), ["b", "d"]);
        assert_eq!(find("-tag:nsfw"), ["a", "c", "d"]);
        assert_eq!(find("-t:nsfw t:reaction"), ["a", "c"]);
        assert_eq!(find("type:animated -vibe:tired"), ["d"]);
        assert_eq!(find("q>60"), ["b", "c"]);
        assert_eq!(find("e:😂"), ["d"]);
        assert_eq!(find("is:liked"), ["c"]);
        assert_eq!(find("-is:liked"), ["a", "b", "d"]);
        assert_eq!(find("is:mine"), ["a", "b", "c", "d"]);
        assert_eq!(find("is:public b:alice"), ["a", "b", "c", "d"]);
        assert!(find("by:bob").is_empty());
        assert!(find("vibe:happy").is_empty());
    }

    #[test]
    fn set_looks_at_the_pack_but_its_name_is_no_word() {
        assert_eq!(find("set:cats"), ["a", "b", "c", "d"]);
        assert_eq!(find("s:Cats"), ["a", "b", "c", "d"]);
        assert_eq!(find("set:s-cats"), ["a", "b", "c", "d"], "the id");
        assert!(find("set:dogs").is_empty());
        assert!(find("-set:cats").is_empty());
        assert_eq!(found("cats"), ["a", "b"], "the cats by a typo, not the whole pack");
        assert_eq!(found("cat"), ["a", "b"]);
        assert_eq!(found("-cats"), ["a", "b", "c", "d"], "no item has the word");
    }

    #[test]
    fn sort_orders_as_the_search_does() {
        assert_eq!(find("sort:copies"), ["b", "c", "a", "d"]);
        assert_eq!(find("sort:quality"), ["b", "c", "a", "d"], "no score last");
        assert_eq!(find("t:reaction sort:quality"), ["c", "a"]);
        assert_eq!(find("cat sort:quality"), ["b", "a"]);
    }

    #[test]
    fn words_match_as_well_as_before() {
        assert_eq!(find("grumpy"), ["a"]);
        assert_eq!(find("GRUMPY"), ["a"]);
        assert_eq!(find("grumpi"), ["a"], "a typo");
        assert_eq!(found("cat"), ["a", "b"]);
        assert_eq!(find("g"), ["a", "d"], "one letter: anywhere, in list order");
        assert_eq!(find("at"), ["a", "b"], "inside a word");
        assert_eq!(find("😾"), ["a"], "an emoji");
        assert_eq!(found("reaction"), ["a", "c"], "a tag");
        assert_eq!(find("\"big chungus\""), ["d"], "a phrase");
        assert_eq!(find("tired"), ["b", "c"], "the vibe, which the old filter missed");
        assert_eq!(find("sleepy cat")[0], "b", "both words first");
        assert_eq!(find("cat -sleepy"), ["a"]);
        assert_eq!(find("cat type:gif"), ["b"]);
        assert_eq!(find("cat -t:nsfw"), ["a"]);
        assert!(find("xyzzy").is_empty());
    }

    #[test]
    fn ranked_words_report_the_fields() {
        let items = pack();
        let w = Within::new(&items, None, None);
        let found = w.search("grumpy", None, &HashSet::new());
        assert_eq!(found.len(), 1);
        assert!(found[0].matched.contains(&"name".to_string()), "{found:?}");
        assert!(w
            .search("c", None, &HashSet::new())
            .iter()
            .all(|f| f.matched.is_empty()));
    }

    #[test]
    fn without_the_viewer_or_the_pack() {
        let items = pack();
        let w = Within::new(&items, None, Some("alice"));
        let none = HashSet::new();
        assert!(w.search("is:mine", None, &none).is_empty(), "who is mine is not known");
        assert!(w.search("is:liked", Some("alice"), &none).is_empty());
        assert!(
            w.search("is:public", Some("alice"), &none).is_empty(),
            "alice's items count as private"
        );
        assert_eq!(
            w.search("grumpy", None, &none).len(),
            1,
            "private items are found all the same"
        );
        let bob = Within::new(&items, None, Some("bob"));
        assert_eq!(bob.search("is:public", Some("bob"), &none).len(), 4);
        assert!(Within::default().search("cat", None, &none).is_empty());
        assert!(Within::new(&[], None, None).is_empty());
    }
}
