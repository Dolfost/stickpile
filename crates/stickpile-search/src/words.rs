//! The words written on the items, offered while a `text:` (`x:`) value
//! is typed (docs/search.md, "Suggestions"): read from the index (the
//! `TEXT` postings and the ordered vocabulary), each counted once per item
//! the viewer can search, ranked as the suggestion line ranks values
//! ([`offer_tier`]).

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::ops::Bound;

use crate::rank::visible;
use crate::store::{Store, TokenEntry, TEXT};
use crate::text::{osa_start_within, osa_start_within_in, tokenize};
use crate::Viewer;

/// Typos forgiven in a typed value of `len` characters when values are
/// offered for it: none up to 2, 1 up to 5, then 2.
pub fn typo_budget(len: usize) -> usize {
    match len {
        0..=2 => 0,
        3..=5 => 1,
        _ => 2,
    }
}

/// Where `word` stands among the values offered for `typed` (both in
/// lower case), best first; `None`: it is not offered. 0: the word itself;
/// 1: it starts with `typed`; 2: it has `typed` inside (from 2 letters);
/// 3 and more: its start is that many minus 3 typos away from `typed`
/// (optimal string alignment, at most [`typo_budget`]), so that a typo in
/// a half-typed word counts. Nothing typed: every word, at 0.
///
/// The one ranking of the suggestion line (`stickpile_quick::suggest`)
/// and of [`crate::Index::text_words`].
pub fn offer_tier(typed: &[char], word: &[char]) -> Option<usize> {
    tier_by(typed, word, |b, from, max| osa_start_within(typed, b, from, max))
}

/// [`offer_tier`], the typos counted by `typos(start of word, from, max)`.
fn tier_by(typed: &[char], word: &[char], typos: impl FnOnce(&[char], usize, usize) -> Option<usize>) -> Option<usize> {
    if typed.is_empty() || word == typed {
        return Some(0);
    }
    if word.starts_with(typed) {
        return Some(1);
    }
    if typed.len() >= 2 && word.windows(typed.len()).any(|w| w == typed) {
        return Some(2);
    }
    // The start of the word of about the typed length (give or take the
    // budget), so that a typo in a half-typed word counts.
    let budget = typo_budget(typed.len());
    let from = typed.len().saturating_sub(budget).max(1);
    let to = (typed.len() + budget).min(word.len());
    if budget == 0 || to < from {
        return None;
    }
    typos(&word[..to], from, budget).map(|d| 3 + d)
}

/// [`offer_tier`] for one typed value against many words.
struct Offer {
    typed: Vec<char>,
    rows: [Vec<usize>; 3],
}

impl Offer {
    fn new(typed: Vec<char>) -> Offer {
        Offer {
            typed,
            rows: Default::default(),
        }
    }

    fn tier(&mut self, word: &[char]) -> Option<usize> {
        let (typed, rows) = (&self.typed, &mut self.rows);
        tier_by(typed, word, |b, from, max| {
            osa_start_within_in(typed, b, from, max, rows)
        })
    }
}

/// A token that may be offered: a word of 2 letters or more, not a
/// number alone.
fn offerable(e: &TokenEntry) -> bool {
    e.word && e.chars.len() >= 2 && !e.chars.iter().all(|c| c.is_numeric())
}

/// The one word `typed` is, as the index keeps words (`П'ятн` -> `пятн`,
/// `Café` -> `cafe`): `Some("")` while nothing is typed; `None` for more
/// than one word (`a-b`), none (`!!!`) or an emoji.
pub fn text_word(typed: &str) -> Option<String> {
    if typed.trim().is_empty() {
        return Some(String::new());
    }
    match tokenize(typed).as_slice() {
        [t] if t.word => Some(t.text.clone()),
        _ => None,
    }
}

/// Whether the viewer can search each doc, looked up once.
struct Seen<'a> {
    store: &'a Store,
    viewer: &'a Viewer,
    /// Per slot: 0 not looked up yet, 1 visible, 2 not.
    memo: Vec<u8>,
}

impl Seen<'_> {
    fn visible(&mut self, doc: u32) -> bool {
        let m = &mut self.memo[doc as usize];
        if *m == 0 {
            *m = if visible(self.viewer, &self.store.slot(doc).doc) {
                1
            } else {
                2
            };
        }
        *m == 1
    }
}

/// An offered word: (tier, items with it, token).
type Found = (usize, Reverse<u32>, u32);

impl Store {
    /// See [`crate::Index::text_words`].
    pub(crate) fn text_words(&self, typed: &str, viewer: &Viewer, limit: usize) -> Vec<String> {
        let Some(typed) = text_word(typed) else {
            return Vec::new();
        };
        if limit == 0 {
            return Vec::new();
        }
        let mut seen = Seen {
            store: self,
            viewer,
            memo: vec![0; self.slots.len()],
        };
        if typed.is_empty() {
            return self.most_common_text_words(&mut seen, limit);
        }
        let chars: Vec<char> = typed.chars().collect();
        let mut found: Vec<Found> = Vec::new();
        // The word itself and the words starting with it: a range of the
        // ordered vocabulary.
        let range = self
            .vocab
            .range::<str, _>((Bound::Included(typed.as_str()), Bound::Unbounded))
            .take_while(|(k, _)| k.starts_with(typed.as_str()));
        for (text, &id) in range {
            if let Some(items) = self.offered(id, &mut seen) {
                found.push((usize::from(*text != typed), Reverse(items), id));
            }
        }
        // The words with it inside, then those a typo or two away; only
        // when the line is not full of those starting with it.
        if found.len() < limit && (chars.len() >= 2 || typo_budget(chars.len()) > 0) {
            let mut offer = Offer::new(chars);
            for (id, e) in self.tokens.iter().enumerate() {
                if !offerable(e) || e.chars.starts_with(&offer.typed) {
                    continue;
                }
                let Some(tier) = offer.tier(&e.chars) else {
                    continue;
                };
                if let Some(items) = self.offered(id as u32, &mut seen) {
                    found.push((tier, Reverse(items), id as u32));
                }
            }
        }
        self.best_words(&mut found, limit);
        self.texts_of(found)
    }

    /// Keep the `limit` best of `found`, best first: by tier, then the most
    /// items, then alphabetically.
    fn best_words(&self, found: &mut Vec<Found>, limit: usize) {
        let text = |id: u32| self.tokens[id as usize].text.as_str();
        let cmp = |a: &Found, b: &Found| (a.0, a.1).cmp(&(b.0, b.1)).then_with(|| text(a.2).cmp(text(b.2)));
        if found.len() > limit {
            found.select_nth_unstable_by(limit, cmp);
            found.truncate(limit);
        }
        found.sort_unstable_by(cmp);
    }

    fn texts_of(&self, found: Vec<Found>) -> Vec<String> {
        found
            .into_iter()
            .map(|(_, _, id)| self.tokens[id as usize].text.clone())
            .collect()
    }

    /// The number of items `seen` lets through with the word `id` in their
    /// text, when the word is one to offer ([`offerable`]) and in such an
    /// item's text.
    fn offered(&self, id: u32, seen: &mut Seen) -> Option<u32> {
        let e = &self.tokens[id as usize];
        if !offerable(e) {
            return None;
        }
        let items = e
            .postings
            .iter()
            .filter(|p| p.field as usize == TEXT && seen.visible(p.doc))
            .count() as u32;
        (items > 0).then_some(items)
    }

    /// Nothing typed yet: the words in the most items. The words are
    /// counted from those in the most docs (in any field) down, and the
    /// counting stops where a word's docs could no longer reach the line.
    fn most_common_text_words(&self, seen: &mut Seen, limit: usize) -> Vec<String> {
        let mut by_docs: BinaryHeap<(u32, Reverse<u32>)> = self
            .tokens
            .iter()
            .enumerate()
            .filter(|(_, e)| offerable(e))
            .map(|(id, e)| (e.df, Reverse(id as u32)))
            .collect();
        let mut found: Vec<Found> = Vec::with_capacity(limit + 1);
        while let Some((docs, Reverse(id))) = by_docs.pop() {
            if found.len() == limit && docs < found[limit - 1].1 .0 {
                break;
            }
            if let Some(items) = self.offered(id, seen) {
                found.push((0, Reverse(items), id));
                self.best_words(&mut found, limit);
            }
        }
        self.texts_of(found)
    }
}
