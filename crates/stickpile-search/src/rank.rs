//! Query evaluation: filters, lexical scoring, vector scoring, fusion, ordering.

use std::cmp::Ordering;
use std::ops::Bound;

use stickpile_api::query::{Filter, Is, Query, Sort};
use stickpile_api::Visibility;

use crate::store::{IntMap, Slot, Store, EMOJI_NAME, FIELD_B, FIELD_LABEL, FIELD_WEIGHT, NFIELDS};
use crate::text::{
    dot, emoji_key, is_skin_tone, normalize, osa_within, padded_trigrams, squash_key, tokenize, tri_is_padded,
};
use crate::{Doc, Hit, Outcome, SearchOptions, Viewer};

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
const LABELS: [&str; 9] = [
    "name",
    "keyword",
    "emoji",
    "vibe",
    "tag",
    "set",
    "fuzzy",
    "expansion",
    "image",
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
}

struct QTok {
    text: String,
    chars: Vec<char>,
    word: bool,
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
        })
        .collect()
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
}

/// Filters and visibility, prepared once per query.
struct Filters<'a> {
    q: &'a Query,
    viewer: &'a Viewer,
    /// (key without variation selectors, query has a skin tone)
    emojis: Filter<(String, bool)>,
    sets: Filter<String>,
    vibes: Filter<String>,
    tags: Filter<String>,
    owners: Filter<String>,
    /// Token-id sequences that must not occur in any own text field.
    exclude: Vec<Vec<u32>>,
}

fn map_filter<T, U>(f: &Filter<T>, g: impl Fn(&T) -> U) -> Filter<U> {
    Filter {
        any: f.any.iter().map(&g).collect(),
        none: f.none.iter().map(&g).collect(),
    }
}

impl<'a> Filters<'a> {
    fn new(store: &Store, q: &'a Query, viewer: &'a Viewer) -> Filters<'a> {
        let norm = |s: &String| normalize(s.trim());
        let exclude = q
            .exclude_text
            .iter()
            .filter_map(|t| {
                let toks = tokenize(t);
                if toks.is_empty() {
                    return None;
                }
                // A token missing from the vocabulary cannot occur in any doc.
                toks.iter()
                    .map(|t| store.vocab.get(&t.text).copied())
                    .collect::<Option<Vec<u32>>>()
            })
            .collect();
        Filters {
            q,
            viewer,
            emojis: map_filter(&q.emojis, |e| {
                let k = emoji_key(e);
                let toned = k.chars().any(is_skin_tone);
                (k, toned)
            }),
            sets: map_filter(&q.sets, norm),
            vibes: map_filter(&q.vibes, norm),
            tags: map_filter(&q.tags, norm),
            owners: map_filter(&q.owners, |s| s.trim().to_lowercase()),
            exclude,
        }
    }

    fn visible(&self, doc: &Doc) -> bool {
        doc.visibility == Visibility::Public || (!self.viewer.user_id.is_empty() && doc.owner_id == self.viewer.user_id)
    }

    fn pass(&self, s: &Slot) -> bool {
        let d = &s.doc;
        let q = self.q;
        self.visible(d)
            && q.quality_ok(d.quality)
            && q.likes_ok(d.likes)
            && q.copies_ok(d.copies)
            && q.types.accepts(|t| t.matches(d.kind, d.format))
            && q.is.accepts(|i| match i {
                Is::Mine => !self.viewer.user_id.is_empty() && d.owner_id == self.viewer.user_id,
                Is::Liked => self.viewer.liked.contains(&d.item_id),
                Is::Public => d.visibility == Visibility::Public,
            })
            && self.emojis.accepts(|(k, toned)| {
                if *toned {
                    s.emoji_keys.iter().any(|e| e == k)
                } else {
                    s.emoji_bases.iter().any(|e| e == k)
                }
            })
            && self
                .sets
                .accepts(|v| s.set_slug == *v || s.set_name == *v || d.set_id == *v)
            && self.vibes.accepts(|v| s.vibe.as_deref() == Some(v.as_str()))
            && self.tags.accepts(|v| s.tags.contains(v))
            && self.owners.accepts(|v| s.owner == *v)
            && !self.excluded(s)
    }

    fn excluded(&self, s: &Slot) -> bool {
        self.exclude.iter().any(|seq| {
            s.fields[..EMOJI_NAME]
                .iter()
                .flatten()
                .any(|v| v.windows(seq.len()).any(|w| w == seq.as_slice()))
        })
    }
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
        let tf = tf as f32;
        tf * (K1 + 1.0) / (tf + K1 * (1.0 - b + b * len as f32 / avg.max(1e-3)))
    }

    /// Vocabulary tokens matching one query token, with the best match kind.
    fn candidates(&self, q: &QTok) -> IntMap<u32, Kind> {
        let mut best: IntMap<u32, Kind> = IntMap::default();
        let mut put = |id: u32, k: Kind| {
            best.entry(id)
                .and_modify(|e| *e = if k.quality() > e.quality() { k } else { *e })
                .or_insert(k);
        };
        if let Some(&id) = self.vocab.get(&q.text) {
            put(id, Kind::Exact);
        }
        if !q.word {
            return best;
        }
        let n = q.chars.len();
        if n >= 2 {
            let range = self
                .vocab
                .range::<str, _>((Bound::Excluded(q.text.as_str()), Bound::Unbounded));
            for (_, &id) in range.take_while(|(k, _)| k.starts_with(&q.text)) {
                put(id, Kind::Prefix);
            }
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
            if substring && interior > 0 && inner == interior && e.text.contains(&q.text) {
                put(id, Kind::Substring);
            }
        }
        best
    }

    fn term_hits(&self, term: &Term) -> IntMap<u32, TermHit> {
        let mut hits: IntMap<u32, TermHit> = IntMap::default();
        if let [tok] = term.toks.as_slice() {
            for (id, kind) in self.candidates(tok) {
                let e = &self.tokens[id as usize];
                let (q, idf) = (kind.quality(), self.idf(e.df));
                for p in &e.postings {
                    let f = p.field as usize;
                    let s = q * idf * self.sat(p.tf, p.len, f);
                    hits.entry(p.doc).or_default().add(f, s, q, kind.is_fuzzy());
                }
            }
            return hits;
        }
        // Phrase: consecutive tokens inside one field value.
        let cands: Vec<IntMap<u32, Kind>> = term.toks.iter().map(|t| self.candidates(t)).collect();
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
        for text in &query.text {
            let toks = prepare(text);
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
        let n_user = terms.len();
        if n_user == 0 {
            return self.browse(&filters, query.sort, opts.offset, limit);
        }
        for (text, weight) in opts.expansions.iter().take(MAX_EXPANSIONS) {
            let weight = if weight.is_nan() { 0.0 } else { weight.clamp(0.0, 1.0) };
            let toks = prepare(text);
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
        let by_score = |a: &(u32, f32), b: &(u32, f32)| {
            b.1.total_cmp(&a.1)
                .then_with(|| tiebreak(&self.slot(a.0).doc, &self.slot(b.0).doc))
        };
        let mut lexical: Vec<(u32, f32)> = acc
            .iter()
            .filter(|(d, _)| filters.pass(self.slot(**d)))
            .map(|(d, a)| {
                // Coordination: reward docs that match more of the user's terms;
                // expansion-only matches count as half a term.
                let matched = (a.mask.count_ones() as f32).max(0.5);
                (*d, a.score * matched / n_user as f32)
            })
            .collect();
        lexical.sort_by(by_score);

        // Vector side.
        let mut vector: Vec<(u32, f32)> = Vec::new();
        if let Some(qv) = opts.text_embedding.as_deref().filter(|v| !v.is_empty()) {
            let norm = dot(qv, qv).sqrt();
            if norm.is_finite() && norm > 0.0 {
                let qv: Vec<f32> = qv.iter().map(|x| x / norm).collect();
                for (idx, slot) in self.slots.iter().enumerate() {
                    let Some(slot) = slot else { continue };
                    let Some(e) = slot.doc.embedding.as_deref() else {
                        continue;
                    };
                    if e.len() != qv.len() || !filters.pass(slot) {
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
        for (rank, (idx, _)) in lexical.iter().enumerate() {
            let r = fused.entry(*idx).or_insert(Row {
                idx: *idx,
                score: 0.0,
                labels: 0,
            });
            r.score += rrf(rank);
            r.labels |= acc[idx].labels;
        }
        for (rank, (idx, _)) in vector.iter().enumerate() {
            let r = fused.entry(*idx).or_insert(Row {
                idx: *idx,
                score: 0.0,
                labels: 0,
            });
            r.score += rrf(rank);
            r.labels |= IMAGE;
        }
        let rows: Vec<Row> = fused.into_values().collect();
        self.page(rows, query.sort, false, opts.offset, limit)
    }

    /// Filter-only query: every visible doc passing the filters.
    fn browse(&self, filters: &Filters, sort: Sort, offset: usize, limit: usize) -> Outcome {
        let rows: Vec<Row> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                s.as_ref().filter(|s| filters.pass(s)).map(|_| Row {
                    idx: i as u32,
                    score: 0.0,
                    labels: 0,
                })
            })
            .collect();
        self.page(rows, sort, true, offset, limit)
    }

    fn page(&self, mut rows: Vec<Row>, sort: Sort, browse: bool, offset: usize, limit: usize) -> Outcome {
        let cmp = |a: &Row, b: &Row| -> Ordering {
            let (da, db) = (&self.slot(a.idx).doc, &self.slot(b.idx).doc);
            let by_score = || b.score.total_cmp(&a.score);
            let primary = match sort {
                Sort::Relevance if browse => db.copies.cmp(&da.copies).then_with(|| db.likes.cmp(&da.likes)),
                Sort::Relevance => by_score(),
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
            })
            .collect();
        Outcome { hits, total }
    }
}
