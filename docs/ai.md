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

What it does **not** do: captions, OCR of text in memes, face recognition,
NSFW classification, or any training or fine-tuning on your library. A
caption backend can be added later behind the same trait (see the last
section).

## How results are marked AI-generated

`stickpile-ai` only returns values. It never writes them anywhere. Under the
plan's data model, the server stores them with `keywords_source`,
`emojis_source` and `vibe_source` set to `ai`. They become `user` when
someone edits or confirms them in the web UI, and `set.toml` export carries
the same `*_source = "ai"` fields. Re-enrichment must only replace fields
whose source is still `ai`.

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
image in, `{keywords, emojis, vibe}` out, nothing stored. It runs the same
code as the `Enrich` job (`service::ai::embed_and_suggest`: embed the image,
then `ZeroShot::suggest` with `max_keywords`, `max_emojis` and the default
thresholds), on the blocking thread pool, rate limited per user
(`limits.ai_suggest_per_minute`), and answers 503 when AI is off (with
`Retry-After` while the model loads).

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

## Tests

- `cargo test -p stickpile-ai` needs neither network nor model files. It
  covers preprocessing (shape, normalisation, alpha flattening, crop,
  extreme aspect ratios), tokenizer padding and truncation with a synthetic
  tokenizer, mean pooling, the dense layer from safetensors, blob round-trip,
  the model file table against the script and the Nix file, vocabulary
  loading and validation, `vibes_for_word`, `ZeroShot` with `MockEnricher`,
  and a broken ONNX Runtime library giving an error rather than a panic.
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
