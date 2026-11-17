//! The offers under a search field while a query is typed
//! ([`crate::StickpileLibrary::hints`]): on a phone there is no Tab key to
//! complete with, so each offer carries the whole query it makes.
//!
//! - While a `key:value` filter is typed (`vibe:sar`, `-tag:`), its values,
//!   as [`suggest::suggest`] ranks them for the desktop's suggestion line;
//!   for `e:` (or `emoji:`) the emojis, labelled with their names
//!   (`💀 skull`): the emojis of the cached items, most used first, and
//!   while a name is typed (`e:sk`) the emojis it selects.
//! - While a bare word is typed (`rea`), the filters whose value contains
//!   it (`tag:reaction`, `vibe:really`, `set:reactions`), and the filter
//!   keys it starts (`t` offers `tag:` and `type:`).
//! - After a space, or with nothing typed, the filter keys.
//!
//! A key offered shows its one-letter form too (`tag: (t:)`); the short
//! keys work as the long ones (`v:sar` offers what `vibe:sar` does).

use stickpile_api::query::Key;
use stickpile_api::ItemSummary;
use stickpile_quick::suggest::{self, Vocabulary};

use crate::types::{HintKind, SearchHint};

/// Offers at most.
pub const MAX_HINTS: usize = 12;

/// Value offers for a bare word at most (the keys it starts come after).
const MAX_WORD_HINTS: usize = 8;

/// A bare word shorter than this offers only the keys it starts.
const MIN_WORD: usize = 2;

/// The filter keys offered, in this order.
pub const KEYS: [&str; 8] = ["tag:", "vibe:", "e:", "set:", "by:", "type:", "is:", "sort:"];

/// The words the hints offer: the suggestion vocabulary, the emojis of
/// the items included ([`Vocabulary::emojis`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Words {
    pub vocab: Vocabulary,
}

impl Words {
    pub fn from_items<'a>(items: impl IntoIterator<Item = &'a ItemSummary>) -> Words {
        Words {
            vocab: Vocabulary::from_items(items),
        }
    }
}

/// The offers for `query` as typed so far.
pub fn hints(query: &str, words: &Words) -> Vec<SearchHint> {
    if query.matches('"').count() % 2 == 1 {
        // Inside a phrase: nothing to offer.
        return Vec::new();
    }
    if query.is_empty() || query.ends_with(char::is_whitespace) {
        return keys(query, "");
    }
    let start = query
        .rfind(char::is_whitespace)
        .map_or(0, |i| i + query[i..].chars().next().map_or(1, char::len_utf8));
    let (before, token) = query.split_at(start);
    if token.contains('"') {
        return Vec::new();
    }
    let (neg, bare) = match token.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", token),
    };
    if bare.contains(':') {
        return match suggest::suggest(query, &words.vocab) {
            Some(s) => {
                let chosen = s.chosen();
                s.words
                    .iter()
                    .zip(&s.labels)
                    .enumerate()
                    .map(|(i, (w, name))| SearchHint {
                        // An emoji with its name: `💀 skull`.
                        label: match name.is_empty() {
                            true => w.clone(),
                            false => format!("{w} {name}"),
                        },
                        query: format!("{}{w} ", &query[..s.start]),
                        chosen: chosen == Some(i),
                        kind: HintKind::Value,
                    })
                    .collect()
            }
            None => Vec::new(),
        };
    }
    let mut out = if bare.chars().count() >= MIN_WORD {
        values(before, neg, bare, &words.vocab)
    } else {
        Vec::new()
    };
    out.extend(keys(before, bare).into_iter().map(|mut h| {
        if !neg.is_empty() {
            h.label = format!("-{}", h.label);
            h.query = format!("{before}-{}", &h.query[before.len()..]);
        }
        h
    }));
    out.truncate(MAX_HINTS);
    out
}

/// The keys starting with `typed` (all of them for none), as offers that
/// put the key in place of `typed` after `before`.
fn keys(before: &str, typed: &str) -> Vec<SearchHint> {
    let typed = typed.to_lowercase();
    KEYS.iter()
        .filter(|k| k.starts_with(&typed))
        .map(|k| SearchHint {
            label: key_label(k),
            query: format!("{before}{k}"),
            chosen: false,
            kind: HintKind::Key,
        })
        .collect()
}

/// A key as offered: with its one-letter form when it has another
/// (`tag: (t:)`, `e:`, `type:`).
fn key_label(key: &str) -> String {
    let name = key.trim_end_matches(':');
    match Key::parse(name).and_then(Key::short) {
        Some(short) if short != name => format!("{key} ({short}:)"),
        _ => key.to_string(),
    }
}

/// The filters whose value contains the bare word `typed`: the tags, vibes
/// and packs, those starting with it first, each list in its order of use.
fn values(before: &str, neg: &str, typed: &str, vocab: &Vocabulary) -> Vec<SearchHint> {
    let lower = typed.to_lowercase();
    let fields: [(&str, &[String]); 3] = [("tag:", &vocab.tags), ("vibe:", &vocab.vibes), ("set:", &vocab.sets)];
    let mut starts = Vec::new();
    let mut contains = Vec::new();
    for (key, list) in fields {
        for w in list {
            if w.starts_with(&lower) {
                starts.push((key, w));
            } else if w.contains(&lower) {
                contains.push((key, w));
            }
        }
    }
    starts
        .into_iter()
        .chain(contains)
        .take(MAX_WORD_HINTS)
        .map(|(key, w)| SearchHint {
            label: format!("{neg}{key}{w}"),
            query: format!("{before}{neg}{key}{w} "),
            chosen: false,
            kind: HintKind::Filter,
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn words() -> Words {
        Words {
            vocab: Vocabulary {
                vibes: vec!["sarcastic".into(), "happy".into(), "really".into()],
                tags: vec!["reaction".into(), "nsfw".into(), "cat".into()],
                sets: vec!["cats".into(), "reactions".into()],
                owners: vec!["alice".into()],
                emojis: vec!["😂".into(), "🐱".into(), "❤️".into(), "💀".into()],
                me: None,
            },
        }
    }

    fn labels(h: &[SearchHint]) -> Vec<&str> {
        h.iter().map(|h| h.label.as_str()).collect()
    }

    fn query_of<'a>(h: &'a [SearchHint], label: &str) -> &'a str {
        &h.iter().find(|h| h.label == label).unwrap().query
    }

    #[test]
    fn nothing_typed_offers_the_keys() {
        let h = hints("", &words());
        assert_eq!(
            labels(&h),
            vec![
                "tag: (t:)",
                "vibe: (v:)",
                "e:",
                "set: (s:)",
                "by: (b:)",
                "type:",
                "is:",
                "sort:"
            ],
            "with their one-letter forms"
        );
        assert!(h.iter().all(|h| h.kind == HintKind::Key));
        assert_eq!(query_of(&h, "tag: (t:)"), "tag:");
        let h = hints("grumpy ", &words());
        assert_eq!(query_of(&h, "vibe: (v:)"), "grumpy vibe:");
        assert_eq!(query_of(&h, "e:"), "grumpy e:");
    }

    #[test]
    fn short_keys_offer_what_the_long_ones_do() {
        let h = hints("cat v:sar", &words());
        assert_eq!(labels(&h), vec!["sarcastic"]);
        assert_eq!(h[0].query, "cat v:sarcastic ", "the key stays as typed");
        assert!(h[0].chosen);
        let h = hints("-t:", &words());
        assert_eq!(labels(&h), vec!["reaction", "nsfw", "cat"]);
        assert_eq!(query_of(&h, "nsfw"), "-t:nsfw ");
        assert_eq!(labels(&hints("s:ca", &words())), vec!["cats"]);
        assert_eq!(hints("b:al", &words())[0].query, "b:alice ");
        assert_eq!(labels(&hints("T:re", &words())), labels(&hints("tag:re", &words())));
    }

    #[test]
    fn a_filter_being_typed_offers_its_values() {
        let h = hints("cat vibe:sar", &words());
        assert_eq!(labels(&h), vec!["sarcastic"]);
        assert_eq!(h[0].query, "cat vibe:sarcastic ");
        assert!(h[0].chosen);
        assert_eq!(h[0].kind, HintKind::Value);

        let h = hints("-tag:", &words());
        assert_eq!(labels(&h), vec!["reaction", "nsfw", "cat"]);
        assert_eq!(query_of(&h, "nsfw"), "-tag:nsfw ");
        assert!(h.iter().all(|h| !h.chosen), "nothing typed, nothing chosen");

        let h = hints("is:", &words());
        assert_eq!(labels(&h), vec!["mine", "liked", "public"]);
        assert!(hints("vibe:zzzzzz", &words()).is_empty());
        assert!(hints("color:red", &words()).is_empty(), "not a key with values");
    }

    #[test]
    fn emoji_filters_offer_the_emojis_with_their_names() {
        let h = hints("cat e:", &words());
        assert_eq!(
            labels(&h),
            vec!["😂 face with tears of joy", "🐱 cat face", "❤️ red heart", "💀 skull"]
        );
        assert_eq!(query_of(&h, "🐱 cat face"), "cat e:🐱 ", "the emoji goes in");
        assert!(h.iter().all(|h| h.kind == HintKind::Value && !h.chosen));
        let h = hints("-emoji:", &words());
        assert_eq!(query_of(&h, "😂 face with tears of joy"), "-emoji:😂 ");
        let h = hints("e:❤", &words());
        assert_eq!(labels(&h), vec!["❤️ red heart"], "the selector completes the heart");
        assert!(h[0].chosen, "the same emoji");
        let h = hints("e:😂", &words());
        assert_eq!(labels(&h), vec!["😂 face with tears of joy"]);
        assert!(h[0].chosen);
    }

    #[test]
    fn an_emoji_name_offers_the_emojis_it_selects() {
        let h = hints("cat e:sk", &words());
        assert_eq!(h[0].label, "💀 skull", "the own one first");
        assert_eq!(h[0].query, "cat e:💀 ");
        assert_eq!(h[1].label, "☠️ skull and crossbones");
        assert_eq!(h.len(), MAX_HINTS);
        assert!(h.iter().all(|h| !h.chosen), "a name is searched as typed");
        let s = suggest::suggest("cat e:sk", &words().vocab).unwrap();
        let glyphs: Vec<String> = h
            .iter()
            .map(|h| h.label.split(' ').next().unwrap().to_string())
            .collect();
        assert_eq!(glyphs, s.words, "the desktop's offers");
        let h = hints("e:SKLUL", &words());
        assert_eq!(
            labels(&h),
            vec!["💀 skull", "☠️ skull and crossbones", "🩻 x-ray"],
            "typos as the search forgives them"
        );
        assert!(hints("e:xqzv", &words()).is_empty());
    }

    #[test]
    fn a_bare_word_offers_the_filters_containing_it() {
        let h = hints("funny rea", &words());
        assert_eq!(
            labels(&h),
            vec!["tag:reaction", "vibe:really", "set:reactions"],
            "starting with it, then containing it"
        );
        assert_eq!(query_of(&h, "tag:reaction"), "funny tag:reaction ");
        assert!(h.iter().all(|h| h.kind == HintKind::Filter));

        let h = hints("at", &words());
        assert_eq!(labels(&h), vec!["tag:cat", "set:cats"]);

        let h = hints("-ns", &words());
        assert_eq!(labels(&h), vec!["-tag:nsfw", "-set:reactions"]);
        assert_eq!(h[0].query, "-tag:nsfw ");

        let h = hints("REA", &words());
        assert_eq!(h[0].label, "tag:reaction", "case does not matter");
    }

    #[test]
    fn a_word_offers_the_keys_it_starts() {
        let h = hints("t", &words());
        assert_eq!(labels(&h), vec!["tag: (t:)", "type:"]);
        assert_eq!(query_of(&h, "type:"), "type:");
        let h = hints("cat -ta", &words());
        assert_eq!(labels(&h), vec!["-tag: (t:)"]);
        assert_eq!(h[0].query, "cat -tag:");
        let h = hints("vibe", &words());
        assert_eq!(labels(&h), vec!["vibe: (v:)"], "the key itself, without its colon");
        let h = hints("sa", &words());
        assert_eq!(labels(&h), vec!["vibe:sarcastic"], "sa- starts no key");
    }

    #[test]
    fn phrases_offer_nothing() {
        assert!(hints("\"big chu", &words()).is_empty());
        assert!(hints("\"big chungus\"", &words()).is_empty());
        assert!(!hints("\"big chungus\" ", &words()).is_empty());
    }

    #[test]
    fn the_emojis_are_counted_from_the_items() {
        let item = |emojis: &[&str]| ItemSummary {
            emojis: emojis.iter().map(|e| e.to_string()).collect(),
            ..summary()
        };
        let items = [item(&["🐱", "😂"]), item(&["😂", " "]), item(&["😂", "🐶"])];
        let w = Words::from_items(items.iter());
        assert_eq!(w.vocab.emojis, vec!["😂", "🐱", "🐶"]);
    }

    pub(crate) fn summary() -> ItemSummary {
        ItemSummary {
            id: "i".into(),
            set_id: "s".into(),
            set_slug: "cats".into(),
            set_name: "Cats".into(),
            owner_username: "alice".into(),
            name: "cat".into(),
            keywords: vec![],
            emojis: vec![],
            vibe: None,
            tags: vec![],
            text: String::new(),
            kind: stickpile_api::MediaKind::Still,
            format: stickpile_api::MediaFormat::Png,
            width: 1,
            height: 1,
            quality: None,
            likes: 0,
            copies: 0,
            media_id: String::new(),
            sha256: String::new(),
            phash: String::new(),
            variants: vec![],
            updated_at: 0,
        }
    }
}

#[cfg(test)]
mod review_tests;
