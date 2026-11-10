# Search

stickpile has one search engine: the in-memory hybrid index in
`crates/stickpile-search`. The server runs it over the whole library. Clients
run the same code over their offline cache. It does not depend on the
database, so a later PostgreSQL backend needs no new search code (see the
"Search" row in `docs/plan.md`).

```
human e:😂 set:cats vibe:tired q>70 type:gif tag:reaction -tag:nsfw likes>=2 sort:likes
```

## Query syntax

The parser is `stickpile_api::query::Query::parse`
(`crates/stickpile-api/src/query.rs`). That module is the grammar's source of
truth. Server, CLI, desktop popup and mobile clients all parse the same way.

| Syntax | Meaning |
|---|---|
| `word`, `"a phrase"` | Soft text term, ranked. A phrase (or a term with inner punctuation such as `big-chungus`) must match consecutive words. |
| `-word`, `-"a phrase"` | No text field may contain this word or phrase (whole words, after normalisation). |
| `e:😂`, `emoji:😂` | The item has this emoji. |
| `set:cats`, `set:"my cats"` | Set slug or set name (case- and accent-insensitive). The set id also works. |
| `vibe:tired` | Exact vibe label, case-insensitive. |
| `tag:reaction` | Exact tag, case-insensitive. |
| `type:gif` | `still`/`image`, `animated`/`animation`, `video`, or a format: `png jpeg jpg webp gif mp4 webm`. |
| `by:alice`, `user:alice` | Owner username, case-insensitive. |
| `is:mine`, `is:liked`, `is:public` | Own items, items you liked, items in public sets. |
| `q>70`, `likes>=2`, `copies<5` | Comparisons with `> >= < <= =`. Several are ANDed: `q>=50 q<90`. Items without a quality score never pass a `q` filter. |
| `sort:relevance` (default), `sort:recent`/`new`, `sort:likes`, `sort:copies`/`used`/`popular`, `sort:quality` | Result order. |

Rules:

- Repeating a filter means OR: `set:cats set:dogs`, `type:gif type:webp`.
- A leading `-` negates any filter (`-tag:nsfw`, `-e:😂`, `-is:mine`). Every
  negation must hold.
- Different filters are ANDed.
- Unknown `key:value` tokens are plain text.

Examples:

| Query | Finds |
|---|---|
| `naahh` | Items with keyword `nah` or `naaah` (squashed match). With the server's vibe expansion it also finds items whose vibe is `dismissive`. |
| `chungs` | `Big Chungus` (one typo). |
| `"big chungus" type:gif` | GIFs whose name, keyword, tag, … contains the words `big chungus` in that order. |
| `кіт -tag:nsfw` | Ukrainian `Кіт` in any field, without the `nsfw` tag. |
| `cafe` | Also `Café`, and the other way round. |
| `😂` | Items with 😂 in their emojis (ranked first) or in their text. |
| `laugh` | Also items tagged with 😂 or 🤣 (built-in emoji vocabulary). |
| `e:👍` | 👍 in any skin tone. `e:👍🏽` matches only that tone. |
| `set:cats likes>=3 sort:likes` | The cats set, items with at least 3 likes, most liked first ("hide below N likes"). |
| `is:liked sort:recent` | Your likes, newest first. |
| `vibe:tired -is:mine` | Tired items from other users' public sets. |

## Visibility

A viewer can find an item if and only if the viewer owns it, or the item's
set is **public**:

| | own set | other's private | other's unlisted | other's public |
|---|---|---|---|---|
| search, offline cache | ✓ | ✗ | ✗ | ✓ |

Unlisted sets of other users can be opened by link but never appear in
search. The index checks visibility before any other filter and applies it
to both the lexical and the vector side. A viewer with an empty `user_id`
(anonymous) sees only public items. Admins get no extra search access.

## Matching

**Normalisation.** `stickpile_search::normalize` applies NFC, then NFKD with
combining marks removed, then lowercasing. So `Café` → `cafe`,
`ｃａｔ` → `cat`, `ﬁ` → `fi`, `ё` → `е`. Cyrillic stays Cyrillic. Letters
where the mark makes a different letter (`й ї ў ќ ѓ`) and kana are kept
whole. Variation selectors (U+FE0F) are dropped, so `❤️` equals `❤`.

**Tokens.** Text is split on Unicode (UAX #29) word boundaries:

- `_ . :` also separate words (`my_cats`, `file.png`).
- Apostrophes are removed (`can't` → `cant`, `п'ять` → `пять`).
- Each emoji is a token of its own. ZWJ sequences and flags stay whole, and
  skin tones are dropped (`👍🏽` → `👍`).

**Per-token match kinds.** Each query word is looked up in the vocabulary of
indexed tokens. Emoji tokens only match exactly.

| Kind | Rule | Quality |
|---|---|---|
| exact | Same token. | 1.0 |
| squashed | Equal after collapsing letter runs (`squash`: `naahh` = `nah` = `naaah`) and then repeated letter pairs (`hahaha` = `haha`, `lololol` = `lol`). | 0.8 |
| prefix | Query word ≥ 2 chars is a prefix (`chun` → `chungus`). | 0.7 |
| fuzzy | Optimal-string-alignment Damerau-Levenshtein ≤ 1 for words ≥ 4 chars, ≤ 2 for ≥ 7 chars. Not used for words containing digits. | 0.5 |
| substring | Query word ≥ 3 chars inside a token (`ungu` → `chungus`). | 0.3 |

When one vocabulary token matches in several ways, the best quality counts.
The index never scans every document or every token:

- prefix: a range scan of the ordered vocabulary;
- squashed: a `squashed form → tokens` map;
- fuzzy and substring: a padded-trigram index over the vocabulary. A fuzzy
  candidate must share at least `max(|Tq|, |Tc|) − 4·edits` distinct
  trigrams with the query (one edit destroys at most 4 of either word's),
  and is then verified exactly. A substring candidate must contain all of
  the query's interior trigrams.

A property test checks that every single-edit variant of a word still finds
it.

**Phrases** (multi-word terms) match when consecutive tokens inside one field
value match position by position. Each position may match by any kind, so
`"big chun"` works while typing. A phrase's quality is that of its weakest
position.

## Ranking

### Lexical score

Fields and weights:

| Field | Weight | BM25 `b` | Label |
|---|---|---|---|
| name | 3.0 | 0.75 | `name` |
| keywords | 2.5 | 0.4 | `keyword` |
| emojis | 2.0 | 0.3 | `emoji` |
| tags | 2.0 | 0.3 | `tag` |
| vibe | 1.5 | 0 | `vibe` |
| set name + slug | 1.0 | 0.5 | `set` |
| emoji vocabulary (😂 → joy laugh lol tears…) | 1.0 | 0.3 | `emoji` |

For each query term `t` and each document:

```
field_score(t, f) = max over matching tokens m:  quality(m) · idf(m) · sat(tf, len_f)
idf(m)            = ln(1 + (N − df_m + 0.5) / (df_m + 0.5))    df over all docs, any field
sat(tf, len)      = tf · (k1 + 1) / (tf + k1 · (1 − b_f + b_f · len / avglen_f)),  k1 = 1.2
term_score(t)     = Σ_f weight_f · field_score(t, f)
lexical           = Σ_t weight_t · term_score(t)  ·  max(matched_user_terms, 0.5) / user_terms
```

- `avglen_f` is averaged over documents where the field is non-empty, so
  rarely used fields such as tags are not penalised.
- A phrase uses its own document frequency (documents containing the phrase).
- The last factor is coordination: a document that matches more of the
  user's words ranks higher, so `sad cat` puts "sad cat" above plain "cat".
  A document matched only through expansions counts as half a term.

What this gives, and what the tests check:

- An exact name match beats a keyword substring match.
- name > keyword > tag > vibe > set for the same word.
- exact > squashed > prefix > fuzzy > substring for the same field.
- Rare words outweigh common ones.

### Expansions

`SearchOptions::expansions` holds extra `(term, weight)` pairs with weights
in 0..=1. They go through the same matching as user terms. Their score is
multiplied by the weight, they do not count for coordination, and documents
they reach get the label `expansion` (no field labels). An expansion that
equals a user term is ignored. The index does not compute expansions; the
caller does. The server builds them from:

- **Vibe synonyms.** `stickpile_ai::Vocabulary::vibes_for_word("naahh")`
  returns `["dismissive"]`, which becomes `("dismissive", 0.6)`. This needs
  no model, so clients that ship the vocabulary can do it offline too.
- **AI related terms.** `ZeroShot::related_terms(text_embedding, k)` returns
  the nearest vocabulary keywords and vibes, thresholded by the server (CLIP
  text-text cosines of unrelated words are high; see `docs/ai.md`), with a
  weight scaled from the cosine (for example 0.3–0.5).

The built-in emoji vocabulary (`src/emoji_names.rs`, about 170 common
sticker emojis) is part of the index, not an expansion. With it, `cat`
reaches items tagged 🐱 and `laugh` reaches 😂, labelled `emoji`. Exclusions
(`-cat`) ignore it.

### Vector score

If `SearchOptions::text_embedding` is set (the query text through the CLIP
text encoder, in the image embedding space), every visible document that
passes the filters and has an embedding of the same dimension is scored by
dot product. Embeddings are L2-normalised; the index re-normalises the query
vector. Documents with cosine ≥ `min_similarity` form the vector ranking.

The default `min_similarity` of 0.0 would let almost everything through,
because CLIP text-image cosines are rarely negative. The server should set
it to about 0.2–0.25 for ViT-B/32, in line with
`ZeroShotParams::keyword_min_similarity`.

### Fusion and order

- **Fusion.** Lexical and vector rankings are combined with reciprocal-rank
  fusion: `score = Σ 1 / (60 + rank)`, ranks from 1. A document found by
  both sides gets both contributions. A document found only by the vector
  side still appears, labelled `image`. `Hit::score` is the fused score.
- **Ties** go to copies (desc), then `updated_at` (desc), then `item_id`.
- **Filter-only queries** (no text, or only punctuation) return every
  visible document that passes the filters. They are ordered by copies,
  likes, then `updated_at` (all descending), with score 0. Expansions and
  the embedding are ignored.
- **`sort:`** `recent`, `likes`, `copies` and `quality` override the order
  for any query but never add results. Ties fall back to the fused score,
  then the tie-breaks above. Items without a quality score sort last.
- **Paging.** `total` counts all matches before `offset`/`limit`.
  `limit == 0` means 50 (`stickpile_search::DEFAULT_LIMIT`). The index does
  not cap `limit`; the server does.

`Hit::matched` lists, in this order, whichever apply: `name`, `keyword`,
`emoji`, `vibe`, `tag`, `set` (fields where a user term matched); `fuzzy`
(the best match for some user term was fuzzy or squashed); `expansion`;
`image`.

## Keeping the index current

`Index` is a plain value. Wrap it in a lock to share it; it is
`Send + Sync + Clone`.

| Call | When |
|---|---|
| `upsert(doc)` | Item created or its text, set, visibility or owner changed. Replaces by `item_id`; old tokens stop matching. |
| `remove(item_id)` / `remove_set(set_id)` | Item or set deleted, or a set made unreachable. |
| `set_counters(item_id, likes, copies)` | After a like or a usage event. Cheap: no re-indexing. |
| `set_embedding(media_id, emb)` | When the AI job finishes. Applies to every item sharing that media. |

Visibility is stored per document. When a set's visibility, name or slug
changes, re-upsert its items. Tokens nobody uses any more are dropped from
the vocabulary and all side indexes. A randomised test checks that an index
built by incremental upserts, removals and counter updates answers exactly
like one rebuilt from scratch.

## Offline behaviour

Clients feed the same `Index` from their cache
(`stickpile_core::cache::OfflineIndex`). The cache holds the user's own items
and public items, i.e. what search may return.
Embeddings are not cached and no text encoder runs on the device, so
offline search is lexical only: normalisation, squash, prefix, fuzzy,
phrases, the emoji vocabulary and every filter work as on the server; image
similarity and AI related terms do not. Vibe-synonym expansion works offline
if the client ships the vibe vocabulary. When the server is reachable,
clients use `GET /api/v1/search` (`stickpile_api::routes::SEARCH`), which
adds the vector side.

## Limits and performance

- Up to 16 distinct soft terms and 32 expansions per query; the rest are
  ignored.
- Term frequency is capped at 255 and field length at 65535 tokens.
- Memory grows linearly: one posting (8 bytes) per distinct token per field
  per item, plus the stored `Doc` and its embedding (2 KiB at 512 × f32).
  The plan sizes it for libraries of up to about 100k items.

Measured with the `#[ignore]`d `perf_synthetic_library` test, release build,
i7-14700HX, one thread. Each synthetic doc has 1–3 name words, 2–6
keywords, 0–2 tags, 0–2 emojis, a vibe, one of 300 sets, a 512-dim
embedding, and a vocabulary of about 25k words. Latency is the mean of 20
runs.

```
cd /data/stickpile && nix develop -c cargo test -p stickpile-search --release -- --ignored --nocapture perf
```

| | 10k docs | 50k docs |
|---|---|---|
| indexing throughput | ~80k docs/s (125 ms) | ~92k docs/s (0.54 s) |
| exact word | 0.04 ms | 0.05 ms |
| 2-char prefix (`ve`, 4.3k hits at 50k) | 0.31 ms | 1.5 ms |
| 3-char prefix | 0.05 ms | 0.24 ms |
| typo (transposition) | 0.03 ms | 0.03 ms |
| 7-char word (squashed + 2-edit fuzzy pass) | 0.36 ms | 0.40 ms |
| two words | 0.07 ms | 0.10 ms |
| phrase | 0.06 ms | 0.07 ms |
| word + filters | 0.03 ms | 0.03 ms |
| filter-only (`type:gif q>50`) | 0.22 ms | 1.4 ms |
| browse everything visible (18k at 50k) | 0.23 ms | 2.0 ms |
| word + 512-dim embedding (vector scan) | 0.63 ms | 5.2 ms |
| word + 3 expansions | 0.58 ms | 3.1 ms |

The vector side is a linear scan over the visible, filtered documents. It is
the most expensive part, and fine at this scale. If libraries grow well past
100k items, an ANN index would replace the scan behind the same API.
