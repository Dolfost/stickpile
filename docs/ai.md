# AI enrichment

stickpile runs one CLIP model in-process on the CPU (crate `stickpile-ai`,
ONNX Runtime through `ort`). There is no LLM and nothing leaves the server.
From one 512-d image embedding per item it derives:

- **text → image search**: a query is embedded with the text encoder and
  compared with the image embeddings (cosine similarity),
- **keywords** from a configurable vocabulary (zero-shot),
- **emojis** from a vocabulary of emojis with descriptions (zero-shot),
- **a vibe** from a fixed set: happy, sad, sarcastic, angry, tired, cringe,
  wholesome, confused, smug, scared, love, shocked, dismissive, excited, bored
  and awkward. No vibe is set when the model is unsure.

It also feeds search expansion. `Vocabulary::vibes_for_word` maps slang to
vibes without a model (`naahh` → dismissive), and `ZeroShot::related_terms`
maps a query embedding to nearby keywords and vibes.

It also has the seam for reading the words written in a picture (OCR)
into the item text ([OCR and the text field](#ocr-and-the-text-field)); no
reader ships yet.

What it does **not** do: captions, face recognition, NSFW classification,
or any training or fine-tuning on your library. A caption backend can be
added later behind the same trait (see the last section).

## How results are marked AI-generated

`stickpile-ai` only returns values. It never writes them anywhere. Under the
plan's data model, the server stores them with `keywords_source`,
`emojis_source` and `vibe_source` set to `ai`. They become `user` when
someone edits or confirms them in the web UI, and `set.toml` export carries
the same `*_source = "ai"` fields. Re-enrichment must only replace fields
whose source is still `ai`. The item text (`text_source`) has a stricter
rule, below.

Next to every embedding the server stores `Enricher::model_id()`; a different
id triggers re-embedding. `Enricher::image_model_id()` is `clip-vit-b32` for
both text encoders, so switching `text_encoder` does not require
re-embedding images, only re-running zero-shot suggestions (their prompts
change). `ZeroShot::vocab_fingerprint()` (16 hex chars) identifies the
vocabulary that produced a set of suggestions.

| `text_encoder` | `model_id` |
|---|---|
| `clip` | `clip-vit-b32+clip-text` |
| `multilingual` | `clip-vit-b32+mclip-v1` |

## Suggestions before an upload

The web upload tray asks for keywords, emojis and a vibe while files are
staged, so they can be reviewed before anything is uploaded
([web.md](web.md#upload-and-import)). The endpoint is
`POST /api/v1/ai/suggest` ([api.md](api.md#ai-suggestions)): one small
image in, `{keywords, emojis, vibe}` out, nothing stored. With a text
reader it also reads the words in the image (`text`; the image is one
frame). It runs the same
code as the `Enrich` job (`service::ai::embed_and_suggest`: embed the image,
then `ZeroShot::suggest` with `max_keywords`, `max_emojis` and the default
thresholds), on the blocking thread pool, rate limited per user
(`limits.ai_suggest_per_minute`). It answers 503 `unavailable` when AI is
off or the model failed to load, and 503 `ai_loading` with `Retry-After: 2`
while the model loads; the tray asks again then (for up to three minutes)
and says it is waiting, and says when AI is off on the server.

**Timing** (measured on a laptop CPU with `text_encoder = "multilingual"`,
`threads = 2`, a debug build, Chromium): one request takes about 0.1 s on
the server and the first one after startup is no slower, so the model needs
no warm-up; from adding files to the fields being filled took 0.15 s for one
file and 0.4 s for five (two requests at a time). Startup is the slow part:
the model loads in under a second, embedding the vocabulary (about 1,400
prompts) takes about 8 s, and until then suggestions answer `ai_loading`.
Files added meanwhile are filled within 2 s of the model being ready (it
was up to 10 s with the earlier `Retry-After: 10`).

The browser renders what the server's thumbnail would show and sends that,
not the file: a PNG with the longest side 256 px (the default
`media.thumb_px`; CLIP looks at a 224 × 224 centre crop anyway). An
animation sends its first frame and a video the frame at a tenth of its
length, the frames the server's thumbnails use. So requests stay small (the
endpoint takes at most 2 MiB), the server only ever decodes a still, and the
suggestions match what enrichment picks from the thumbnail after the upload.

Approval decides the source. The tray uploads approved or edited values as
the user's (`*_source = user`) and lists unapproved ones in
`UploadMeta.ai_fields`, which stores them with `*_source = ai`, like
enrichment results. The item page then shows them with the AI badge and
"✓ Confirm". Tags are never suggested: they are the user's own labels.

After the upload the `Enrich` job still runs once per new picture, because
it stores the image embedding that search needs. It fills fields by the
usual rule (empty or still `ai`), so approved values are never touched and
an item whose fields are all the user's is not written at all. Unapproved
suggestions are AI values like any other and get the thumbnail's
suggestions, which are normally the same ones. The only extra work is one
inference per staged file in the tray (about 50 ms on the CPU with
`clip`, threads = 2).

## Models and layout

```
<model_dir>/
  clip-vit-b32/vision.onnx        image tower, both modes
  clip-vit-b32/text.onnx          text_encoder = "clip"
  clip-vit-b32/tokenizer.json     text_encoder = "clip"
  multilingual/model.onnx         text_encoder = "multilingual"
  multilingual/tokenizer.json     text_encoder = "multilingual"
  multilingual/dense.safetensors  text_encoder = "multilingual"
```

| File | Source (pinned revision) | Size | sha256 |
|---|---|---|---|
| `clip-vit-b32/vision.onnx` | [Xenova/clip-vit-base-patch32](https://huggingface.co/Xenova/clip-vit-base-patch32) `onnx/vision_model.onnx` (fp32), rev `6b7c5237` | 351.7 MB | `fd6e1402…7d40` |
| `clip-vit-b32/text.onnx` | same repo, `onnx/text_model.onnx` (fp32) | 254.1 MB | `3f6571f5…a8cc` |
| `clip-vit-b32/tokenizer.json` | same repo, `tokenizer.json` | 2.2 MB | `f7f3b7af…90cd` |
| `multilingual/model.onnx` | [sentence-transformers/clip-ViT-B-32-multilingual-v1](https://huggingface.co/sentence-transformers/clip-ViT-B-32-multilingual-v1) `onnx/model_qint8_avx512.onnx`, rev `58edf8ca` | 135.3 MB | `3bed77e8…a8a7` |
| `multilingual/tokenizer.json` | same repo, `tokenizer.json` | 2.0 MB | `5b4e1a81…df81` |
| `multilingual/dense.safetensors` | same repo, `2_Dense/model.safetensors` | 1.6 MB | `d12568dc…4f74` |

The full hashes and URLs are in `scripts/fetch-models.sh`, `nix/models.nix`
and `stickpile_ai::MODEL_FILES`; a unit test keeps the three in sync.
Download size: `clip` ≈ 608 MB, `multilingual` ≈ 490 MB, both ≈ 745 MB.

- **Vision**: CLIP ViT-B/32 `CLIPVisionModelWithProjection`. Input
  `pixel_values` [N,3,224,224]; output `image_embeds` [N,512] (projected).
- **CLIP text**: `CLIPTextModelWithProjection`. Input `input_ids` [N,L], with
  at most 77 tokens including BOS/EOS. Output `text_embeds` [N,512]. Byte-level
  BPE, lowercased, padded with `<|endoftext|>`. Padding after EOS cannot
  change the result (causal attention, pooled at the first EOS).
- **Multilingual text**: DistilBERT (`input_ids`, `attention_mask` →
  `last_hidden_state` [N,L,768]), at most 128 tokens (the sentence-transformers
  `max_seq_length`). Then mean pooling over the attention mask and
  `Dense(768→512, bias=false, activation=Identity)` (`linear.weight`
  [512,768], per `2_Dense/config.json`). It was distilled onto the CLIP text
  tower for 50+ languages, so it shares the image space.
- All embeddings are L2-normalised, so cosine = dot product.

### Why these files

Measured against the fp32 exports on 31 images and 2857 prompts (cosine
between the fp32 embedding and the variant's):

| Variant | image mean / min | text mean / min | Verdict |
|---|---|---|---|
| CLIP `*_quantized` (uint8 dynamic) | 0.937 / 0.876 | 0.929 / 0.638 | too lossy; it also squeezes text-text similarities ("cat"~"dismissive" 0.92) |
| CLIP `*_int8` | 0.973 / 0.928 | 0.926 / 0.617 | too lossy |
| CLIP `*_q4` (4-bit weights) | 0.979 / 0.955 | 0.988 / 0.945 | lossy |
| CLIP `*_fp16` | 1.0000 | 1.0000 | lossless, but on CPU ONNX Runtime re-casts the weights on every run: +40 % image latency, 3× single-text latency and **1.35 GB** peak RSS vs 0.73 GB. It also needs graph optimisation ≤ level 2 because of an ORT fusion bug |
| multilingual `qint8` | – | 0.9989 / 0.9931 | **used**: near-lossless at a quarter of the size |

So the CLIP towers ship as fp32 and the multilingual model as qint8. The
`qint8_avx512` file is byte-identical to `qint8_arm64` and runs on any CPU
through ONNX Runtime's generic integer kernels. It was tested on a CPU
without AVX-512. If you put the fp16 CLIP exports in the model dir they
still load: the session builder falls back to basic graph optimisation.

Correctness checks:

- **fp32 CLIP reproduces the Hugging Face reference.** On the COCO image
  `000000039769.jpg` (two cats), `100·cos` for "a photo of a cat" / "a photo
  of a dog" is 24.55 / 19.28. The `transformers` docs give 24.57 / 19.30; the
  small residual comes from `image`-crate vs PIL bicubic resampling.
- **The multilingual pipeline lines up with CLIP.** The same English
  sentence through mclip and through the CLIP text tower gives mean cosine
  0.953 over 2857 prompts. `mclip("фото кота")` vs `clip("a photo of a
  cat")` is 0.961, and `mclip("кіт")` vs `mclip("cat")` is 0.983.

## Fetching

Script (curl plus sha256 verification; keeps files that are already
correct):

```sh
scripts/fetch-models.sh --english      /var/lib/stickpile/models   # default
scripts/fetch-models.sh --multilingual /var/lib/stickpile/models
scripts/fetch-models.sh --all          /var/lib/stickpile/models
```

Nix (fixed-output derivations, assembled with `linkFarm`):

```nix
let models = import ./nix/models.nix { inherit pkgs; };
in {
  services.stickpile.settings.ai = {
    enabled = true;
    model_dir = "${models.all}";        # or models.english / models.multilingual
    onnxruntime_library = "${pkgs.onnxruntime}/lib/libonnxruntime.so";
  };
}
```

A downloader inside the server can use `stickpile_ai::MODEL_FILES` (path,
URL, sha256, size) and `TextEncoderKind::model_files()`.
`stickpile_ai::missing_model_files(dir, encoder)` lists what is absent, for
`check-config` and startup errors.

### Runtime dependency: ONNX Runtime

`ort` is built with `load-dynamic`, so the binary has no link-time
dependency. `libonnxruntime.so` is loaded at runtime from the first of:

1. `[ai] onnxruntime_library`,
2. the `ORT_DYLIB_PATH` environment variable (the dev shell sets it to
   nixpkgs `onnxruntime`),
3. `libonnxruntime.so` through the dynamic loader.

The crate uses `ort` `=2.0.0-rc.15` (ORT C API version 17), tested with
nixpkgs `onnxruntime` 1.24.4. Newer runtimes are backwards compatible; ort
only logs a version notice. Runtimes older than 1.17 are rejected. The
library is loaded once per process. A missing path is reported without
touching ONNX Runtime. A library that fails to load (wrong file, wrong
architecture) gives `AiError::Model`, not a crash, and needs a restart once
fixed. ORT telemetry is disabled.

## Configuration (`[ai]` in the server config)

```toml
[ai]
enabled = true
model_dir = "/var/lib/stickpile/models"   # layout above
text_encoder = "clip"                     # or "multilingual"
# onnxruntime_library = "/nix/store/…-onnxruntime-1.24.4/lib/libonnxruntime.so"
threads = 2                               # intra-op threads per inference; 0 = one per core
# vocabulary_file = "/etc/stickpile/vocabulary.toml"
max_keywords = 5                          # upper bound; fewer when the model is unsure
max_emojis = 3
ocr = true                                # read the words in pictures, when a text reader is there
```

The server maps these to `ClipOptions { model_dir, text_encoder,
onnxruntime_library, threads }`, `Vocabulary::load` / `Vocabulary::builtin()`
and `ZeroShot::suggest(embedding, max_keywords, max_emojis)`.

Threads and concurrency: `ort` 2.0 runs a session through `&mut Session`,
so each tower (vision and text) sits behind its own `Mutex`. Concurrent
image embeddings queue; an image and a text query can run at the same time.
`ClipEnricher` is `Send + Sync`, so share one behind an `Arc`. Intra-op
spinning is off, so an idle server does not burn CPU after a request.

## Zero-shot details

The vocabulary prompts are embedded once, when `ZeroShot::new` runs: 1365
texts for the built-in vocabulary, batched 32 at a time.

| Kind | Prompts (`{}` = term) | Selection (`ZeroShotParams::default()`) |
|---|---|---|
| keyword | mean of "a meme of {}", "a sticker of {}", "a picture of {}" | best first, at most `max_keywords`, each ≥ best − 0.025 and ≥ 0.22 |
| emoji | mean of "{}", "a sticker of {}" on the description | best first, at most `max_emojis`, each ≥ best − 0.02 and ≥ 0.22 |
| vibe | 0.7 × mean("a {} meme", "a {} reaction sticker") + 0.3 × mean(bare synonyms) | softmax over vibes of 100·cos (CLIP's logit scale); reported only if p ≥ 0.25 |

Keyword and emoji scores are raw cosines; matching image-text pairs land
around 0.27–0.34 for CLIP ViT-B/32. The vibe score is the softmax
probability.

**Calibration.** I used one photo (the COCO cats image) and 30 transparent
Twemoji stickers rendered at 512 px. These were local test data, not shipped.

- **Emoji templates.** I compared six template sets. "{}" + "a sticker of {}"
  was best for CLIP: correct emoji top-1 for 19/30 stickers and top-3 for
  24/30. For multilingual it was 13/30 and 21/30. Descriptions describe the
  picture: "laughing face with tears of joy" ranks 😂 first for both
  encoders, while "face with tears of joy, laughing so hard it cries" ranked
  it 4th / 21st.
- **Keywords.** The best keyword was right for pizza, cat, frog, thumbs up,
  skull, fire, coffee, heart, clown, facepalm and sleeping. For the cats photo
  the selection was sleeping, couple, couch, lying in bed and cat. The 0.025
  margin keeps 3.7 keywords per image on average. Generic face keywords ("sad
  face", "crying face"; "smug face" for multilingual) act as hubs on face
  emojis.
- **Vibes.** I tested 7 label template sets × 7 synonym variants. The chosen
  one labelled 16/22 vibe-bearing stickers correctly for CLIP and 12/22 for
  multilingual. At p ≥ 0.25, CLIP keeps 8 of the 16 correct labels, 0 of the
  wrong ones and 1 of the 8 stickers without an expected vibe (clown →
  happy). Multilingual keeps 6/12 correct and 0 wrong.
- **Language of synonyms.** Including the non-English synonyms in the
  prototypes made no measurable difference for the English CLIP, so all
  synonyms are used for both encoders.

Synthetic stickers from `tests/real_model.rs`, English CLIP:

| Image | Keywords | Emojis | Vibe |
|---|---|---|---|
| drawn smiley | smiley face .327, happy face .326, laughing face .319, smiling .314, smug face .304 | 😀 .306, 😊 .304, 😬 .302 | happy 0.33 |
| drawn frown | sad face .320, crying face .310, angry face .304 | 😞 .310, 😢 .308 | sad 0.32 |
| red heart | heart .304, heart hands .292 | ❤️ .309 | love 0.90 |
| red square | flag .268, … (nothing in the vocabulary fits) | ❌ .289 | none |

**`related_terms(query_embedding, k)`** does a text-text comparison against
bare keywords and against each vibe's label and synonyms (a vibe scores its
best term). It returns the top `k` with cosines. CLIP text-text cosines are
high even for unrelated words ("cat"~"truck" 0.79, "kitty"~"puppy" 0.91,
"cat"~"kitten" 0.94), so an absolute threshold does not work well. Use the
top 1–3 terms, or the terms within about 0.02 of the best. An exact synonym
scores ≈ 1.0.
Examples: "kitty" → kitten .93, cat .93; "exhausted" → tired 1.00 (exact
synonym); multilingual "кіт" → cat .98, kitten .98; "сумно" → sad 1.00.

**`Vocabulary::vibes_for_word(word)`** is lexical and model-free. It
lowercases, strips surrounding punctuation, collapses repeated characters
("naahh" → "nah", "ніііі" → "ні") and repeated two-character units
("hahaha" → "ha", "lolol" → "lol"). The same normalisation (`vocab::squash`)
is applied to labels and synonyms, so "NAAAH", "nooope", "bruuuh" and "ні"
all give `dismissive`, and "yaaay" gives `happy` and `excited`.

## Vocabulary file

`[ai] vocabulary_file` replaces the built-in vocabulary. It has the same
format as `crates/stickpile-ai/src/vocab_default.toml`, which also serves as
the reference list (232 keywords, 122 emojis, 16 vibes with 145 synonyms
including slang and Ukrainian):

```toml
keywords = ["cat", "thumbs up", "this is fine"]

[[emojis]]
emoji = "😂"
description = "laughing face with tears of joy"   # what it looks like; used as the prompt

[[vibes]]
label = "dismissive"                               # [a-z][a-z0-9_-]*, used by `vibe:`
synonyms = ["nah", "nope", "bruh", "ні", "неа"]
```

- Each top-level key is optional. A missing key keeps the built-in list; an
  explicit `[]` disables that kind of suggestion.
- Strings are trimmed and labels lowercased.
- Rejected: empty or overlong terms (> 100 characters), duplicate keywords
  (case-insensitive), duplicate emojis or vibes, ASCII-only "emojis", unknown
  fields, and a single vibe (the softmax needs at least two, or none).
- Keywords work best as short English noun phrases with `clip`. With
  `multilingual` any of its 50+ languages works.

## Performance

Measured on an Intel i7-14700HX (28 threads) shared with parallel builds
(load average 15–40), so these are best-of-30 single calls. Release build,
ONNX Runtime 1.24.4.

| | `clip`, threads = 2 | `clip`, threads = 4 | `multilingual`, threads = 2 |
|---|---|---|---|
| load models | ~1 s | ~1 s | ~0.8 s |
| embed 1365 vocabulary prompts | 3–7 s | 2.6 s | 2.4 s |
| `embed_image` (incl. preprocessing) | 51 ms | 29 ms | 50 ms |
| `embed_text` (one query) | 7.4 ms | 4.8 ms | 3.1 ms |
| `embed_texts`, per text in a batch of 32 | 3.8 ms | 2.0 ms | 1.8 ms |
| peak RSS of the probe process | 740 MB | 730 MB | 630 MB |

`suggest` itself (about 370 dot products of 512 floats) takes microseconds.
To reproduce:

```sh
cargo run --release -p stickpile-ai --example probe -- --models models --encoder clip --threads 2 [image.png…] [--query "text"]
```

## OCR and the text field

**The field.** Items have `text`, the words written in the picture: one
string, line breaks kept, at most 2000 characters
(`stickpile_api::text::clean_text`: `\r\n` and a lone `\r` become `\n`,
tabs spaces, other control characters go, the ends are trimmed), and
`text_source` (`user` or `ai`) in `items.text` / `items.text_source`
(migration `0014`; existing items get `''` and `ai`). For the text, an
empty value with source `ai` means "not set". The computer writes the text
only while its source is `ai`; any edit by a person makes it `user` for
good, clearing included ([api.md](api.md#items)). That is stricter than
the rule of `keywords`, `emojis`, `vibe` and `name` (fill when empty or
still `ai`): an empty text a person left stays empty. Every AI write of
the text goes through `service::ai::apply_text`, whose SQL only touches
`text_source = 'ai'` rows; an import's or a restore's text goes through
`ItemText::merged_into` (a directory without a text changes nothing, an
AI text from a file never replaces a person's).

**The result belongs to the media.** `media.ocr_text` (`NULL`: never read,
`''`: read, nothing confident) and `media.ocr_model` (the reader's
`model_id`), like `embedding` and `embedding_model`. A file is read once,
however many items and sets it is in: a new item of media that the
current reader has read gets the text at once. A result of another model
counts as not read.

**Reading.** The seam is `stickpile_ai::ocr`, next to `Enricher` (no cargo
feature; `MockTextReader` is always there, like `MockEnricher`):

```rust
pub trait TextReader: Send + Sync {
    fn model_id(&self) -> &str;
    fn read(&self, frame: &image::RgbaImage) -> Result<Vec<TextLine>, AiError>;
}
pub struct TextLine { pub text: String, pub confidence: f32, pub bounds: Option<LineBox> }
```

`ocr::read_frames(reader, frames)` scales each frame down to at most
`MAX_READ_PIXELS` (2048 x 2048, aspect kept), reads it, and passes what was
found through `ocr::clean_lines`, the rules every reader shares:

- lines below `MIN_CONFIDENCE` (0.5) are dropped;
- reading order: rows top to bottom (a line joins a row when its vertical
  centre is within half a line height of the row's first line), left to
  right inside a row; lines without boxes keep the reader's order;
- printable text only: `clean_text`, private-use and replacement
  characters removed, whitespace runs as one space, a line without a
  letter or digit (frames, borders, specks) dropped;
- a line already read in an earlier frame (case and spacing aside) is not
  repeated; the same line twice in one frame stays;
- the lines joined with `\n`, cut at 2000 characters.

The frames are `stickpile_media::text_frames` of the stored, sanitised
original (never the raw upload): a still as it is stored; an animation's
first, middle and last frames (fewer when it has fewer); a video's frames
at its start, its middle and one frame before its end (ffmpeg, the same
frame grab as the representative frame).

**Where it runs.**

- The `Enrich` job: after CLIP, or alone when there is a reader but no
  CLIP model. When the media has no result from the reader's model it
  reads it on the blocking pool (no timeout, as for CLIP) and stores
  `ocr_text` and `ocr_model`, then gives the text to the media's items
  whose text is still `ai`. The job result adds `ocr_model`, `text` and
  `text_items_updated`. A failed read fails nothing: it is logged, the
  result says `ocr_error`, and the media stays unread for a later run.
- Uploads queue `Enrich` when the embedding or the OCR result from the
  current reader is missing.
- `POST /api/v1/ai/suggest` reads the posted image and returns `text`
  (nothing stored; a failed read leaves `text` out).
- Startup does not queue reading of media stored before; a manual
  re-enrichment (`POST /api/v1/items/{id}/enrich`) reads a media that was
  never read.

**Configuration.** `[ai] ocr = true` (the default) reads with a text reader
when `ai.enabled`; `false` reads nothing. The server builds no reader yet
(`app::text_reader_from_config` returns none): with `ocr = true` it logs
once at startup ("ai.ocr is on, but this server has no text reader: the
words in pictures are not read") and goes on. Tests give a reader through
`StartOptions::text_reader` (`TextReaderSource::Provided`), which still
obeys `ai.ocr`.

**Set directories and older servers.** `set.toml` carries `text` and
`text_source` ([set-format.md](set-format.md)). A 0.7.0 server keeps them
as unknown keys (`items.extra_toml`) and writes them back; after an
upgrade, the server moves such keys into the item text at startup
(`transfer::lift_text_from_extra`), unless the item has a person's text.

## Tests

- `cargo test -p stickpile-ai` needs neither network nor model files. It
  covers preprocessing (shape, normalisation, alpha flattening, crop,
  extreme aspect ratios), tokenizer padding and truncation with a synthetic
  tokenizer, mean pooling, the dense layer from safetensors, blob round-trip,
  the model file table against the script and the Nix file, vocabulary
  loading and validation, `vibes_for_word`, `ZeroShot` with `MockEnricher`,
  a broken ONNX Runtime library giving an error rather than a panic, and
  the OCR cleaning (confidence, reading order, merging frames, the cap,
  frame bounding) with `MockTextReader`.
- `MockTextReader::new(&lines)` returns the same lines for every frame and
  records the size of each frame it read (`frames()`, `reads()`); the
  server tests (`tests/text.rs`) inject it to check what is read, how
  often, and that a person's text is never overwritten.
- `STICKPILE_TEST_MODEL_DIR=$PWD/models cargo test -p stickpile-ai --release`
  adds the real-model tests (`tests/real_model.rs`):
  - red square vs "a red square" / "a blue square", also in Ukrainian;
  - "cat"~"kitten" > "cat"~"truck";
  - "кіт" ≈ "cat", and mclip ≈ CLIP for the same sentence;
  - batch == single;
  - transparency is treated as white;
  - concurrent use;
  - zero-shot on drawn stickers;
  - query expansion.
- `MockEnricher` (dim ≥ 16) is deterministic. Text is a hashed bag of words
  in which colour words ("red", "blue", …) land on dedicated dimensions.
  Images are colour histograms over the same named colours, so "red" is close
  to an all-red image. Other crates can use it to test search and enrichment
  without models.

## The seam for a caption backend

Everything goes through `stickpile_ai::Enricher`:

```rust
pub trait Enricher: Send + Sync {
    fn model_id(&self) -> &str;
    fn image_model_id(&self) -> &str { self.model_id() }
    fn dim(&self) -> usize;
    fn embed_image(&self, rgba: &image::RgbaImage) -> Result<Vec<f32>, AiError>;
    fn embed_text(&self, text: &str) -> Result<Vec<f32>, AiError>;
    fn embed_texts(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AiError> { /* loop */ }
    fn caption(&self, rgba: &image::RgbaImage) -> Result<Option<String>, AiError> { Ok(None) }
}
```

An OpenAI-compatible captioner (e.g. a local llama.cpp or vLLM server with a
vision model) would be a struct holding a `ClipEnricher` for embeddings and
an HTTP client. It delegates the `embed_*` methods and implements `caption`
by POSTing the image as a `data:image/png;base64,…` URL to
`/v1/chat/completions`. `model_id` stays the CLIP id, because captions do not
change the embedding space. The server would store the caption as another
AI-sourced field and could add caption words to full-text search.
Configuration would be a new `[ai.caption]` table (endpoint, model, API key
file, timeout). Nothing else in the crate needs to change.

## OCR evaluation

Measured in November 2026 to choose the models behind the `text` field
(next section). The harness, pictures and raw results stayed in a scratch
directory; none of it is in the repo.

**Setup.** Python onnxruntime 1.24.4 from nixpkgs, 2 intra-op threads, 1
inter-op thread, full graph optimisation. Reference pre- and
post-processing: RapidOCR 3.8.1 (nixpkgs) for the PaddleOCR models, EasyOCR
1.7.2 (nixpkgs) for EasyOCR, whose networks were exported to ONNX for this
(opset 17, fp32; largest difference to PyTorch 5e-8 for the detector, 3e-5
for the recogniser). Transparency is flattened as RapidOCR does it: on
white when the opaque pixels are dark on average, else on black. Lines at
or above the score threshold are kept, sorted top to bottom, and boxes on
the same row are joined with a space. Intel i7-14700HX shared with other
jobs.

**Test set.** 56 pictures. 29 real: 26 from Wikimedia Commons (CC0, public
domain, CC BY, CC BY-SA) and 3 OpenMoji emoji (CC BY-SA). 27 rendered with
known text in Impact, Oswald, Comic Sans, Arial, Times New Roman, Roboto,
Caveat, Bad Script, Neucha and xkcd Script. Kinds: memes with outlined or
plain captions (12), stickers on transparency or photographed (10), chat,
tweet and web screenshots, light and dark, some with 11 px text (11),
handwriting and handwriting fonts (6), subtitles, a demotivator, a 320 px
JPEG at quality 30, and 11 pictures without text (photos, a brick wall,
emoji, a LEGO face). Languages: English 20, Ukrainian 17 (13 of them
rendered), mixed 6, Russian 2 (only in "all"), none 11. Ground truth was
transcribed by hand, one line per visual line, without incidental text
such as a brand on a collar tag.

**Metrics.** CER is edits divided by reference characters, summed over a
group. "raw" compares the text as it is, lines joined with newlines;
"norm" folds case, maps apostrophe variants to `'` and collapses
whitespace. Word recall is the share of reference words (normalised) found
in the output. False text: characters and pictures with output among the
11 textless pictures. Time: the whole pipeline per picture without
decoding, fastest warm run, then median and p95 over the 56 pictures. Peak
RSS: VmHWM of the Python process, 158 MB of which is the interpreter,
OpenCV and ONNX Runtime before any model is loaded.

### Candidates

| Candidate | Licence code / weights | ONNX | Download | Latin + Cyrillic | Verdict |
|---|---|---|---|---|---|
| PaddleOCR PP-OCRv5 mobile: DB detector, CTC recogniser per script (RapidOCR exports) | Apache-2.0 / Apache-2.0 | yes, dictionary in the model metadata | det 4.8 MB, rec 7.9 to 8.1 MB | `cyrillic` and `eslav`: full Latin plus Cyrillic incl. і ї є ґ | measured |
| PP-OCRv5 server detector | Apache-2.0 / Apache-2.0 | yes | 88.1 MB | - | measured |
| PP-OCRv4 detector + `cyrillic_PP-OCRv3` recogniser | Apache-2.0 / Apache-2.0 | yes | 4.7 + 9.0 MB | dictionary lacks ї and Ї | measured |
| PaddleOCR 0/180 degree line classifier (v2.0) | Apache-2.0 / Apache-2.0 | yes | 0.6 MB | - | measured |
| EasyOCR: CRAFT detector + `cyrillic_g2` CRNN | Apache-2.0 / Apache-2.0 (CRAFT: MIT) | no official export, exported here | 83.1 + 15.3 MB | yes | measured |
| docTR / OnnxTR | Apache-2.0 / Apache-2.0 | yes | multilingual PARSeq 96.7 MB | no: that model's 195-character vocabulary has no Cyrillic | discarded |
| Tesseract | Apache-2.0 | no | - | yes | discarded: native library |
| ocrs | MIT, Apache-2.0 | ONNX-derived RTen models | - | Latin only | discarded |
| TrOCR | MIT code | yes | small: 61M parameters (model card) | printed English lines, needs a detector | discarded |
| Surya | Apache-2.0 / modified AI Pubs OpenRAIL-M, wider commercial use paid | no | - | yes | discarded: weights licence |
| VLMs (Florence-2, GOT-OCR2, PaddleOCR-VL) | various | partly | large (not measured) | varies | discarded: size, CPU time |

### Results

Threshold 0.5 unless noted. CER in %, normalised unless marked raw.

| Detector + recogniser | CER all raw / norm | en | uk | mixed | real pictures | word recall % | false text (chars / pictures) | median / p95 ms | peak RSS MB |
|---|---|---|---|---|---|---|---|---|---|
| v5 mobile + `cyrillic` v5 | 5.0 / 4.2 | 3.2 | 4.4 | 4.5 | 3.6 | 88.5 | 1 / 1 | 329 / 918 | 1108 |
| same, detector input capped at 2000 px | 4.5 / 4.0 | 3.2 | 4.0 | 4.2 | 3.1 | 89.3 | 1 / 1 | 370 / 1077 (b) | 895 |
| same, cap 2000, look-alike repair, lines without letters or digits dropped | 4.2 / 3.6 | 3.0 | 4.3 | 2.7 | 2.5 | 89.9 | 0 / 0 | as above | as above |
| v5 mobile + `cyrillic` v5, detector input capped at 1280 px | 4.3 / 4.1 | 5.0 | 3.5 | 3.7 | 3.8 | 89.9 | 1 / 1 | 339 / 1298 (b) | 684 |
| v5 mobile + `eslav` v5 | 5.6 / 4.9 | 3.9 | 5.0 | 5.3 | 4.0 | 85.6 | 0 / 0 | 331 / 935 | 1114 |
| v5 mobile + `eslav` + `latin`, higher score per line | 11.9 / 11.6 | 4.3 | 11.1 | 23.8 | 10.7 | 81.8 | 1 / 1 | (a) | 1178 |
| v5 mobile + `eslav` + `en`, higher score per line | 10.0 / 9.6 | 3.6 | 11.4 | 12.5 | 5.8 | 83.4 | 3 / 2 | (a) | 1195 |
| v5 mobile + `cyrillic` + `en`, `en` only for lines read without Cyrillic | 5.1 / 4.4 | 3.5 | 4.6 | 4.5 | 3.5 | 88.2 | 3 / 2 | (a) | 1010 |
| v5 mobile + line classifier + `cyrillic` v5 | 7.9 / 7.2 | 4.3 | 9.5 | 5.1 | 7.1 | 85.9 | 1 / 1 | (a) | 947 |
| v5 mobile + `latin` v5 only | 57.5 / 57.9 | 4.3 | 85.7 | 69.9 | 65.9 | 44.2 | 1 / 1 | (a) | 1095 |
| v5 server + `cyrillic` v5 | 5.6 / 4.8 | 5.1 | 4.6 | 4.0 | 4.0 | 88.6 | 2 / 1 | 5749 / 14861 (a) | 2440 |
| v4 mobile + `cyrillic` v3 | 44.6 / 40.7 | 13.8 | 50.9 | 64.0 | 49.7 | 29.8 | 11 / 6 | (a) | 1124 |
| EasyOCR CRAFT + `cyrillic_g2`, threshold 0.3 | 28.8 / 27.7 | 26.7 | 19.6 | 57.6 | 26.7 | 61.3 | 0 / 0 | 1349 / 10175 (a) | 4077 |

Rows without a note were timed at load average 4 to 6. (b) load average 10
to 35, so these times are rough. (a) load average 8 to 55, times not
comparable; the server detector and EasyOCR are slow regardless (their
model time alone: 5.7 s and 1.3 s median). Repeated runs of the first two
rows gave medians of 329 to 363 and 370 to 467 ms, p95 of 918 to 1232 and
1077 to 1169 ms, and peak RSS of 1079 to 1108 and 835 to 895 MB: the other
load on the machine moves the times by up to a third, and the cap only
ever removes work.

Findings:

- `cyrillic_PP-OCRv5_mobile_rec` is the best single recogniser. Its
  dictionary (850 characters) has the full Latin alphabet, all Ukrainian
  letters and both `'` and U+2019, so one model reads English, Ukrainian
  and mixed lines. `eslav` is 0.7 points worse and reads Ukrainian `і` as
  `и` 22 times on the Ukrainian and mixed pictures (`cyrillic`: 8).
- A second recogniser does not pay. Taking the higher-scoring reading per
  line hurts, because the Latin models read Cyrillic as Latin with high
  scores. Running `en` only on lines the Cyrillic model read without
  Cyrillic letters does not help either (4.4 % vs 4.2 %).
- The 0/180 degree line classifier makes every group worse.
- The 88 MB server detector is no better than the 4.8 MB mobile one.
- EasyOCR's grayscale recogniser reads most outlined captions as junk,
  though it is good on the Russian pictures (3.5 %).
- RapidOCR scales the detector input until its short side is 736 px. A 72
  x 1080 strip of screenshot text becomes 736 x 11040; such strips were
  the slowest pictures and set the peak memory. Capping the long side at
  2000 px lowered peak RSS and CER.
- Remaining errors: Latin and Cyrillic look-alikes mixed in a line ("GLORY
  ТО UКRАINE" with Cyrillic Т, О, К, А); `і` read as `и` in Impact
  capitals; apostrophes and spaces dropped in Impact ("КОЛИВ ПЯТНИЦЮ" for
  "КОЛИ В П'ЯТНИЦЮ"); curved or stylised lettering; cursive fonts.

Worst pictures of the recommended setup (cap 2000, repair, threshold 0.5):

| Picture | Reference | Read |
|---|---|---|
| marker on a round lid, curved (real) | NO SHEEP OUT | NO |
| logo sticker "Wiki любить Музику" (real) | Wікі / любить / Музику | iki / любить / y3uky |
| Bad Script font on ruled paper | Не забудь купити / хліб і молоко, / а ще яйця та сир | X zadygb / xλiδi МОЛОКО, / awe. яйцята сир |
| Bad Script sticker | Доброго ранку, / сонечко | Доброго ранку |
| 320 px JPEG, quality 30, Impact | Є ПИТАННЯ / ҐРУНТОВНЕ, ЯК / ЗАВЖДИ | СПИТАННЯ / ГРУНТОВНЕ, AK / ЗАВЖДИ |
| hand-painted sign (real) | MAHALO ... cellphone ... wireless ... peace & | MAMALC ... ellphane ... Wineless ... peale |
| Impact meme | МІЙ ҐАЗДІВСЬКИЙ / НАСТРІЙ О СЬОМІЙ РАНКУ | МИЙГАЗДІВСЬКИЙ / НАСТРИЙ О СЬОМИЙ РАНКУ |

### Recommendation

One detector and one recogniser, 12.9 MB together, no classifier:

| File | URL | Bytes | sha256 |
|---|---|---|---|
| `ocr/det.onnx` | https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.8.0/onnx/PP-OCRv5/det/ch_PP-OCRv5_det_mobile.onnx | 4819576 | `4d97c44a20d30a81aad087d6a396b08f786c4635742afc391f6621f5c6ae78ae` |
| `ocr/rec.onnx` | https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.8.0/onnx/PP-OCRv5/rec/cyrillic_PP-OCRv5_rec_mobile.onnx | 8074092 | `90f761b4bfcce0c8c561c0cb5c887b0971d3ec01c32164bdf7374a35b0982711` |

The hashes are those in RapidOCR 3.8.1's `default_models.yaml`. Weights:
PaddleOCR, Apache-2.0 (Hugging Face `PaddlePaddle/PP-OCRv5_mobile_det`,
`PaddlePaddle/cyrillic_PP-OCRv5_mobile_rec`); ONNX conversion: RapidOCR,
Apache-2.0.

- Keep lines with score >= 0.5 that contain a letter or a digit; with
  that, none of the textless pictures produced text.
- Cap the detector input at 2000 px on the long side.
- Repair look-alikes per word: put the letters that look the same in both
  scripts (А В С Е Н І К М О Р Т Х У, а с е і о р х у, Ї ї ё) into the
  script of the word's other letters, else of the line's; leave the word
  when both are undecided.
- Open the OCR sessions without ONNX Runtime's CPU memory arena (RapidOCR
  does the same): the input sizes vary, and the arena keeps the largest
  activations. In the Rust check below, over the 56 pictures with the
  2000 px cap, peak RSS was 861 MB with ort's defaults and 470 MB with the
  arena and the memory pattern off (333 MB with a 1280 px cap); median
  time 331 vs 374 ms at load average 35 to 42, so rough.
- Expected time on 2 threads: 0.33 to 0.47 s per still (median over the
  set), 0.9 to 1.2 s p95. Stickers of 512 to 618 px: 153 to 182 ms median
  in the quieter runs, so an animation read at 3 frames takes about 0.5
  s (3 x 153 to 182 ms, computed, not measured).
- Search may want to treat `і`/`и` and Latin/Cyrillic look-alikes as close
  matches in OCR text; most remaining confusions are of that kind.

### What the Rust side implements

Reference: RapidOCR 3.8.1 `ch_ppocr_det/utils.py` (`DetPreProcess`,
`DBPostProcess`), `ch_ppocr_det/main.py` (`sorted_boxes`),
`utils/process_img.py` (`get_rotate_crop_image`), `ch_ppocr_rec/main.py`
(`resize_norm_img`) and `ch_ppocr_rec/utils.py` (`CTCLabelDecode`).

1. Input: the decoded picture, alpha flattened as above, in BGR order.
   RapidOCR scales pictures over 2000 px down and under 30 px up, and pads
   very wide (width / height > 8) or very low (<= 30 px) ones above and
   below.
2. Detector input: scale up until the short side is 736 px, cap the long
   side at 2000 px, round both sides to multiples of 32,
   `(x / 255 - 0.5) / 0.5`, NCHW float32, input `x`.
3. Detector output: probability map [1, 1, H, W]. Binarise at 0.3,
   dilate with a 2 x 2 kernel, find contours (at most 1000), take each
   contour's minimum-area rectangle, drop it if its short side is under 3
   px, score it by the mean probability inside it and drop it under 0.5,
   expand it by `area * 1.6 / perimeter` (polygon offset, round joins),
   take the minimum-area rectangle again, drop it if its short side is
   under 5 px, scale to the picture, order the corners clockwise from top
   left, clip, drop boxes with a side of 3 px or less.
4. Order: by top edge; boxes within 10 px of the previous one form a row,
   left to right.
5. Crop: perspective-warp each quadrilateral to a rectangle (width and
   height from its longer opposite edges, cubic, replicated border);
   rotate by 90 degrees when height / width >= 1.5.
6. Recogniser input: height 48, width `ceil(48 * w / h)`, batches of 6
   sorted by aspect ratio, zero-padded on the right to the widest (at
   least 320 px), `(x / 255 - 0.5) / 0.5`, NCHW, input `x`.
7. Recogniser output [N, T, 852]: greedy CTC. Arg-max per step, collapse
   repeats, drop index 0 (blank). The dictionary is the model metadata key
   `character` (850 lines) plus a space at the end; index i is entry
   i - 1. The line score is the mean of the kept steps' maximum
   probabilities.
8. Filter, repair and join the lines as above.

**Runs through `ort`.** A scratch program with `ort = "=2.0.0-rc.15"`
(`load-dynamic`, `std`) and `image` 0.25, built in this repo's dev shell,
loaded nixpkgs `onnxruntime` 1.24.4 with `ort::init_from`, opened both
models with `Session::builder(&env)` (level 3, intra 2, inter 1), read the
dictionary from the model metadata and printed, for the rendered Ukrainian
chat: `Мама`, `в мережі`, `Ти поїла? Не забудь шапку, бо`, `холодно`,
`09:14`, `Так, мамо, все гаразд`, `09:20`, `Привези ввечері ґудзики й
нитки`, `09:21`, `Добре, з'їжджу після роботи`, `09:25` (scores 0.82 to
1.00). Its post-processing was simplified (connected components instead of
contours and polygon offsets), so it proves loading and running, not
accuracy.

## Draft: recheck

**What.** A user rechecks their own items, an admin the whole server,
optionally one set. Parts (`RecheckPart`, snake_case): `keywords`,
`emojis`, `vibe`, `text`, `embedding`, `quality`, `renditions`
(plus `name` if the server derives one), each chosen on its own. Modes:
`missing` (default) computes only what is not there (no embedding from
the current model, no OCR result from the current model, an empty field);
`refresh` recomputes and replaces AI values. Neither changes a value a
person gave (the rules above). A user recheck changes only that user's
items; work on shared media (embedding, OCR, quality, renditions) is
harmless to the other owners.

**The batch.** Table `rechecks` (migration `0015`): id, scope (`user` or
`server`), requested_by, owner_id (user scope), set_id, parts (JSON),
mode, state (`running`, `done`, `cancelled`, `failed`), total, done,
failed, changed, created_at, finished_at; failures in
`recheck_failures` (recheck_id, item_id or media_id, error). The batch
fans out to one `JobKind::Recheck` job per media in scope (payload
`{recheck_id, media_id}`), which updates the counters when it ends. The
same migration adds `jobs.priority INTEGER NOT NULL DEFAULT 0`; the queue
claims by `priority DESC, run_after, created_at`, and recheck jobs have a
negative priority, so uploads (`Variants`, `Enrich`) made during a
server-wide recheck go first. Jobs persist, so a restart continues
(`requeue_interrupted`). Cancel marks the batch `cancelled` and deletes
its queued jobs. Starting the same scope (owner, set, parts, mode) while
it runs returns the running batch. A server-wide start is audited; user
starts are rate limited. The startup re-enrichment of media whose
embedding is from another model (`jobs::media::enqueue_missing_enrichment`)
becomes a server-scope recheck started by the server itself, so there is
one mechanism.
