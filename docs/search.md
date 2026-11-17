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
| `-word`, `-"a phrase"` | No text field (the item text included) may contain this word or phrase (whole words, after normalisation). |
| `e:😂`, `emoji:😂` | The item has this emoji. |
| `e:skull`, `emoji:thumbs_up` | The item has an emoji this English name selects ([emoji names](#emoji-names)): `e:skull` is 💀, ☠️ and the others with a word starting with `skull` in their name or keywords. |
| `set:cats`, `s:cats`, `set:"my cats"` | Set slug or set name (case- and accent-insensitive). The set id also works. |
| `vibe:tired`, `v:tired` | Exact vibe label, case-insensitive. |
| `tag:reaction`, `t:reaction` | Exact tag, case-insensitive. |
| `type:gif` | `still`/`image`, `animated`/`animation`, `video`, or a format: `png jpeg jpg webp gif mp4 webm`. |
| `by:alice`, `b:alice`, `user:alice` | Owner username, case-insensitive. |
| `text:"no thoughts"`, `x:thoughts` | The item text (the words written in the picture) has these words, one after another, after normalisation: whole words (`text:thought` does not find `thoughts`), no typos. Only the text: a name with the words does not count. |
| `is:mine`, `is:liked`, `is:public` | Own items, items you liked, items in public sets. |
| `q>70`, `likes>=2`, `copies<5` | Comparisons with `> >= < <= =`. Several are ANDed: `q>=50 q<90`. Items without a quality score never pass a `q` filter. |
| `sort:relevance` (default), `sort:recent`/`new`, `sort:likes`, `sort:copies`/`used`/`popular`, `sort:quality` | Result order. |

Rules:

- Repeating a filter means OR: `set:cats set:dogs`, `type:gif type:webp`.
- A leading `-` negates any filter (`-tag:nsfw`, `-e:😂`, `-is:mine`). Every
  negation must hold.
- Different filters are ANDed.
- Keys are case-insensitive. `e:`, `s:`, `v:`, `t:`, `b:` and `x:` are
  the one-letter forms of `emoji:`, `set:`, `vibe:`, `tag:`, `by:` and
  `text:` (`stickpile_api::query::Key`): `-v:tired`, `t:"two words"` and
  the suggestions work as with the long keys. The normalised query
  (`Query`'s `Display`) writes the long keys, and `e:` for emojis. The
  desktop popup and the Android app send them long
  (`stickpile_quick::suggest::resolve`), so a server older than the short
  keys reads them too.
- `text:` values are kept lowercased in `Query.texts`; the search
  tokenizes them like the indexed text and wants the run of tokens in the
  item's text. A value whose words no item has finds nothing, and so does
  a value without a word (`text:!!!`). Nothing is offered while a `text:`
  value is typed (`stickpile_quick::suggest::Field::of` gives `None`): the
  words are free. A server from before the text field (0.7.0) reads
  `text:hi` as the plain words `text:hi`.
- An `e:` value made of letters (with `_` or `-` between words) is an
  emoji name, anything else an emoji. `Query.emojis` keeps the values as
  typed (the normalised query writes `e:skull`); `stickpile-search` turns a
  name into the emojis it selects when it builds its filter. A name that
  selects nothing finds nothing: it is not searched as text. A server
  from before emoji names (0.7.0) compares the name as an emoji, so
  `e:skull` finds nothing there.
- While `e:` is typed, the suggestion line (`stickpile_quick::suggest`,
  and the Android hints) offers the emojis of the viewer's items, most
  used first, each with its English name (`Suggestions::labels`); while a
  name is typed (`e:sk`), the emojis it selects, the viewer's own first,
  then the others in the order of
  [`emoji_names::by_name`](#emoji-names). Taking one puts the emoji into
  the query (`e:💀`). `resolve` never rewrites an `e:` value: a name goes
  to the server as typed, and the server forgives its typos itself. The
  web UI's search fields get the same offers from the server
  (`GET /api/v1/search/suggest`, [web.md](web.md#keyboard-and-mouse)), which runs
  `suggest` over the values of the items the viewer can search.
- Unknown `key:value` tokens are plain text.

Examples:

| Query | Finds |
|---|---|
| `naahh` | Items with keyword `nah` or `naaah` (squashed match). With the server's vibe expansion it also finds items whose vibe is `dismissive`. |
| `chungs` | `Big Chungus` (one typo). |
| `"big chungus" type:gif` | GIFs whose name, keyword, tag, … contains the words `big chungus` in that order. |
| `кіт -tag:nsfw` | Ukrainian `Кіт` in any field, without the `nsfw` tag. |
| `thoughts` | Also items with "no thoughts" written in the picture, below those with it in their name or keywords. |
| `text:"no thoughts"` | Items whose picture says "no thoughts" (or "NO THOUGHTS", across a line break too), not "thoughts? no". |
| `x:кіт -tag:nsfw` | Items with `Кіт` written on them, without the `nsfw` tag. |
| `cafe` | Also `Café`, and the other way round. |
| `😂` | Items with 😂 in their emojis (ranked first) or in their text. |
| `laugh` | Also items tagged with 😂 or 🤣 (built-in emoji vocabulary). |
| `e:👍` | 👍 in any skin tone. `e:👍🏽` matches only that tone. |
| `e:skull` | Items with 💀 or ☠️, also 🩻 (x-ray: keyword skull) and 👲 (person with skullcap). Not an item that is only named "skull". |
| `-e:skull` | Items with none of those. |
| `e:sklul` | Items with 💀, ☠️ or 🩻: the whole word `skull` is one typo away (`skullcap` is not). |
| `e:thumbs` | 👍 and 👎, in any skin tone. |
| `set:cats likes>=3 sort:likes` | The cats set, items with at least 3 likes, most liked first ("hide below N likes"). |
| `is:liked sort:recent` | Your likes, newest first. |
| `vibe:tired -is:mine` | Tired items from other users' public sets. |
| `v:tired t:reaction -t:nsfw` | The same as `vibe:tired tag:reaction -tag:nsfw`. |

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
- Apostrophes are removed (`can't` → `cant`, `п'ять` → `пять`). One
  between two letters goes before the text is split, in each form people
  type it: `'`, `’` (U+2019), `ʼ` (U+02BC) and a backtick (a keyboard
  without the apostrophe), so `п'ятниця`, `п’ятниця`, `пʼятниця` and
  ``п`ятниця`` are all the word `пятниця`, in the items and in queries.
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

**The item text** (`Doc::text`, the words written in the picture, read by
OCR or typed; see [ai.md](ai.md#ocr-and-the-text-field)) is indexed as one
value, so a phrase may run over a line break. Plain words and phrases
match it like any field; `text:` matches it alone, exact tokens in a row.

### Emoji names

An `e:` name selects emojis by their English names
(`stickpile_search::emoji_names::by_name`). Case and accents are
ignored; `_` and `-` separate words.

- It selects every emoji with a word that starts with it, in its CLDR
  short name or in one of its keywords: `e:skull` selects 💀 (skull), ☠️
  (skull and crossbones), 🩻 (x-ray, keyword skull) and 👲 (person with
  skullcap). Several words must follow each other in the name or in one
  keyword, and the last may be cut short: `e:thumbs_u` is 👍.
- When no word starts with it and it has 4 letters or more, whole words
  one edit away (optimal string alignment distance 1) select instead:
  `e:sklul` selects the emojis with the word `skull` (not `skullcap`).
  One edit in all, also over several words.
- An item passes when it has any of the selected emojis, compared as the
  emoji filter compares, without variation selectors and skin tones
  (`e:thumbs` finds 👍🏽); `-e:skull` leaves out items with any of them.

The order of `by_name`, which the suggestion line uses: whole words before
the start of one, a word of the name before one of a keyword, then the
curated emojis of `emoji_names.rs` (the most used on stickers first), then
Unicode's order. `emoji_names::short_name` gives the name an emoji is
labelled with (`skull`), skin tones and variation selectors aside.

The names come from one table, `crates/stickpile-search/src/emoji_names.tsv`
(90 kB; 1,914 emojis): the emojis of Unicode `emoji-test.txt` 17.0 without
skin-tone variants (hair-style variants included), their English CLDR 48.2
short names and keywords, and the curated words of `emoji_names.rs`
(`lol`, `sus`). `scripts/gen-emoji-data.py` writes it together with the web
picker's `emoji.json`; a test fails when `emoji_names.rs` has a word the
table lacks, so regenerate it after changing them. The table is compiled
into the server and every client, so they all select the same emojis. It
adds about 120 kB to a release binary (90 kB of data, 30 kB of code;
measured on the x86_64 `stickpile` CLI) and is parsed on first use (about
3 ms). The names are English only: the Ukrainian CLDR names and keywords
would add about 212 kB of data (the names alone 60 kB) to every client.

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
| text (the words in the picture) | 1.5 | 0.75 | `text` |

The text weighs less than the name and the keywords: it is often long,
OCR makes it noisy, and a word in a caption says less about the sticker
than its name. Its `b` is the prose value, so a word in a long text counts
less than in a short one. A word said again in the text counts once (`tf`
is 1 in the text): a caption saying "NO NO NO" says `no` once, so it does
not outrank the keyword `no`, and `😂😂😂` in the text does not outrank
😂 in the emojis.

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
- name > keyword > text for the same word, also when the text says it
  several times.
- An emoji in the emojis > the same emoji in the text.
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
(`-cat`) ignore it. `e:` names use the complete names table instead
([emoji names](#emoji-names)), which has these words too.

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
`image`; `text` (a user term matched the item text; last, so the order of
the older labels stays).

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
and public items, i.e. what search may return, as `ItemSummary`, whose
`text` is the item text: plain words and `text:` find it offline as on the
server. A cache an older client wrote holds items without `text`
(`cache::ITEM_FORMAT` 1, recorded as `item_format` in its `meta` table):
the next sync lists everything again, once, and records format 2. The
schema stays, so an older client still opens the cache; items it syncs
afterwards have no text until they change on the server.
Embeddings are not cached and no text encoder runs on the device, so
offline search is lexical only: normalisation, squash, prefix, fuzzy,
phrases, the emoji vocabulary, emoji names and every filter work as on the
server; image similarity and AI related terms do not. Vibe-synonym
expansion works offline if the client ships the vibe vocabulary. When the
server is reachable, clients use `GET /api/v1/search`
(`stickpile_api::routes::SEARCH`), which adds the vector side.

The values the clients suggest while a `key:` filter is typed
([desktop.md](desktop.md#search-suggestions)) come from the same cache, so
they follow the sync. `Session::sync` tells whether the cached items
changed (an item new, changed or removed, or another account's dropped; an
item the server sends again as it was is no change), and a client reads
its suggestions again after such a sync, showing or not. The popup model
takes `Event::LibraryChanged`, which the desktop and Mac shells dispatch
after such a sync, and answers it with `Command::LoadVocabulary`; a
suggestion line on screen takes the new values in place (the query, the
cursor and the selection stay). The search goes out again when the cache
answered it, or when a typed value now resolves to another one; results
the server gave stay. On Android the sync calls
return `SyncState.cache_changed`, and the app and the keyboard then ask
for the offers on screen again. So the first opening after a start, while
the cache is still empty, gets the values as soon as its sync is over.

## Inside a pack

With a pack open, the clients search its items by themselves, online and
offline alike: the desktop popup (GTK and macOS) in the field of an opened
pack, the Android app with a pack open and the keyboard with a set open
(`StickpileLibrary::pack_search`). `stickpile_quick::within` puts the
pack's items (as the client loaded them for the pack) into an `Index` of
their own and searches it with the whole syntax:

- Every filter works as above. `is:public` follows the pack's visibility,
  `is:mine` compares the owner with the viewer's username (from the
  cache), `is:liked` uses the likes the client knows.
- The pack's name and slug are not indexed as words (every item has
  them, so `cats` would find the whole Cats pack); `set:` compares them
  (and the pack's id) itself. The item text is indexed, so words and
  `text:` find it.
- Words are ranked as above. As the pack's old substring filter did, a
  word also finds the items with it anywhere in the name, a keyword, an
  emoji or a tag (`c`, `at` while `cat` is typed); those come after the
  ranked ones, in pack order.
- Filters alone keep the pack's order; `sort:` other than relevance sorts
  as above.
- No embeddings and no expansions: image similarity and vibe synonyms
  are left out, as offline.

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
cd /data/stickpile && nix develop -c cargo test -p stickpile-search --release -- --ignored --nocapture perf_synthetic
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
| emoji name (`e:skull`, 1.7k hits at 50k) | 0.67 ms | 2.3 ms |
| 1-letter emoji name (`e:s`, 747 emojis) | 0.65 ms | 2.6 ms |

`perf_emoji_names`, run alone so that the names table is not loaded yet
(`... --nocapture perf_emoji_names`): the first use parses the table in
about 3 ms; then a name takes 0.2 to 0.3 ms, and 0.9 ms when no word
starts with it (the typo pass); `short_name` under 1 us.

The vector side is a linear scan over the visible, filtered documents. It is
the most expensive part, and fine at this scale. If libraries grow well past
100k items, an ANN index would replace the scan behind the same API.
