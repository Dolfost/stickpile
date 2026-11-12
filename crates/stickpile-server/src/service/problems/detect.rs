//! Problem detection over facts loaded by the parent module: pure functions,
//! unit-tested with synthetic facts and with real images through the media
//! pipeline. See docs/web.md ("Problems") for what each kind means to users.
//!
//! Pairs (near-duplicates, look-alikes) are found in two steps so a set of a
//! few hundred items stays cheap: [`candidates`] compares every pair's
//! stored hashes and embeddings (O(n²) popcounts and dot products: 20,000
//! pairs for 200 items), then [`detect`] confirms the near-duplicate candidates
//! with the thumbnails' pixel signatures (see [`stickpile_media::signature`]).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use stickpile_api::profiles::profile;
use stickpile_api::{
    JobStatus, MediaKind, Problem, ProblemAction, ProblemItem, ProblemKind, ProblemSeverity, TargetApp, Variant,
};
use stickpile_media::hash::hamming;
use stickpile_media::signature::Signature;
use stickpile_telegram::{PackMapping, DEFAULT_EMOJI, MAX_PACK_STICKERS};

use crate::ids::sha256_hex;

/// pHash distance up to which two items are near-duplicate candidates.
/// Generous on purpose (6 % crops reach 20); the pixel check decides.
pub const NEAR_PHASH: u32 = 20;
/// dHash distance up to which two items are near-duplicate candidates.
pub const NEAR_DHASH: u32 = 12;
/// Worst-block luma difference (0..=255) of the thumbnails up to which a
/// candidate is a near-duplicate. Calibrated on synthetic stickers (flat
/// faces, text) and a photo-like picture: resized, recompressed (JPEG
/// 50-70), blurred and 3-10 % cropped copies scored 0.8-10.5; the same face
/// with closed eyes 27-33, another mouth, a hat or a caption 36-128, other
/// faces 36+. The same text in another colour scores 9: [`NEAR_COLOUR`]
/// tells those apart.
pub const NEAR_PIXELS: f32 = 16.0;
/// Largest colour difference (|ΔCb| + |ΔCr| of a 4×4 cell, 0..=510) of a
/// near-duplicate. Copies scored 0-10 in calibration, the same drawing in
/// another colour 14-170.
pub const NEAR_COLOUR: u32 = 12;
/// CLIP cosine similarity from which two items "look alike" (and become
/// near-duplicate candidates). Conservative: in calibration, copies scored
/// 0.94-1.0, the same character with another expression 0.87-0.98, two
/// different characters in the same style 0.85-0.92.
pub const SIMILAR_COSINE: f32 = 0.95;
/// At most this many look-alike pairs per set (the most similar first): in a
/// pack of one character drawn the same way, many pairs pass the threshold.
pub const MAX_SIMILAR_PAIRS: usize = 30;
/// Items whose longest side is below this are "very small".
pub const LOW_RES_PX: u32 = 128;

#[derive(Debug, Clone)]
pub struct JobFact {
    pub id: String,
    pub status: JobStatus,
    pub error: Option<String>,
    pub finished_at: Option<i64>,
}

/// What detection needs to know about one item.
#[derive(Debug, Clone)]
pub struct ItemFacts {
    pub id: String,
    pub name: String,
    pub position: i64,
    pub created_at: i64,
    pub media_id: String,
    pub sha256: String,
    pub original_sha256: String,
    pub kind: MediaKind,
    pub width: u32,
    pub height: u32,
    pub duration_ms: Option<u64>,
    pub quality: Option<u8>,
    pub phash: u64,
    pub dhash: u64,
    pub emojis: Vec<String>,
    /// Fields holding AI values nobody confirmed (`name`, `keywords`, ...).
    pub ai_fields: Vec<&'static str>,
    /// Variants that exist.
    pub variants: Vec<Variant>,
    pub variants_done: bool,
    /// The newest `Variants` job of the media, when the variants are not done.
    pub variants_job: Option<JobFact>,
    /// CLIP embedding of the current model, when AI is loaded.
    pub embedding: Option<Arc<[f32]>>,
    /// (id, name) of the owner's other sets that got the same file earlier.
    pub earlier_in: Vec<(String, String)>,
}

/// What detection needs to know about one set.
#[derive(Debug, Clone, Default)]
pub struct SetFacts {
    pub id: String,
    pub version: i64,
    pub updated_at: i64,
    /// `git_last_error` of a git pack.
    pub git_error: Option<String>,
    pub telegram_pack: Option<String>,
    pub telegram_mapping: Option<PackMapping>,
    /// The newest `TelegramExport` job of the set.
    pub last_export: Option<JobFact>,
    /// When the newest successful export finished.
    pub last_export_done_at: Option<i64>,
    /// In set order.
    pub items: Vec<ItemFacts>,
}

/// Server facts that change what is a problem.
#[derive(Debug, Clone, Copy)]
pub struct Env {
    /// The Telegram integration is configured.
    pub telegram: bool,
    /// ffmpeg is available (animated stickers need it).
    pub ffmpeg: bool,
}

/// A pair worth a closer look.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub a: usize,
    pub b: usize,
    pub phash: u32,
    pub dhash: u32,
    pub cosine: Option<f32>,
    /// Could be a near-duplicate (same kind and shape); needs the pixel check.
    pub near: bool,
}

fn moving(k: MediaKind) -> bool {
    k != MediaKind::Still
}

/// Same kind of media and about the same shape (and length): only such
/// pairs can be copies of each other.
fn comparable(a: &ItemFacts, b: &ItemFacts) -> bool {
    if moving(a.kind) != moving(b.kind) {
        return false;
    }
    let ratio = |w: u32, h: u32| f64::from(w.max(1)) / f64::from(h.max(1));
    let (ra, rb) = (ratio(a.width, a.height), ratio(b.width, b.height));
    if ra.max(rb) / ra.min(rb) > 1.25 {
        return false;
    }
    match (a.duration_ms, b.duration_ms) {
        (Some(x), Some(y)) if moving(a.kind) => x.abs_diff(y) * 100 <= x.max(y) * 15,
        _ => true,
    }
}

fn cosine(a: &ItemFacts, b: &ItemFacts) -> Option<f32> {
    match (&a.embedding, &b.embedding) {
        (Some(x), Some(y)) if x.len() == y.len() && !x.is_empty() => Some(stickpile_ai::cosine(x, y)),
        _ => None,
    }
}

/// Every pair of items that could be a near-duplicate or look alike. Items
/// with the same stored file are exact duplicates instead.
pub fn candidates(items: &[ItemFacts]) -> Vec<Candidate> {
    let mut out = Vec::new();
    for (i, a) in items.iter().enumerate() {
        for (j, b) in items.iter().enumerate().skip(i + 1) {
            if a.original_sha256 == b.original_sha256 {
                continue;
            }
            let (p, d) = (hamming(a.phash, b.phash), hamming(a.dhash, b.dhash));
            let cos = cosine(a, b);
            let alike = cos.is_some_and(|c| c >= SIMILAR_COSINE);
            let near = comparable(a, b) && (p <= NEAR_PHASH || d <= NEAR_DHASH || alike);
            if near || alike {
                out.push(Candidate {
                    a: i,
                    b: j,
                    phash: p,
                    dhash: d,
                    cosine: cos,
                    near,
                });
            }
        }
    }
    out
}

/// Media ids whose signatures [`detect`] needs.
pub fn signatures_needed<'a>(items: &'a [ItemFacts], cands: &[Candidate]) -> Vec<&'a ItemFacts> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for c in cands.iter().filter(|c| c.near) {
        for i in [c.a, c.b] {
            if seen.insert(items[i].media_id.as_str()) {
                out.push(&items[i]);
            }
        }
    }
    out
}

/// `<kind>-<16 hex>` from what makes the problem a problem.
pub fn problem_id(kind: ProblemKind, fingerprint: &str) -> String {
    let h = sha256_hex(format!("{}\n{fingerprint}", kind.as_str()).as_bytes());
    format!("{}-{}", kind.as_str(), &h[..16])
}

/// A valid problem id (as made by [`problem_id`]).
pub fn valid_problem_id(id: &str) -> bool {
    match id.rsplit_once('-') {
        Some((kind, hash)) => {
            ProblemKind::parse(kind).is_some()
                && hash.len() == 16
                && hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }
        None => false,
    }
}

fn item(it: &ItemFacts, detail: Option<String>) -> ProblemItem {
    ProblemItem {
        item_id: it.id.clone(),
        name: it.name.clone(),
        kind: it.kind,
        width: it.width,
        height: it.height,
        detail,
        hidden: false,
    }
}

/// "512×512 px, quality 85": helps choosing which copy to keep.
fn size_detail(it: &ItemFacts) -> String {
    match it.quality {
        Some(q) => format!("{}×{} px, quality {q}", it.width, it.height),
        None => format!("{}×{} px", it.width, it.height),
    }
}

fn seconds(ms: u64) -> String {
    let s = ms as f64 / 1000.0;
    if s.fract() < 0.05 {
        format!("{s:.0} s")
    } else {
        format!("{s:.1} s")
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

struct Builder<'a> {
    set_id: &'a str,
    out: Vec<Problem>,
}

impl Builder<'_> {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        kind: ProblemKind,
        severity: ProblemSeverity,
        message: String,
        items: Vec<ProblemItem>,
        action: Option<ProblemAction>,
        score: Option<f32>,
        fingerprint: &str,
    ) {
        self.out.push(Problem {
            id: problem_id(kind, fingerprint),
            kind,
            severity,
            message,
            set_id: self.set_id.to_string(),
            items,
            action,
            score,
            dismissed: false,
        });
    }

    /// One problem for every item `f` reports on, with a per-item detail.
    #[allow(clippy::too_many_arguments)]
    fn per_item(
        &mut self,
        kind: ProblemKind,
        severity: ProblemSeverity,
        items: &[ItemFacts],
        action: ProblemAction,
        message: impl Fn(usize) -> String,
        f: impl Fn(&ItemFacts) -> Option<(Option<String>, String)>,
    ) {
        let mut hits = Vec::new();
        let mut fp = String::new();
        for it in items {
            if let Some((detail, key)) = f(it) {
                fp.push_str(&format!("{}:{}:{key}\n", it.id, it.sha256));
                hits.push(item(it, detail));
            }
        }
        if !hits.is_empty() {
            let msg = message(hits.len());
            self.push(kind, severity, msg, hits, Some(action), None, &fp);
        }
    }
}

/// Why the Telegram export skips this item, if it does.
fn telegram_skip_reason(it: &ItemFacts, env: &Env) -> Option<String> {
    match it.kind {
        MediaKind::Video if it.duration_ms.is_some_and(|d| d > stickpile_media::SHORT_VIDEO_MS) => Some(format!(
            "videos longer than {} cannot become stickers",
            seconds(stickpile_media::SHORT_VIDEO_MS)
        )),
        MediaKind::Animation | MediaKind::Video if !env.ffmpeg => {
            Some("animated stickers need ffmpeg on the server".into())
        }
        MediaKind::Still if it.variants_done && !it.variants.contains(&Variant::TgSticker) => {
            Some("no Telegram sticker could be made from it".into())
        }
        MediaKind::Animation | MediaKind::Video
            if it.variants_done && !it.variants.contains(&Variant::TgVideoSticker) =>
        {
            Some("cannot be made into a Telegram video sticker (at most 3 s, 512 px, 256 KB)".into())
        }
        _ => None,
    }
}

/// Every problem of one set, worst first. `sigs` holds the signatures of
/// the media in [`signatures_needed`] that could be computed.
pub fn detect(f: &SetFacts, cands: &[Candidate], sigs: &HashMap<String, Arc<Signature>>, env: &Env) -> Vec<Problem> {
    let mut b = Builder {
        set_id: &f.id,
        out: Vec::new(),
    };
    let items = &f.items;
    let bound = env.telegram && f.telegram_pack.is_some();

    if items.is_empty() {
        b.push(
            ProblemKind::EmptySet,
            ProblemSeverity::Info,
            "The set has no items yet.".into(),
            Vec::new(),
            Some(ProblemAction::AddItems),
            None,
            "",
        );
    }

    // --- duplicates -------------------------------------------------------
    let mut by_file: BTreeMap<&str, Vec<&ItemFacts>> = BTreeMap::new();
    for it in items {
        by_file.entry(&it.original_sha256).or_default().push(it);
    }
    for group in by_file.values().filter(|g| g.len() > 1) {
        let fp: String = group.iter().map(|it| format!("{}:{}\n", it.id, it.sha256)).collect();
        b.push(
            ProblemKind::ExactDuplicate,
            ProblemSeverity::Warning,
            format!("{} items store the same file.", group.len()),
            group.iter().map(|it| item(it, Some(size_detail(it)))).collect(),
            Some(ProblemAction::DeleteOne),
            None,
            &fp,
        );
    }
    let pair_fp = |x: &ItemFacts, y: &ItemFacts| format!("{}:{}\n{}:{}\n", x.id, x.sha256, y.id, y.sha256);
    let mut similar = Vec::new();
    for c in cands {
        let (x, y) = (&items[c.a], &items[c.b]);
        let pixels = if c.near {
            match (sigs.get(&x.media_id), sigs.get(&y.media_id)) {
                (Some(sx), Some(sy)) => sx.distance(sy, NEAR_PIXELS, NEAR_COLOUR),
                _ => None,
            }
        } else {
            None
        };
        if let Some(d) = pixels {
            let message = if d < 2.0 && c.phash == 0 && c.dhash == 0 {
                "These items look identical (the files differ)."
            } else {
                "These items show the same picture (resized, recompressed or slightly cropped)."
            };
            b.push(
                ProblemKind::NearDuplicate,
                ProblemSeverity::Warning,
                message.into(),
                vec![item(x, Some(size_detail(x))), item(y, Some(size_detail(y)))],
                Some(ProblemAction::DeleteOne),
                Some(c.phash as f32),
                &pair_fp(x, y),
            );
        } else if let Some(cos) = c.cosine.filter(|c| *c >= SIMILAR_COSINE) {
            similar.push((cos, x, y));
        }
    }
    similar.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (cos, x, y) in similar.into_iter().take(MAX_SIMILAR_PAIRS) {
        b.push(
            ProblemKind::Similar,
            ProblemSeverity::Info,
            "These items look alike to the AI. Only a hint: variations of one character often do.".into(),
            vec![item(x, None), item(y, None)],
            Some(ProblemAction::Compare),
            Some((cos * 1000.0).round() / 1000.0),
            &pair_fp(x, y),
        );
    }
    b.per_item(
        ProblemKind::InOtherSet,
        ProblemSeverity::Info,
        items,
        ProblemAction::OpenItem,
        |n| {
            format!(
                "{} also in other sets of yours (added there first).",
                count(n, "item is", "items are")
            )
        },
        |it| {
            (!it.earlier_in.is_empty()).then(|| {
                let names: Vec<&str> = it.earlier_in.iter().map(|(_, n)| n.as_str()).collect();
                let ids: Vec<&str> = it.earlier_in.iter().map(|(i, _)| i.as_str()).collect();
                (Some(format!("also in {}", names.join(", "))), ids.join(","))
            })
        },
    );

    // --- media ------------------------------------------------------------
    b.per_item(
        ProblemKind::MissingFiles,
        ProblemSeverity::Error,
        items,
        ProblemAction::OpenItem,
        |n| {
            format!(
                "{} missing files; delete and upload them again.",
                count(n, "item is", "items are")
            )
        },
        |it| {
            let missing: Vec<&str> = [(Variant::Original, "original"), (Variant::Thumb, "thumbnail")]
                .into_iter()
                .filter(|(v, _)| !it.variants.contains(v))
                .map(|(_, n)| n)
                .collect();
            (!missing.is_empty()).then(|| (Some(format!("no {}", missing.join(", no "))), missing.join(",")))
        },
    );
    b.per_item(
        ProblemKind::ProcessingFailed,
        ProblemSeverity::Warning,
        items,
        ProblemAction::OpenItem,
        |n| {
            format!(
                "Processing of {} did not finish: some downloads and conversions are missing.",
                count(n, "item", "items")
            )
        },
        |it| {
            if it.variants_done {
                return None;
            }
            match &it.variants_job {
                // Still in the queue: not a problem (yet).
                Some(j) if matches!(j.status, JobStatus::Queued | JobStatus::Running) => None,
                Some(j) => Some((
                    Some(j.error.clone().unwrap_or_else(|| "processing failed".into())),
                    j.id.clone(),
                )),
                None => Some((Some("never processed".into()), String::new())),
            }
        },
    );
    b.per_item(
        ProblemKind::LowResolution,
        if bound {
            ProblemSeverity::Warning
        } else {
            ProblemSeverity::Info
        },
        items,
        ProblemAction::FindBetter,
        |n| {
            let tg = if bound {
                " Telegram scales stickers up to 512 px."
            } else {
                ""
            };
            format!(
                "{} very small (under {LOW_RES_PX} px) and may look blurry.{tg}",
                count(n, "item is", "items are")
            )
        },
        |it| {
            (it.width.max(it.height) < LOW_RES_PX)
                .then(|| (Some(format!("{}×{} px", it.width, it.height)), String::new()))
        },
    );

    // --- metadata ---------------------------------------------------------
    b.per_item(
        ProblemKind::NotEmoji,
        if bound {
            ProblemSeverity::Warning
        } else {
            ProblemSeverity::Info
        },
        items,
        ProblemAction::EditEmojis,
        |n| {
            format!(
                "{} emoji entries that are not emoji (Telegram leaves them out).",
                count(n, "item has", "items have")
            )
        },
        |it| {
            let bad: Vec<&str> = it
                .emojis
                .iter()
                .map(|e| e.trim())
                .filter(|e| !e.is_empty() && !stickpile_telegram::is_emoji(e))
                .collect();
            (!bad.is_empty()).then(|| (Some(bad.join(" ")), bad.join("\u{1f}")))
        },
    );
    if bound {
        b.per_item(
            ProblemKind::NoEmoji,
            ProblemSeverity::Warning,
            items,
            ProblemAction::EditEmojis,
            |n| {
                format!(
                    "{} no emoji; on Telegram they get {DEFAULT_EMOJI}, which makes them hard to find.",
                    count(n, "item has", "items have")
                )
            },
            |it| {
                let any = it.emojis.iter().any(|e| stickpile_telegram::is_emoji(e.trim()));
                (!any).then(|| (None, it.emojis.join("\u{1f}")))
            },
        );
    }
    b.per_item(
        ProblemKind::AiUnreviewed,
        ProblemSeverity::Info,
        items,
        ProblemAction::ReviewAi,
        |n| {
            format!(
                "{} AI suggestions nobody confirmed yet.",
                count(n, "item has", "items have")
            )
        },
        |it| (!it.ai_fields.is_empty()).then(|| (Some(it.ai_fields.join(", ")), it.ai_fields.join(","))),
    );

    // --- Telegram and git -------------------------------------------------
    if env.telegram {
        if let Some(job) = f.last_export.as_ref().filter(|j| j.status == JobStatus::Failed) {
            b.push(
                ProblemKind::TelegramFailed,
                ProblemSeverity::Error,
                format!(
                    "The last publish to Telegram failed: {}",
                    job.error.as_deref().unwrap_or("unknown error")
                ),
                Vec::new(),
                Some(ProblemAction::PublishTelegram),
                None,
                &job.id,
            );
        }
    }
    if bound {
        let spec = profile(TargetApp::Telegram).sticker.expect("telegram has stickers");
        let skipped: HashMap<&str, String> = items
            .iter()
            .filter_map(|it| telegram_skip_reason(it, env).map(|r| (it.id.as_str(), r)))
            .collect();
        let exportable = items.len() - skipped.len();
        if exportable > MAX_PACK_STICKERS {
            b.push(
                ProblemKind::TelegramTooMany,
                ProblemSeverity::Error,
                format!(
                    "The set has {exportable} stickers; a Telegram pack holds at most {MAX_PACK_STICKERS}, so publishing \
                     fails. Move some items to another set."
                ),
                Vec::new(),
                Some(ProblemAction::SplitSet),
                None,
                &exportable.to_string(),
            );
        }
        b.per_item(
            ProblemKind::TelegramUnsupported,
            ProblemSeverity::Warning,
            items,
            ProblemAction::OpenItem,
            |n| {
                format!(
                    "{} cannot become Telegram stickers and {} left out when the set is published.",
                    count(n, "item", "items"),
                    if n == 1 { "is" } else { "are" }
                )
            },
            |it| skipped.get(it.id.as_str()).map(|r| (Some(r.clone()), r.clone())),
        );
        b.per_item(
            ProblemKind::TelegramTrimmed,
            ProblemSeverity::Info,
            items,
            ProblemAction::OpenItem,
            |n| {
                format!(
                    "{} longer than {}: Telegram gets only the first {}.",
                    count(n, "animation or video is", "animations or videos are"),
                    seconds(spec.video_max_ms),
                    seconds(spec.video_max_ms)
                )
            },
            |it| {
                let long = moving(it.kind)
                    && it.duration_ms.is_some_and(|d| d > spec.video_max_ms)
                    && !skipped.contains_key(it.id.as_str());
                long.then(|| (it.duration_ms.map(seconds), String::new()))
            },
        );
        // What the stored mapping (item -> sticker) says is not on Telegram.
        if let Some(mapping) = &f.telegram_mapping {
            let mut missing = 0;
            let mut replaced = 0;
            for it in items.iter().filter(|it| !skipped.contains_key(it.id.as_str())) {
                match mapping.get(&it.id) {
                    None => missing += 1,
                    Some((content, _)) if *content != it.sha256 => replaced += 1,
                    Some(_) => {}
                }
            }
            let removed = mapping.keys().filter(|k| !items.iter().any(|it| &it.id == *k)).count();
            // Metadata and order changes are not in the mapping: compare with
            // the last successful export (jobs are kept for 30 days).
            let later = f.last_export_done_at.is_some_and(|done| f.updated_at > done);
            if missing + replaced + removed > 0 || later {
                let mut parts = Vec::new();
                if missing > 0 {
                    parts.push(format!("{} not on Telegram yet", count(missing, "item", "items")));
                }
                if replaced > 0 {
                    parts.push(format!("{} with a new file", count(replaced, "item", "items")));
                }
                if removed > 0 {
                    parts.push(format!(
                        "{} removed here but still in the pack",
                        count(removed, "sticker", "stickers")
                    ));
                }
                let message = if parts.is_empty() {
                    "The set changed after its last publish to Telegram; publish again to update the pack.".to_string()
                } else {
                    format!(
                        "The Telegram pack is out of date: {}. Publish again to update it.",
                        parts.join(", ")
                    )
                };
                b.push(
                    ProblemKind::TelegramOutdated,
                    if parts.is_empty() {
                        ProblemSeverity::Info
                    } else {
                        ProblemSeverity::Warning
                    },
                    message,
                    Vec::new(),
                    Some(ProblemAction::PublishTelegram),
                    None,
                    &format!("{}:{missing}:{replaced}:{removed}", f.version),
                );
            }
        }
    }
    if let Some(err) = &f.git_error {
        b.push(
            ProblemKind::GitFailed,
            ProblemSeverity::Error,
            format!("The last git pull or push failed: {err}"),
            Vec::new(),
            Some(ProblemAction::PullGit),
            None,
            err,
        );
    }

    let position: HashMap<&str, i64> = items.iter().map(|it| (it.id.as_str(), it.position)).collect();
    let mut out = b.out;
    out.sort_by_key(|p| {
        (
            p.severity,
            p.kind,
            p.items
                .first()
                .and_then(|i| position.get(i.item_id.as_str()).copied())
                .unwrap_or(-1),
        )
    });
    out
}
