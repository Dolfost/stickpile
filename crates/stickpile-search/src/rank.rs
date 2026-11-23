//! Query evaluation: filters, lexical scoring, vector scoring, fusion, ordering.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::ops::Bound;

use stickpile_api::query::{Filter, Is, Query, Sort};
use stickpile_api::Visibility;

use crate::store::{IntMap, Punct, Slot, Store, EMOJI_NAME, FIELD_B, FIELD_LABEL, FIELD_WEIGHT, NFIELDS, TEXT};
use crate::text::{
    dot, emoji_key, is_skin_tone, normalize, osa_within, padded_trigrams, squash_key, tokenize, tri_is_padded,
};
use crate::{emoji_names, Doc, Hit, Outcome, SearchOptions, Viewer};

/// Page size used when [`SearchOptions::limit`] is 0.
pub const DEFAULT_LIMIT: usize = 50;
/// Reciprocal-rank-fusion constant.
const RRF_K: f32 = 60.0;
/// BM25 term-frequency saturation.
const K1: f32 = 1.2;
/// Soft terms beyond this are ignored (match bitmask is a u32).
const MAX_USER_TERMS: usize = 16;
const MAX_EXPANSIONS: usize = 32;

const FUZZY: u16 = 1 << 6;
const EXPANSION: u16 = 1 << 7;
const IMAGE: u16 = 1 << 8;
/// Bit `i` is label `i`; the fields' bits are in `FIELD_LABEL` (the text's
/// is bit 9, after the older labels, so their order stays).
const LABELS: [&str; 10] = [
    "name",
    "keyword",
    "emoji",
    "vibe",
    "tag",
    "set",
    "fuzzy",
    "expansion",
    "image",
    "text",
];

/// How a query token matched a vocabulary token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Exact,
    Squash,
    Prefix,
    Fuzzy,
    Substring,
}

impl Kind {
    pub(crate) fn quality(self) -> f32 {
        match self {
            Kind::Exact => 1.0,
            Kind::Squash => 0.8,
            Kind::Prefix => 0.7,
            Kind::Fuzzy => 0.5,
            Kind::Substring => 0.3,
        }
    }
    fn is_fuzzy(self) -> bool {
        matches!(self, Kind::Squash | Kind::Fuzzy)
    }
    /// Where a `text:` match ranks, best first ([`crate::Hit::text_tier`]):
    /// the word, a word it starts (being typed), squashed, a typo, inside
    /// a word.
    pub(crate) fn tier(self) -> u8 {
        match self {
            Kind::Exact => 0,
            Kind::Prefix => 1,
            Kind::Squash => 2,
            Kind::Fuzzy => 3,
            Kind::Substring => 4,
        }
    }
}

/// The match kinds [`Store::candidates`] looks for.
#[derive(Debug, Clone, Copy)]
struct Rule {
    /// Prefixes from this many characters; `None`: no prefixes.
    prefix_from: Option<usize>,
    /// Squashed, typo and substring matches too.
    fuzzy: bool,
    /// Per token, keep the kind of the best [`Kind::tier`] rather than of
    /// the best [`Kind::quality`].
    tiered: bool,
}

/// A plain word: prefixes from 2 characters, every kind, the best quality.
const WORD: Rule = Rule {
    prefix_from: Some(2),
    fuzzy: true,
    tiered: false,
};

impl Rule {
    /// A word of a `text:` value: a prefix from the first letter for the
    /// last word only (the one being typed); for `-text:` the word or a
    /// prefix only, so a typo never hides more than was meant.
    fn text(last: bool, negated: bool) -> Rule {
        Rule {
            prefix_from: last.then_some(1),
            fuzzy: !negated,
            tiered: true,
        }
    }
}

struct QTok {
    text: String,
    chars: Vec<char>,
    word: bool,
    /// Punctuation: it matches the punctuation tokens it is inside
    /// ([`Store::punct_candidates`]).
    punct: bool,
}

struct Term {
    toks: Vec<QTok>,
    weight: f32,
    /// Bit in the matched-terms mask for user terms; `None` for expansions.
    user_bit: Option<u32>,
}

fn prepare(text: &str) -> Vec<QTok> {
    tokenize(text)
        .into_iter()
        .map(|t| QTok {
            chars: t.text.chars().collect(),
            text: t.text,
            word: t.word,
            punct: t.punct,
        })
        .collect()
}

/// Whether the punctuation tokens `marks` (`(at, token)` of one value, in
/// order) have, at word position `at`, tokens matching the punctuation
/// `gap` of a `text:` value, one each, in its order.
fn gap_has(marks: &[(usize, u32)], at: usize, gap: &[IntMap<u32, Kind>]) -> bool {
    if gap.is_empty() {
        return true;
    }
    let mut want = gap.iter().peekable();
    for (_, id) in marks.iter().filter(|(a, _)| *a == at) {
        if want.peek().is_some_and(|c| c.contains_key(id)) {
            want.next();
        }
    }
    want.peek().is_none()
}

#[derive(Default, Clone, Copy)]
struct TermHit {
    field: [f32; NFIELDS],
    best_q: f32,
    fuzzy: bool,
}

impl TermHit {
    fn add(&mut self, field: usize, score: f32, kind_q: f32, fuzzy: bool) {
        if score > self.field[field] {
            self.field[field] = score;
        }
        if kind_q > self.best_q {
            self.best_q = kind_q;
            self.fuzzy = fuzzy;
        }
    }
}

#[derive(Default)]
struct Acc {
    score: f32,
    mask: u32,
    labels: u16,
}

struct Row {
    idx: u32,
    score: f32,
    labels: u16,
    /// [`crate::Hit::text_tier`].
    tier: u8,
}

/// One `e:` value.
#[derive(PartialEq)]
enum EmojiValue {
    /// An emoji: its key without variation selectors, and whether it has a
    /// skin tone (then only that tone matches, else any).
    Glyph(String, bool),
    /// A name: the keys ([`emoji_names::key`]) of the emojis it selects;
    /// none for a name that selects nothing.
    Name(HashSet<&'static str>),
}

impl EmojiValue {
    fn new(value: &str) -> EmojiValue {
        if emoji_names::is_name(value) {
            return EmojiValue::Name(emoji_names::by_name(value).into_iter().map(|e| e.key()).collect());
        }
        let k = emoji_key(value);
        let toned = k.chars().any(is_skin_tone);
        EmojiValue::Glyph(k, toned)
    }

    fn in_doc(&self, s: &Slot) -> bool {
        match self {
            EmojiValue::Glyph(k, true) => s.emoji_keys.iter().any(|e| e == k),
            EmojiValue::Glyph(k, false) => s.emoji_bases.iter().any(|e| e == k),
            EmojiValue::Name(keys) => s.emoji_bases.iter().any(|e| keys.contains(e.as_str())),
        }
    }
}

/// Filters and visibility, prepared once per query.
struct Filters<'a> {
    q: &'a Query,
    viewer: &'a Viewer,
    emojis: Filter<EmojiValue>,
    sets: Filter<String>,
    vibes: Filter<String>,
    tags: Filter<String>,
    owners: Filter<String>,
    texts: TextFilter,
    /// Token-id sequences that must not occur in any own text field.
    exclude: Vec<Vec<u32>>,
    /// The docs a `-` term of punctuation only (`-?`) leaves out: those
    /// with it in the name or the text.
    exclude_punct: IntMap<u32, ()>,
}

fn map_filter<T, U>(f: &Filter<T>, g: impl Fn(&T) -> U) -> Filter<U> {
    Filter {
        any: f.any.iter().map(&g).collect(),
        none: f.none.iter().map(&g).collect(),
    }
}

/// The `text:` values, matched in the index before the docs are looked
/// at (docs/search.md, "The item text").
struct TextFilter {
    /// The docs with one of the `text:` values, each with the best
    /// [`Kind::tier`] it matched; `None` without such values.
    any: Option<IntMap<u32, u8>>,
    /// The docs with one of the `-text:` values, the words themselves or
    /// a prefix of the last.
    none: IntMap<u32, ()>,
}

impl TextFilter {
    fn new(store: &Store, f: &Filter<String>) -> TextFilter {
        let any = (!f.any.is_empty()).then(|| {
            let mut docs: IntMap<u32, u8> = IntMap::default();
            for v in &f.any {
                for (doc, tier) in store.text_matches(v, false) {
                    docs.entry(doc).and_modify(|t| *t = (*t).min(tier)).or_insert(tier);
                }
            }
            docs
        });
        let mut none: IntMap<u32, ()> = IntMap::default();
        for v in &f.none {
            none.extend(store.text_matches(v, true).into_keys().map(|d| (d, ())));
        }
        TextFilter { any, none }
    }

    fn accepts(&self, idx: u32) -> bool {
        self.any.as_ref().is_none_or(|docs| docs.contains_key(&idx)) && !self.none.contains_key(&idx)
    }

    /// [`crate::Hit::text_tier`] of a doc that passed.
    fn tier(&self, idx: u32) -> u8 {
        self.any.as_ref().and_then(|docs| docs.get(&idx)).copied().unwrap_or(0)
    }
}

impl<'a> Filters<'a> {
    fn new(store: &Store, q: &'a Query, viewer: &'a Viewer) -> Filters<'a> {
        let norm = |s: &String| normalize(s.trim());
        let mut exclude = Vec::new();
        let mut exclude_punct: IntMap<u32, ()> = IntMap::default();
        for t in &q.exclude_text {
            let (words, punct): (Vec<_>, Vec<_>) = tokenize(t).into_iter().partition(|t| !t.punct);
            if words.is_empty() {
                for p in &punct {
                    for id in store.puncts_with(&p.text) {
                        exclude_punct.extend(store.tokens[id as usize].postings.iter().map(|p| (p.doc, ())));
                    }
                }
                continue;
            }
            // The words of a term with them, as without its punctuation. A
            // token missing from the vocabulary cannot occur in any doc.
            if let Some(run) = words.iter().map(|t| store.vocab.get(&t.text).copied()).collect() {
                exclude.push(run);
            }
        }
        Filters {
            q,
            viewer,
            emojis: map_filter(&q.emojis, |e| EmojiValue::new(e)),
            sets: map_filter(&q.sets, norm),
            vibes: map_filter(&q.vibes, norm),
            tags: map_filter(&q.tags, norm),
            owners: map_filter(&q.owners, |s| s.trim().to_lowercase()),
            texts: TextFilter::new(store, &q.texts),
            exclude,
            exclude_punct,
        }
    }

    /// Whether the doc in slot `idx` passes.
    fn pass(&self, idx: u32, s: &Slot) -> bool {
        let d = &s.doc;
        let q = self.q;
        self.viewer.can_search(d)
            && self.texts.accepts(idx)
            && q.quality_ok(d.quality)
            && q.likes_ok(d.likes)
            && q.copies_ok(d.copies)
            && q.types.accepts(|t| t.matches(d.kind, d.format))
            && q.is.accepts(|i| match i {
                Is::Mine => !self.viewer.user_id.is_empty() && d.owner_id == self.viewer.user_id,
                Is::Liked => self.viewer.liked.contains(&d.item_id),
                Is::Public => d.visibility == Visibility::Public,
            })
            && self.emojis.accepts(|e| e.in_doc(s))
            && self
                .sets
                .accepts(|v| s.set_slug == *v || s.set_name == *v || d.set_id == *v)
            && self.vibes.accepts(|v| s.vibe.as_deref() == Some(v.as_str()))
            && self.tags.accepts(|v| s.tags.contains(v))
            && self.owners.accepts(|v| s.owner == *v)
            && !self.excluded(s)
            && (self.exclude_punct.is_empty() || !self.exclude_punct.contains_key(&idx))
    }

    /// A `-word` leaves out docs with it in any own field: every field but
    /// the words derived from the emojis.
    fn excluded(&self, s: &Slot) -> bool {
        self.exclude.iter().any(|seq| {
            s.fields
                .iter()
                .enumerate()
                .filter(|(f, _)| *f != EMOJI_NAME)
                .any(|(_, values)| contains_run(values, seq))
        })
    }
}

/// Whether one of `values` has the non-empty token run `seq`, consecutive.
fn contains_run(values: &[Vec<u32>], seq: &[u32]) -> bool {
    !seq.is_empty() && values.iter().any(|v| v.windows(seq.len()).any(|w| w == seq))
}

/// Tie-breaks shared by every ordering: copies desc, updated_at desc, item_id.
fn tiebreak(a: &Doc, b: &Doc) -> Ordering {
    b.copies
        .cmp(&a.copies)
        .then_with(|| b.updated_at.cmp(&a.updated_at))
        .then_with(|| a.item_id.cmp(&b.item_id))
}

impl Store {
    fn idf(&self, df: u32) -> f32 {
        let n = self.len() as f32;
        let df = (df as f32).min(n);
        (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
    }

    fn sat(&self, tf: u8, len: u16, field: usize) -> f32 {
        let avg = match self.field_nonempty[field] {
            0 => 1.0,
            n => self.field_len_sum[field] as f32 / n as f32,
        };
        let b = FIELD_B[field];
        // A word said again in the picture ("NO NO NO") says it once: a
        // caption repeating a word must not outrank the keyword or the
        // emoji given for it.
        let tf = if field == TEXT { 1.0 } else { tf as f32 };
        tf * (K1 + 1.0) / (tf + K1 * (1.0 - b + b * len as f32 / avg.max(1e-3)))
    }

    /// The punctuation tokens with `value` inside: `?` is in `?`, `??` and
    /// `?!`; `...` in `....`, not in `..`.
    fn puncts_with<'a>(&'a self, value: &'a str) -> impl Iterator<Item = u32> + 'a {
        self.puncts
            .iter()
            .copied()
            .filter(move |id| self.tokens[*id as usize].text.contains(value))
    }

    /// The candidates of a punctuation token ([`Self::puncts_with`]): one
    /// kind of match, as good as the token itself; no prefix, typo or
    /// substring tier.
    fn punct_candidates(&self, q: &QTok) -> IntMap<u32, Kind> {
        self.puncts_with(&q.text).map(|id| (id, Kind::Exact)).collect()
    }

    /// Vocabulary tokens matching one query token by the kinds of `rule`,
    /// each with its best kind; punctuation by [`Self::punct_candidates`].
    fn candidates(&self, q: &QTok, rule: Rule) -> IntMap<u32, Kind> {
        if q.punct {
            return self.punct_candidates(q);
        }
        let mut best: IntMap<u32, Kind> = IntMap::default();
        let better = |new: Kind, old: Kind| match rule.tiered {
            true => new.tier() < old.tier(),
            false => new.quality() > old.quality(),
        };
        let mut put = |id: u32, k: Kind| {
            best.entry(id)
                .and_modify(|e| *e = if better(k, *e) { k } else { *e })
                .or_insert(k);
        };
        if let Some(&id) = self.vocab.get(&q.text) {
            put(id, Kind::Exact);
        }
        if !q.word {
            return best;
        }
        let n = q.chars.len();
        let prefix = rule.prefix_from.is_some_and(|from| n >= from);
        if prefix {
            let range = self
                .vocab
                .range::<str, _>((Bound::Excluded(q.text.as_str()), Bound::Unbounded));
            for (_, &id) in range.take_while(|(k, _)| k.starts_with(&q.text)) {
                put(id, Kind::Prefix);
            }
        }
        if !rule.fuzzy {
            return best;
        }
        if let Some(ids) = self.squashed.get(&squash_key(&q.text)) {
            for &id in ids {
                put(id, Kind::Squash);
            }
        }
        let max_edits = match n {
            _ if q.chars.iter().any(|c| c.is_numeric()) => 0,
            7.. => 2,
            4.. => 1,
            _ => 0,
        };
        let substring = n >= 3;
        if max_edits == 0 && !substring {
            return best;
        }
        let tris = padded_trigrams(&q.chars);
        let interior = tris.iter().filter(|t| !tri_is_padded(**t)).count() as u16;
        // Shared (all, interior) trigrams per vocabulary token; dense because
        // the padding trigrams ("$$c", "s$$") have long lists.
        let mut counts: Vec<(u16, u16)> = vec![(0, 0); self.tokens.len()];
        let mut touched: Vec<u32> = Vec::new();
        for &t in &tris {
            let padded = tri_is_padded(t);
            if padded && max_edits == 0 {
                continue;
            }
            let Some(list) = self.trigrams.get(&t) else { continue };
            for &id in list {
                let c = &mut counts[id as usize];
                if c.0 == 0 {
                    touched.push(id);
                }
                c.0 += 1;
                if !padded {
                    c.1 += 1;
                }
            }
        }
        // Count filter: one edit destroys at most 4 distinct padded trigrams
        // (a transposition) of either word, so words within `k` edits share at
        // least max(|Tq|, |Tc|) - 4k of them.
        let slack = 4 * max_edits as i64;
        for id in touched {
            let (all, inner) = counts[id as usize];
            let e = &self.tokens[id as usize];
            let need = (tris.len().max(e.trigrams as usize) as i64 - slack).max(1);
            if max_edits > 0 && all as i64 >= need && osa_within(&q.chars, &e.chars, max_edits).is_some() {
                put(id, Kind::Fuzzy);
            }
            // Inside the word: where no prefix counts (a word before the
            // last of a `text:` phrase), the start of a word is no match
            // either, or `x:"thou head"` would find "thoughts head".
            if substring
                && interior > 0
                && inner == interior
                && e.text.contains(&q.text)
                && (prefix || !e.text.starts_with(&q.text))
            {
                put(id, Kind::Substring);
            }
        }
        best
    }

    /// The docs whose text has the `text:` value `value`, each with the
    /// [`Kind::tier`] it matched: each word by its kinds ([`Rule::text`]),
    /// several words one after another in the text, a phrase as well as
    /// its weakest word, in its best place. `negated`: for `-text:`.
    fn text_matches(&self, value: &str, negated: bool) -> IntMap<u32, u8> {
        let mut out: IntMap<u32, u8> = IntMap::default();
        let toks = prepare(value);
        // The words (and emojis) in a row, and the punctuation around them:
        // `gaps[i]` before word `i`, `gaps[n]` after the last. The last word
        // is the one being typed when nothing follows it.
        let n = toks.iter().filter(|t| !t.punct).count();
        let ends_in_word = toks.last().is_some_and(|t| !t.punct);
        let mut cands: Vec<IntMap<u32, Kind>> = Vec::with_capacity(n);
        let mut gaps: Vec<Vec<IntMap<u32, Kind>>> = (0..=n).map(|_| Vec::new()).collect();
        for t in &toks {
            if t.punct {
                gaps[cands.len()].push(self.punct_candidates(t));
            } else {
                let last = ends_in_word && cands.len() + 1 == n;
                cands.push(self.candidates(t, Rule::text(last, negated)));
            }
        }
        if toks.is_empty() || cands.iter().chain(gaps.iter().flatten()).any(IntMap::is_empty) {
            return out;
        }
        let in_text = |c: &IntMap<u32, Kind>| -> Vec<(u32, u8)> {
            c.iter()
                .flat_map(|(id, k)| {
                    self.tokens[*id as usize]
                        .postings
                        .iter()
                        .filter(|p| p.field as usize == TEXT)
                        .map(|p| (p.doc, k.tier()))
                })
                .collect()
        };
        let punct = gaps.iter().any(|g| !g.is_empty());
        // One word or one punctuation alone (`x:lo`, `x:?`): the texts with it.
        let single = match (n, punct) {
            (1, false) => Some(&cands[0]),
            (0, _) if gaps[0].len() == 1 => Some(&gaps[0][0]),
            _ => None,
        };
        if let Some(c) = single {
            for (doc, tier) in in_text(c) {
                out.entry(doc).and_modify(|t| *t = (*t).min(tier)).or_insert(tier);
            }
            return out;
        }
        // A phrase: the texts with its rarest token, read word by word (and
        // gap by gap).
        let postings_of = |c: &IntMap<u32, Kind>| {
            c.keys()
                .map(|id| self.tokens[*id as usize].postings.len())
                .sum::<usize>()
        };
        let pivot = cands
            .iter()
            .chain(gaps.iter().flatten())
            .min_by_key(|c| postings_of(c))
            .expect("a token");
        let mut docs: Vec<u32> = in_text(pivot).into_iter().map(|(d, _)| d).collect();
        docs.sort_unstable();
        docs.dedup();
        let words_at = |v: &[u32]| {
            v.iter()
                .zip(&cands)
                .try_fold(0u8, |worst, (id, c)| c.get(id).map(|k| worst.max(k.tier())))
        };
        for doc in docs {
            let slot = self.slot(doc);
            let best = if punct {
                // The text is one value; its punctuation by word position.
                let marks: Vec<(usize, u32)> = self
                    .marks_of(doc)
                    .iter()
                    .filter(|p| p.field as usize == TEXT)
                    .map(|p| (p.at as usize, p.id))
                    .collect();
                slot.fields[TEXT].first().and_then(|v| {
                    (0..(v.len() + 1).saturating_sub(n))
                        .filter_map(|k| {
                            let tier = words_at(&v[k..k + n])?;
                            let gaps_ok = gaps.iter().enumerate().all(|(j, g)| gap_has(&marks, k + j, g));
                            gaps_ok.then_some(tier)
                        })
                        .min()
                })
            } else {
                slot.fields[TEXT]
                    .iter()
                    .flat_map(|v| v.windows(n))
                    .filter_map(words_at)
                    .min()
            };
            if let Some(tier) = best {
                out.insert(doc, tier);
            }
        }
        out
    }

    /// The idf of a punctuation token, one for all the tokens it is inside
    /// (`cands`), from their docs together: a `?` in `??` counts as one in
    /// `?`, as the match is one kind.
    fn punct_idf(&self, cands: &IntMap<u32, Kind>) -> f32 {
        let df = cands
            .keys()
            .fold(0u32, |n, id| n.saturating_add(self.tokens[*id as usize].df));
        self.idf(df)
    }

    /// What [`Self::term_hits`] gives one doc for a punctuation token of
    /// candidates `cands` and idf `idf` ([`Self::punct_idf`]), read from
    /// the doc's [`Store::marks`] rather than from every posting: each
    /// candidate in a field scored as its posting is (its count there, the
    /// field's length in words).
    fn punct_hit(&self, doc: u32, cands: &IntMap<u32, Kind>, idf: f32) -> TermHit {
        let mut h = TermHit::default();
        let marks = self.marks_of(doc);
        for (i, m) in marks.iter().enumerate() {
            let same = |o: &Punct| o.id == m.id && o.field == m.field;
            let Some(kind) = cands.get(&m.id) else { continue };
            if marks[..i].iter().any(same) {
                continue;
            }
            let (f, tf) = (m.field as usize, marks.iter().filter(|o| same(o)).count());
            let q = kind.quality();
            let s = q * idf * self.sat(tf.min(255) as u8, self.slot(doc).lens[f], f);
            h.add(f, s, q, false);
        }
        h
    }

    fn term_hits(&self, term: &Term) -> IntMap<u32, TermHit> {
        let mut hits: IntMap<u32, TermHit> = IntMap::default();
        if let [tok] = term.toks.as_slice() {
            let cands = self.candidates(tok, WORD);
            let joint = tok.punct.then(|| self.punct_idf(&cands));
            for (id, kind) in cands {
                let e = &self.tokens[id as usize];
                let (q, idf) = (kind.quality(), joint.unwrap_or_else(|| self.idf(e.df)));
                for p in &e.postings {
                    let f = p.field as usize;
                    let s = q * idf * self.sat(p.tf, p.len, f);
                    hits.entry(p.doc).or_default().add(f, s, q, kind.is_fuzzy());
                }
            }
            return hits;
        }
        // Phrase: consecutive tokens inside one field value.
        let cands: Vec<IntMap<u32, Kind>> = term.toks.iter().map(|t| self.candidates(t, WORD)).collect();
        if cands.iter().any(IntMap::is_empty) {
            return hits;
        }
        let postings_of = |c: &IntMap<u32, Kind>| {
            c.keys()
                .map(|id| self.tokens[*id as usize].postings.len())
                .sum::<usize>()
        };
        let pivot = (0..cands.len()).min_by_key(|&i| postings_of(&cands[i])).unwrap_or(0);
        let mut pairs: Vec<(u32, u8)> = cands[pivot]
            .keys()
            .flat_map(|id| self.tokens[*id as usize].postings.iter().map(|p| (p.doc, p.field)))
            .collect();
        pairs.sort_unstable();
        pairs.dedup();
        let n = cands.len();
        // (doc, field, tf, best quality, fuzzy)
        let mut found: Vec<(u32, usize, u32, f32, bool)> = Vec::new();
        for (doc, field) in pairs {
            let slot = self.slot(doc);
            let (mut tf, mut best_q, mut best_fuzzy) = (0u32, 0f32, false);
            for v in slot.fields[field as usize].iter().filter(|v| v.len() >= n) {
                for w in v.windows(n) {
                    let mut q = 1f32;
                    let mut fuzzy = false;
                    let ok = w.iter().zip(&cands).all(|(id, c)| match c.get(id) {
                        Some(k) => {
                            q = q.min(k.quality());
                            fuzzy |= k.is_fuzzy();
                            true
                        }
                        None => false,
                    });
                    if ok {
                        tf += 1;
                        if q > best_q {
                            best_q = q;
                            best_fuzzy = fuzzy;
                        }
                    }
                }
            }
            if tf > 0 {
                found.push((doc, field as usize, tf, best_q, best_fuzzy));
            }
        }
        let mut docs: Vec<u32> = found.iter().map(|f| f.0).collect();
        docs.sort_unstable();
        docs.dedup();
        let idf = self.idf(docs.len() as u32);
        for (doc, field, tf, q, fuzzy) in found {
            let len = self.slot(doc).lens[field];
            let s = q * idf * self.sat(tf.min(255) as u8, len, field);
            hits.entry(doc).or_default().add(field, s, q, fuzzy);
        }
        hits
    }

    pub(crate) fn search(&self, query: &Query, viewer: &Viewer, opts: &SearchOptions) -> Outcome {
        let limit = if opts.limit == 0 { DEFAULT_LIMIT } else { opts.limit };
        let filters = Filters::new(self, query, viewer);

        let mut seen: Vec<Vec<String>> = Vec::new();
        let mut terms: Vec<Term> = Vec::new();
        // The punctuation of the terms (the `?` of `what?`), each once.
        let mut puncts: Vec<QTok> = Vec::new();
        for text in &query.text {
            let (toks, punct): (Vec<QTok>, Vec<QTok>) = prepare(text).into_iter().partition(|t| !t.punct);
            for p in punct {
                if puncts.len() < MAX_USER_TERMS && !puncts.iter().any(|q| q.text == p.text) {
                    puncts.push(p);
                }
            }
            let key: Vec<String> = toks.iter().map(|t| t.text.clone()).collect();
            if toks.is_empty() || seen.contains(&key) || seen.len() >= MAX_USER_TERMS {
                continue;
            }
            terms.push(Term {
                toks,
                weight: 1.0,
                user_bit: Some(seen.len() as u32),
            });
            seen.push(key);
        }
        // A query of punctuation only (`?`, `...`): each is a term, found in
        // the name and the text; no expansions and no image side, as for
        // filters. With words, it lifts what they find (below).
        let only_punct = terms.is_empty() && !puncts.is_empty();
        if only_punct {
            for (bit, p) in std::mem::take(&mut puncts).into_iter().enumerate() {
                terms.push(Term {
                    toks: vec![p],
                    weight: 1.0,
                    user_bit: Some(bit as u32),
                });
            }
        }
        let n_user = terms.len();
        if n_user == 0 {
            return self.browse(&filters, query.sort, opts.offset, limit);
        }
        let expansions = if only_punct { &[][..] } else { &opts.expansions[..] };
        for (text, weight) in expansions.iter().take(MAX_EXPANSIONS) {
            let weight = if weight.is_nan() { 0.0 } else { weight.clamp(0.0, 1.0) };
            let mut toks = prepare(text);
            toks.retain(|t| !t.punct);
            let key: Vec<String> = toks.iter().map(|t| t.text.clone()).collect();
            if weight <= 0.0 || toks.is_empty() || seen.contains(&key) {
                continue;
            }
            terms.push(Term {
                toks,
                weight,
                user_bit: None,
            });
            seen.push(key);
        }

        // Lexical side.
        let mut acc: IntMap<u32, Acc> = IntMap::default();
        for term in &terms {
            for (doc, h) in self.term_hits(term) {
                let s: f32 = (0..NFIELDS).map(|f| FIELD_WEIGHT[f] * h.field[f]).sum();
                if s <= 0.0 {
                    continue;
                }
                let a = acc.entry(doc).or_default();
                a.score += term.weight * s;
                match term.user_bit {
                    Some(bit) => {
                        a.mask |= 1 << bit;
                        for f in (0..NFIELDS).filter(|f| h.field[*f] > 0.0) {
                            a.labels |= FIELD_LABEL[f];
                        }
                        if h.fuzzy {
                            a.labels |= FUZZY;
                        }
                    }
                    None => a.labels |= EXPANSION,
                }
            }
        }
        // The punctuation beside words: it lifts the items a word found
        // that have it too (a term that is never required and finds
        // nothing by itself), and does not count for coordination. Read
        // from the items that pass, with the labels it adds.
        let lifts: Vec<(IntMap<u32, Kind>, f32)> = puncts
            .iter()
            .map(|p| self.punct_candidates(p))
            .filter(|c| !c.is_empty())
            .map(|c| {
                let idf = self.punct_idf(&c);
                (c, idf)
            })
            .collect();
        let mut lifted: IntMap<u32, u16> = IntMap::default();
        let by_score = |a: &(u32, f32), b: &(u32, f32)| {
            b.1.total_cmp(&a.1)
                .then_with(|| tiebreak(&self.slot(a.0).doc, &self.slot(b.0).doc))
        };
        let mut lexical: Vec<(u32, f32)> = Vec::with_capacity(acc.len());
        for (d, a) in acc.iter().filter(|(d, _)| filters.pass(**d, self.slot(**d))) {
            let mut score = a.score;
            for (cands, idf) in lifts.iter().filter(|_| a.mask != 0) {
                let h = self.punct_hit(*d, cands, *idf);
                score += (0..NFIELDS).map(|f| FIELD_WEIGHT[f] * h.field[f]).sum::<f32>();
                let labels = (0..NFIELDS)
                    .filter(|f| h.field[*f] > 0.0)
                    .fold(0, |l, f| l | FIELD_LABEL[f]);
                if labels != 0 {
                    *lifted.entry(*d).or_default() |= labels;
                }
            }
            // Coordination: reward docs that match more of the user's terms;
            // expansion-only matches count as half a term.
            let matched = (a.mask.count_ones() as f32).max(0.5);
            lexical.push((*d, score * matched / n_user as f32));
        }
        lexical.sort_by(by_score);

        // Vector side.
        let mut vector: Vec<(u32, f32)> = Vec::new();
        if let Some(qv) = opts.text_embedding.as_deref().filter(|v| !v.is_empty() && !only_punct) {
            let norm = dot(qv, qv).sqrt();
            if norm.is_finite() && norm > 0.0 {
                let qv: Vec<f32> = qv.iter().map(|x| x / norm).collect();
                for (idx, slot) in self.slots.iter().enumerate() {
                    let Some(slot) = slot else { continue };
                    let Some(e) = slot.doc.embedding.as_deref() else {
                        continue;
                    };
                    if e.len() != qv.len() || !filters.pass(idx as u32, slot) {
                        continue;
                    }
                    let c = dot(&qv, e);
                    if c.is_finite() && c >= opts.min_similarity {
                        vector.push((idx as u32, c));
                    }
                }
                vector.sort_by(by_score);
            }
        }

        // Reciprocal-rank fusion.
        let mut fused: IntMap<u32, Row> = IntMap::default();
        let rrf = |rank: usize| 1.0 / (RRF_K + rank as f32 + 1.0);
        let row = |idx: u32| Row {
            idx,
            score: 0.0,
            labels: 0,
            tier: filters.texts.tier(idx),
        };
        for (rank, (idx, _)) in lexical.iter().enumerate() {
            let r = fused.entry(*idx).or_insert_with(|| row(*idx));
            r.score += rrf(rank);
            r.labels |= acc[idx].labels;
            if !lifted.is_empty() {
                r.labels |= lifted.get(idx).copied().unwrap_or(0);
            }
        }
        for (rank, (idx, _)) in vector.iter().enumerate() {
            let r = fused.entry(*idx).or_insert_with(|| row(*idx));
            r.score += rrf(rank);
            r.labels |= IMAGE;
        }
        let rows: Vec<Row> = fused.into_values().collect();
        self.page(rows, query.sort, false, opts.offset, limit)
    }

    /// Filter-only query: every visible doc passing the filters; with a
    /// `text:` value, of the docs it matched.
    fn browse(&self, filters: &Filters, sort: Sort, offset: usize, limit: usize) -> Outcome {
        let row = |idx: u32| Row {
            idx,
            score: 0.0,
            labels: 0,
            tier: filters.texts.tier(idx),
        };
        let rows: Vec<Row> = match &filters.texts.any {
            Some(docs) => docs
                .keys()
                .filter(|i| filters.pass(**i, self.slot(**i)))
                .map(|i| row(*i))
                .collect(),
            None => self
                .slots
                .iter()
                .enumerate()
                .filter_map(|(i, s)| s.as_ref().filter(|s| filters.pass(i as u32, s)).map(|_| row(i as u32)))
                .collect(),
        };
        self.page(rows, sort, true, offset, limit)
    }

    fn page(&self, mut rows: Vec<Row>, sort: Sort, browse: bool, offset: usize, limit: usize) -> Outcome {
        let cmp = |a: &Row, b: &Row| -> Ordering {
            let (da, db) = (&self.slot(a.idx).doc, &self.slot(b.idx).doc);
            let by_score = || b.score.total_cmp(&a.score);
            let primary = match sort {
                // How well a `text:` value matched, then as without one.
                Sort::Relevance if browse => a
                    .tier
                    .cmp(&b.tier)
                    .then_with(|| db.copies.cmp(&da.copies))
                    .then_with(|| db.likes.cmp(&da.likes)),
                Sort::Relevance => a.tier.cmp(&b.tier).then_with(by_score),
                Sort::Recent => db.updated_at.cmp(&da.updated_at).then_with(by_score),
                Sort::Likes => db.likes.cmp(&da.likes).then_with(by_score),
                Sort::Copies => db.copies.cmp(&da.copies).then_with(by_score),
                Sort::Quality => db.quality.cmp(&da.quality).then_with(by_score),
            };
            primary.then_with(|| tiebreak(da, db))
        };
        let total = rows.len();
        let end = offset.saturating_add(limit).min(total);
        if offset >= end {
            return Outcome {
                hits: Vec::new(),
                total,
            };
        }
        if end < total {
            rows.select_nth_unstable_by(end, cmp);
            rows.truncate(end);
        }
        rows.sort_by(cmp);
        let hits = rows
            .drain(offset..)
            .map(|r| Hit {
                item_id: self.slot(r.idx).doc.item_id.clone(),
                score: r.score,
                matched: LABELS
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| r.labels & (1 << i) != 0)
                    .map(|(_, l)| *l)
                    .collect(),
                text_tier: r.tier,
            })
            .collect();
        Outcome { hits, total }
    }
}
