//! Index storage: document slots, the inverted index and its side indexes
//! (prefix via the ordered vocabulary, squashed forms, trigrams), and the
//! maintenance operations that keep them consistent.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;

use crate::text::{emoji_base, emoji_key, normalize, padded_trigrams, squash_key, tokenize, Token};
use crate::{emoji_names, Doc};

/// Multiplicative hasher for integer keys (ids, trigram keys); not DoS-resistant,
/// which is fine for keys the index generates itself.
#[derive(Default, Clone, Copy)]
pub(crate) struct IntHasher(u64);

const HASH_K: u64 = 0x517c_c1b7_2722_0a95;

impl Hasher for IntHasher {
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0.rotate_left(5) ^ *b as u64).wrapping_mul(HASH_K);
        }
    }
    fn write_u32(&mut self, n: u32) {
        self.0 = (self.0.rotate_left(5) ^ n as u64).wrapping_mul(HASH_K);
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(HASH_K);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

pub(crate) type IntMap<K, V> = HashMap<K, V, BuildHasherDefault<IntHasher>>;

/// Indexed text fields. The order matches the label order of [`crate::Hit::matched`].
pub(crate) const NAME: usize = 0;
pub(crate) const KEYWORD: usize = 1;
pub(crate) const EMOJI: usize = 2;
pub(crate) const VIBE: usize = 3;
pub(crate) const TAG: usize = 4;
pub(crate) const SET: usize = 5;
/// Words derived from the item's emojis (😂 -> laugh, joy...). Reported as `emoji`.
pub(crate) const EMOJI_NAME: usize = 6;
pub(crate) const NFIELDS: usize = 7;

/// Per-field weight of a match.
pub(crate) const FIELD_WEIGHT: [f32; NFIELDS] = [3.0, 2.5, 2.0, 1.5, 2.0, 1.0, 1.0];
/// BM25 length normalisation per field: names are prose-like, keyword/tag/emoji
/// lists are bags where one more entry should not punish much, vibe is one word.
pub(crate) const FIELD_B: [f32; NFIELDS] = [0.75, 0.4, 0.3, 0.0, 0.3, 0.5, 0.3];
/// Label bit of each field (see `LABELS`).
pub(crate) const FIELD_LABEL: [u16; NFIELDS] = [1, 1 << 1, 1 << 2, 1 << 3, 1 << 4, 1 << 5, 1 << 2];

#[derive(Debug, Clone, Copy)]
pub(crate) struct Posting {
    pub doc: u32,
    /// Tokens in this field of this doc.
    pub len: u16,
    pub tf: u8,
    pub field: u8,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TokenEntry {
    pub text: String,
    pub chars: Box<[char]>,
    pub word: bool,
    /// Distinct padded trigrams (word tokens only), for the fuzzy count filter.
    pub trigrams: u16,
    /// Number of docs containing the token in any field.
    pub df: u32,
    /// One entry per (doc, field).
    pub postings: Vec<Posting>,
}

#[derive(Debug, Clone)]
pub(crate) struct Slot {
    pub doc: Doc,
    /// Per field, per value (one keyword, one tag...), the token ids in order.
    pub fields: [Vec<Vec<u32>>; NFIELDS],
    pub lens: [u16; NFIELDS],
    /// Emojis with variation selectors removed, and additionally without skin tones.
    pub emoji_keys: Vec<String>,
    pub emoji_bases: Vec<String>,
    pub set_slug: String,
    pub set_name: String,
    pub vibe: Option<String>,
    pub tags: Vec<String>,
    pub owner: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Store {
    pub slots: Vec<Option<Slot>>,
    free_slots: Vec<u32>,
    by_id: HashMap<String, u32>,
    by_media: HashMap<String, Vec<u32>>,
    /// Ordered for prefix scans.
    pub vocab: BTreeMap<String, u32>,
    pub tokens: Vec<TokenEntry>,
    free_tokens: Vec<u32>,
    /// `squash_key(token)` -> tokens.
    pub squashed: HashMap<String, Vec<u32>>,
    /// Padded trigram -> word tokens containing it.
    pub trigrams: IntMap<u64, Vec<u32>>,
    pub field_len_sum: [u64; NFIELDS],
    pub field_nonempty: [u32; NFIELDS],
}

fn analyze(doc: &Doc) -> [Vec<Vec<Token>>; NFIELDS] {
    let many = |vals: &[String]| -> Vec<Vec<Token>> { vals.iter().map(|v| tokenize(v)).collect() };
    let mut f: [Vec<Vec<Token>>; NFIELDS] = Default::default();
    f[NAME] = vec![tokenize(&doc.name)];
    f[KEYWORD] = many(&doc.keywords);
    f[EMOJI] = many(&doc.emojis);
    f[VIBE] = doc.vibe.iter().map(|v| tokenize(v)).collect();
    f[TAG] = many(&doc.tags);
    let set_name = tokenize(&doc.set_name);
    let set_slug = tokenize(&doc.set_slug);
    f[SET] = if set_slug == set_name {
        vec![set_name]
    } else {
        vec![set_name, set_slug]
    };
    let mut seen = HashSet::new();
    let names: Vec<Vec<Token>> = f[EMOJI]
        .iter()
        .flatten()
        .filter(|t| !t.word && seen.insert(t.text.as_str()))
        .filter_map(|t| emoji_names::names(&t.text))
        .map(tokenize)
        .collect();
    f[EMOJI_NAME] = names;
    for vals in f.iter_mut() {
        vals.retain(|v| !v.is_empty());
    }
    f
}

impl Store {
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn slot(&self, idx: u32) -> &Slot {
        self.slots[idx as usize].as_ref().expect("live slot")
    }

    pub fn get(&self, item_id: &str) -> Option<&Doc> {
        self.by_id.get(item_id).map(|&i| &self.slot(i).doc)
    }

    fn slot_mut(&mut self, idx: u32) -> &mut Slot {
        self.slots[idx as usize].as_mut().expect("live slot")
    }

    pub fn upsert(&mut self, doc: Doc) {
        if let Some(&idx) = self.by_id.get(&doc.item_id) {
            self.remove_many(&[idx]);
        }
        self.insert(doc);
    }

    pub fn remove(&mut self, item_id: &str) {
        if let Some(&idx) = self.by_id.get(item_id) {
            self.remove_many(&[idx]);
        }
    }

    pub fn remove_set(&mut self, set_id: &str) {
        let idxs: Vec<u32> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.as_ref().is_some_and(|s| s.doc.set_id == set_id))
            .map(|(i, _)| i as u32)
            .collect();
        self.remove_many(&idxs);
    }

    pub fn set_counters(&mut self, item_id: &str, likes: u64, copies: u64) {
        if let Some(&idx) = self.by_id.get(item_id) {
            let doc = &mut self.slot_mut(idx).doc;
            doc.likes = likes;
            doc.copies = copies;
        }
    }

    pub fn set_embedding(&mut self, media_id: &str, embedding: Option<Arc<[f32]>>) {
        let Some(idxs) = self.by_media.get(media_id).cloned() else {
            return;
        };
        for idx in idxs {
            self.slot_mut(idx).doc.embedding = embedding.clone();
        }
    }

    fn intern(&mut self, tok: &Token) -> u32 {
        if let Some(&id) = self.vocab.get(&tok.text) {
            return id;
        }
        let id = self.free_tokens.pop().unwrap_or_else(|| {
            self.tokens.push(TokenEntry::default());
            (self.tokens.len() - 1) as u32
        });
        let chars: Box<[char]> = tok.text.chars().collect();
        let mut trigrams = 0u16;
        if tok.word {
            self.squashed.entry(squash_key(&tok.text)).or_default().push(id);
            let tris = padded_trigrams(&chars);
            trigrams = tris.len().min(u16::MAX as usize) as u16;
            for t in tris {
                self.trigrams.entry(t).or_default().push(id);
            }
        }
        self.vocab.insert(tok.text.clone(), id);
        self.tokens[id as usize] = TokenEntry {
            text: tok.text.clone(),
            chars,
            word: tok.word,
            trigrams,
            df: 0,
            postings: Vec::new(),
        };
        id
    }

    fn insert(&mut self, doc: Doc) {
        let analyzed = analyze(&doc);
        let idx = self.free_slots.pop().unwrap_or_else(|| {
            self.slots.push(None);
            (self.slots.len() - 1) as u32
        });
        let mut fields: [Vec<Vec<u32>>; NFIELDS] = Default::default();
        let mut lens = [0u16; NFIELDS];
        let mut distinct: Vec<u32> = Vec::new();
        for (f, values) in analyzed.iter().enumerate() {
            fields[f] = values
                .iter()
                .map(|v| v.iter().map(|t| self.intern(t)).collect())
                .collect();
            let len: usize = fields[f].iter().map(Vec::len).sum();
            if len == 0 {
                continue;
            }
            lens[f] = len.min(u16::MAX as usize) as u16;
            self.field_len_sum[f] += len as u64;
            self.field_nonempty[f] += 1;
            let mut ids: Vec<u32> = fields[f].iter().flatten().copied().collect();
            ids.sort_unstable();
            for run in ids.chunk_by(|a, b| a == b) {
                let posting = Posting {
                    doc: idx,
                    len: lens[f],
                    tf: run.len().min(255) as u8,
                    field: f as u8,
                };
                self.tokens[run[0] as usize].postings.push(posting);
                distinct.push(run[0]);
            }
        }
        distinct.sort_unstable();
        distinct.dedup();
        for id in distinct {
            self.tokens[id as usize].df += 1;
        }
        let emoji_keys: Vec<String> = doc.emojis.iter().map(|e| emoji_key(e)).collect();
        let emoji_bases = emoji_keys.iter().map(|e| emoji_base(e)).collect();
        let slot = Slot {
            fields,
            lens,
            emoji_keys,
            emoji_bases,
            set_slug: normalize(doc.set_slug.trim()),
            set_name: normalize(doc.set_name.trim()),
            vibe: doc.vibe.as_deref().map(|v| normalize(v.trim())),
            tags: doc.tags.iter().map(|t| normalize(t.trim())).collect(),
            owner: doc.owner_username.trim().to_lowercase(),
            doc,
        };
        self.by_id.insert(slot.doc.item_id.clone(), idx);
        self.by_media.entry(slot.doc.media_id.clone()).or_default().push(idx);
        self.slots[idx as usize] = Some(slot);
    }

    /// Remove docs in one pass over each affected posting list, so dropping a
    /// large set does not rescan the same long lists once per doc.
    fn remove_many(&mut self, idxs: &[u32]) {
        if idxs.is_empty() {
            return;
        }
        let removed: HashSet<u32> = idxs.iter().copied().collect();
        let mut df_dec: IntMap<u32, u32> = IntMap::default();
        for &idx in idxs {
            let slot = self.slots[idx as usize].take().expect("live slot");
            self.by_id.remove(&slot.doc.item_id);
            if let Some(list) = self.by_media.get_mut(&slot.doc.media_id) {
                list.retain(|i| *i != idx);
                if list.is_empty() {
                    self.by_media.remove(&slot.doc.media_id);
                }
            }
            let mut distinct: Vec<u32> = Vec::new();
            for (f, values) in slot.fields.iter().enumerate() {
                let len: usize = values.iter().map(Vec::len).sum();
                if len > 0 {
                    self.field_len_sum[f] -= len as u64;
                    self.field_nonempty[f] -= 1;
                }
                distinct.extend(values.iter().flatten());
            }
            distinct.sort_unstable();
            distinct.dedup();
            for id in distinct {
                *df_dec.entry(id).or_default() += 1;
            }
            self.free_slots.push(idx);
        }
        let mut dead: Vec<u32> = Vec::new();
        for (id, dec) in df_dec {
            let e = &mut self.tokens[id as usize];
            e.postings.retain(|p| !removed.contains(&p.doc));
            e.df -= dec;
            if e.postings.is_empty() {
                dead.push(id);
            }
        }
        self.drop_tokens(&dead);
    }

    /// Forget tokens no doc uses any more, from the vocabulary and side indexes.
    fn drop_tokens(&mut self, ids: &[u32]) {
        if ids.is_empty() {
            return;
        }
        let dead: HashSet<u32> = ids.iter().copied().collect();
        let mut tri_keys: Vec<u64> = Vec::new();
        let mut squash_keys: Vec<String> = Vec::new();
        for &id in ids {
            let e = std::mem::take(&mut self.tokens[id as usize]);
            self.vocab.remove(&e.text);
            if e.word {
                squash_keys.push(squash_key(&e.text));
                tri_keys.extend(padded_trigrams(&e.chars));
            }
            self.free_tokens.push(id);
        }
        tri_keys.sort_unstable();
        tri_keys.dedup();
        for k in tri_keys {
            if let Some(list) = self.trigrams.get_mut(&k) {
                list.retain(|id| !dead.contains(id));
                if list.is_empty() {
                    self.trigrams.remove(&k);
                }
            }
        }
        squash_keys.sort_unstable();
        squash_keys.dedup();
        for k in squash_keys {
            if let Some(list) = self.squashed.get_mut(&k) {
                list.retain(|id| !dead.contains(id));
                if list.is_empty() {
                    self.squashed.remove(&k);
                }
            }
        }
    }

    /// Live vocabulary size (for tests and stats).
    #[cfg(test)]
    pub fn vocab_len(&self) -> usize {
        self.vocab.len()
    }
}
