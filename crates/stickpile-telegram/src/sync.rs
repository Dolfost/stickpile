//! Pack-level import and export.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::api::{BotApi, DownloadError, InputSticker, Sticker, StickerFormat, TelegramError};

/// Telegram's limit for regular sticker packs.
pub const MAX_PACK_STICKERS: usize = 120;
/// `createNewStickerSet` accepts at most this many initial stickers.
pub const MAX_CREATE_BATCH: usize = 50;
/// Used when an item has no emoji (Telegram requires at least one).
pub const DEFAULT_EMOJI: &str = "🙂";
/// Reason given for skipped animated stickers.
pub const TGS_SKIP_REASON: &str = "animated .tgs stickers are not supported (stretch goal)";
const MAX_EMOJIS: usize = 20;
const MAX_KEYWORDS: usize = 20;
const MAX_KEYWORD_CHARS: usize = 64;
const MAX_TITLE_CHARS: usize = 64;

#[derive(Debug, Clone)]
pub struct ImportedSticker {
    pub file_unique_id: String,
    pub bytes: Vec<u8>,
    pub format: StickerFormat,
    pub emojis: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ImportedPack {
    pub name: String,
    pub title: String,
    /// In pack order.
    pub stickers: Vec<ImportedSticker>,
    /// Stickers that could not be imported (e.g. `.tgs`), with reasons.
    /// The first element is the sticker's `file_unique_id`.
    pub skipped: Vec<(String, String)>,
}

/// Download every sticker of pack `name` in pack order.
///
/// Static stickers arrive as WebP, video stickers as WebM; animated `.tgs`
/// stickers and files over `max_file_bytes` are listed in `skipped`. The Bot
/// API exposes one emoji per sticker (no keywords), so `emojis` has at most
/// one entry.
pub async fn import_pack(api: &BotApi, name: &str, max_file_bytes: u64) -> Result<ImportedPack, TelegramError> {
    let set = api.get_sticker_set(name).await?;
    let mut stickers = Vec::with_capacity(set.stickers.len());
    let mut skipped = Vec::new();
    for s in &set.stickers {
        if s.is_animated {
            skipped.push((s.file_unique_id.clone(), TGS_SKIP_REASON.to_string()));
            continue;
        }
        let format = if s.is_video {
            StickerFormat::Video
        } else {
            StickerFormat::Static
        };
        match api.download_capped(&s.file_id, max_file_bytes).await {
            Ok(bytes) => stickers.push(ImportedSticker {
                file_unique_id: s.file_unique_id.clone(),
                bytes,
                format,
                emojis: s.emoji.iter().filter(|e| !e.is_empty()).cloned().collect(),
            }),
            Err(DownloadError::TooLarge) => skipped.push((
                s.file_unique_id.clone(),
                format!("file is larger than {max_file_bytes} bytes"),
            )),
            Err(DownloadError::Other(e)) => return Err(e),
        }
    }
    Ok(ImportedPack {
        name: set.name,
        title: set.title,
        stickers,
        skipped,
    })
}

/// One set item, already rendered as a Telegram sticker (512 px WebP or VP9 WebM).
#[derive(Debug, Clone)]
pub struct ExportItem {
    /// Stable stickpile key (item id) used for the mapping.
    pub key: String,
    /// Changes when the media changes (e.g. media sha256): re-upload if different.
    pub content_key: String,
    pub bytes: Vec<u8>,
    pub format: StickerFormat,
    pub emojis: Vec<String>,
    pub keywords: Vec<String>,
}

/// item key -> (content_key, Telegram file_unique_id). Persisted by the server
/// between exports.
pub type PackMapping = BTreeMap<String, (String, String)>;

/// What stickpile left on a pack after an export: persisted by the server
/// and passed to the next export of the same pack.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PackState {
    pub mapping: PackMapping,
    /// Key of the thumbnail stickpile set on the pack (None: none).
    pub thumbnail: Option<String>,
}

/// The pack's thumbnail (the set's cover), already rendered.
#[derive(Debug, Clone)]
pub struct PackThumbnail {
    /// Changes when the picture changes (e.g. cover item id + media sha256);
    /// the thumbnail is uploaded only when it differs from the last export's.
    pub key: String,
    /// Static PNG, exactly 100×100, at most 128 KB.
    pub png: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExportReport {
    pub created_pack: bool,
    pub added: u32,
    pub removed: u32,
    pub updated: u32,
    pub reordered: bool,
    /// New mapping to persist.
    pub mapping: PackMapping,
    /// Key of the thumbnail stickpile has set on the pack after this export
    /// (`None`: none, Telegram shows the first sticker). Persist it and pass
    /// it back as `previous_thumbnail`.
    pub thumbnail: Option<String>,
    /// The thumbnail was set or dropped by this export.
    pub thumbnail_changed: bool,
}

impl ExportReport {
    /// What to persist and pass to the next export.
    pub fn state(&self) -> PackState {
        PackState {
            mapping: self.mapping.clone(),
            thumbnail: self.thumbnail.clone(),
        }
    }
}

/// Make pack `name` (owned by `owner_user_id`) match `items` in order.
///
/// * Pack missing (`getStickerSet` answers `STICKERSET_INVALID`): created with
///   the first 50 items, the rest added one by one.
/// * Pack present: stickers whose item is gone, whose `content_key` changed or
///   that are unknown to `previous` are deleted; missing items are added (new
///   stickers are appended, their `file_unique_id`s read back with
///   `getStickerSet`); emoji list and keywords of every kept sticker are set
///   (the Bot API does not expose keywords, so changes cannot be detected);
///   positions are fixed to match `items`; the title is updated if different.
///   The pack is never emptied in between: with nothing to keep (e.g. a lost
///   mapping), the last old sticker is swapped via `replaceStickerInSet`.
///
/// The thumbnail: `thumbnail` is set when its key differs from
/// `previous.thumbnail` (what the last export set); with no `thumbnail` but a
/// previous one, it is dropped so the first sticker shows again. A thumbnail
/// stickpile did not set (no `previous.thumbnail`) is left alone.
///
/// Items are sanitised for the Bot API: entries that are not emoji (words
/// typed into the field) are left out, no emoji -> [`DEFAULT_EMOJI`], at most
/// 20 emoji, at most 20 keywords with at most 64 characters in total, title
/// cut to 64 characters. An item whose `content_key` repeats an earlier item
/// is not exported (Telegram would ignore the identical file) and gets no
/// mapping entry. More than [`MAX_PACK_STICKERS`] items or duplicate keys are
/// an error before any call is made.
pub async fn export_pack(
    api: &BotApi,
    owner_user_id: i64,
    name: &str,
    title: &str,
    items: &[ExportItem],
    previous: &PackState,
    thumbnail: Option<&PackThumbnail>,
) -> Result<ExportReport, TelegramError> {
    let mut keys = HashSet::new();
    for it in items {
        if !keys.insert(it.key.as_str()) {
            return Err(TelegramError::Invalid(format!("duplicate item key {:?}", it.key)));
        }
    }
    let mut contents = HashSet::new();
    let items: Vec<&ExportItem> = items
        .iter()
        .filter(|it| contents.insert(it.content_key.as_str()))
        .collect();
    if items.is_empty() {
        return Err(TelegramError::Invalid("nothing to export: the set has no items".into()));
    }
    if items.len() > MAX_PACK_STICKERS {
        return Err(TelegramError::Invalid(format!(
            "Telegram packs hold at most {MAX_PACK_STICKERS} stickers; the set has {}",
            items.len()
        )));
    }
    let title = clamp_title(title, name);
    let mut report = ExportReport::default();

    let current = match api.get_sticker_set(name).await {
        Ok(set) => set,
        Err(e) if e.is_missing_sticker_set() => {
            let first: Vec<InputSticker> = items
                .iter()
                .take(MAX_CREATE_BATCH)
                .map(|it| input_sticker(it))
                .collect();
            api.create_new_sticker_set(owner_user_id, name, &title, &first).await?;
            for it in items.iter().skip(MAX_CREATE_BATCH) {
                api.add_sticker_to_set(owner_user_id, name, &input_sticker(it)).await?;
            }
            let set = api.get_sticker_set(name).await?;
            if set.stickers.len() != items.len() {
                return Err(TelegramError::Invalid(format!(
                    "pack {name} has {} stickers after creation, expected {}",
                    set.stickers.len(),
                    items.len()
                )));
            }
            report.created_pack = true;
            report.added = items.len() as u32;
            report.mapping = items
                .iter()
                .zip(&set.stickers)
                .map(|(it, s)| (it.key.clone(), (it.content_key.clone(), s.file_unique_id.clone())))
                .collect();
            sync_thumbnail(api, owner_user_id, name, thumbnail, None, &mut report).await?;
            return Ok(report);
        }
        Err(e) => return Err(e),
    };

    // Which existing stickers can stay?
    let present: HashSet<&str> = current.stickers.iter().map(|s| s.file_unique_id.as_str()).collect();
    let mut kept: HashMap<&str, String> = HashMap::new(); // item key -> file_unique_id
    let mut kept_fuids: HashSet<String> = HashSet::new();
    for it in &items {
        if let Some((content_key, fuid)) = previous.mapping.get(&it.key) {
            if *content_key == it.content_key && present.contains(fuid.as_str()) && kept_fuids.insert(fuid.clone()) {
                kept.insert(it.key.as_str(), fuid.clone());
            }
        }
    }
    let mut to_delete: Vec<&Sticker> = current
        .stickers
        .iter()
        .filter(|s| !kept_fuids.contains(&s.file_unique_id))
        .collect();
    let to_add: Vec<&ExportItem> = items
        .iter()
        .filter(|it| !kept.contains_key(it.key.as_str()))
        .copied()
        .collect();
    // With nothing to keep, the last old sticker is replaced in place instead
    // of deleted: the pack never becomes empty, and an item identical to that
    // sticker cannot be swallowed by Telegram's "same file already in the
    // set" rule.
    let replaced = if kept.is_empty() && !to_add.is_empty() {
        to_delete.pop()
    } else {
        None
    };

    for s in &to_delete {
        api.delete_sticker_from_set(&s.file_id).await?;
        report.removed += 1;
    }
    let mut adds = to_add.iter();
    if let Some(old) = replaced {
        let first = adds.next().expect("to_add is not empty");
        api.replace_sticker_in_set(owner_user_id, name, &old.file_id, &input_sticker(first))
            .await?;
        report.removed += 1;
        report.added += 1;
    }
    for it in adds {
        api.add_sticker_to_set(owner_user_id, name, &input_sticker(it)).await?;
        report.added += 1;
    }

    let set = if report.added == 0 && report.removed == 0 {
        current.clone()
    } else {
        api.get_sticker_set(name).await?
    };
    let new_stickers: Vec<&Sticker> = set
        .stickers
        .iter()
        .filter(|s| !kept_fuids.contains(&s.file_unique_id))
        .collect();
    if new_stickers.len() != to_add.len() || set.stickers.len() != items.len() {
        return Err(TelegramError::Invalid(format!(
            "pack {name} has {} stickers ({} new) after the update, expected {} ({} new); was it changed concurrently?",
            set.stickers.len(),
            new_stickers.len(),
            items.len(),
            to_add.len()
        )));
    }
    let mut fuid_of: HashMap<&str, String> = kept.iter().map(|(k, f)| (*k, f.clone())).collect();
    for (it, s) in to_add.iter().zip(&new_stickers) {
        fuid_of.insert(it.key.as_str(), s.file_unique_id.clone());
    }
    let file_id_of: HashMap<&str, &str> = set
        .stickers
        .iter()
        .map(|s| (s.file_unique_id.as_str(), s.file_id.as_str()))
        .collect();

    // Metadata of stickers that stayed (new ones were uploaded with theirs).
    for it in &items {
        let Some(fuid) = kept.get(it.key.as_str()) else {
            continue;
        };
        let file_id = file_id_of[fuid.as_str()];
        api.set_sticker_emoji_list(file_id, &emoji_list(it)).await?;
        api.set_sticker_keywords(file_id, &keyword_list(it)).await?;
        report.updated += 1;
    }

    // Order.
    let mut order: Vec<&str> = set.stickers.iter().map(|s| s.file_unique_id.as_str()).collect();
    for (position, it) in items.iter().enumerate() {
        let want = fuid_of[it.key.as_str()].as_str();
        if order[position] != want {
            api.set_sticker_position_in_set(file_id_of[want], position as u32)
                .await?;
            let from = order.iter().position(|f| *f == want).expect("sticker is in the pack");
            let moved = order.remove(from);
            order.insert(position, moved);
            report.reordered = true;
        }
    }

    if set.title != title {
        api.set_sticker_set_title(name, &title).await?;
    }
    sync_thumbnail(
        api,
        owner_user_id,
        name,
        thumbnail,
        previous.thumbnail.as_deref(),
        &mut report,
    )
    .await?;

    report.mapping = items
        .iter()
        .map(|it| {
            (
                it.key.clone(),
                (it.content_key.clone(), fuid_of[it.key.as_str()].clone()),
            )
        })
        .collect();
    Ok(report)
}

async fn sync_thumbnail(
    api: &BotApi,
    owner_user_id: i64,
    name: &str,
    thumbnail: Option<&PackThumbnail>,
    previous: Option<&str>,
    report: &mut ExportReport,
) -> Result<(), TelegramError> {
    match thumbnail {
        Some(t) if previous != Some(t.key.as_str()) => {
            api.set_sticker_set_thumbnail(name, owner_user_id, Some(&t.png)).await?;
            report.thumbnail_changed = true;
        }
        None if previous.is_some() => {
            api.set_sticker_set_thumbnail(name, owner_user_id, None).await?;
            report.thumbnail_changed = true;
        }
        _ => {}
    }
    report.thumbnail = thumbnail.map(|t| t.key.clone());
    Ok(())
}

/// Can `s` be a sticker's emoji? Telegram refuses anything else and the
/// whole export would fail, so words typed into the emoji field are left
/// out: no letters (except enclosed ones like 🅰️ Ⓜ️), whitespace or
/// controls, not plain ASCII, and digits only in keycaps (1️⃣). The
/// server's problems report uses it to point out entries an export drops.
pub fn is_emoji(s: &str) -> bool {
    let enclosed = |c: char| matches!(c, '\u{2460}'..='\u{24FF}' | '\u{1F100}'..='\u{1F1FF}');
    !s.is_ascii()
        && !s
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || (c.is_alphabetic() && !enclosed(c)))
        && (s.contains('\u{20E3}') || !s.chars().any(|c| c.is_ascii_digit()))
}

fn input_sticker(it: &ExportItem) -> InputSticker {
    let ext = match it.format {
        StickerFormat::Video => "webm",
        StickerFormat::Animated => "tgs",
        StickerFormat::Static if it.bytes.starts_with(b"\x89PNG\r\n\x1a\n") => "png",
        StickerFormat::Static => "webp",
    };
    InputSticker {
        bytes: it.bytes.clone(),
        filename: format!("sticker.{ext}"),
        format: it.format,
        emoji_list: emoji_list(it),
        keywords: keyword_list(it),
    }
}

fn emoji_list(it: &ExportItem) -> Vec<String> {
    let mut seen = HashSet::new();
    let list: Vec<String> = it
        .emojis
        .iter()
        .map(|e| e.trim())
        .filter(|e| is_emoji(e) && seen.insert(*e))
        .take(MAX_EMOJIS)
        .map(str::to_string)
        .collect();
    if list.is_empty() {
        vec![DEFAULT_EMOJI.to_string()]
    } else {
        list
    }
}

fn keyword_list(it: &ExportItem) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut total = 0;
    let mut out = Vec::new();
    for k in it.keywords.iter().map(|k| k.trim()) {
        let len = k.chars().count();
        if k.is_empty() || total + len > MAX_KEYWORD_CHARS || !seen.insert(k) {
            continue;
        }
        total += len;
        out.push(k.to_string());
        if out.len() == MAX_KEYWORDS {
            break;
        }
    }
    out
}

fn clamp_title(title: &str, fallback: &str) -> String {
    let t: String = title.trim().chars().take(MAX_TITLE_CHARS).collect();
    if t.is_empty() {
        fallback.chars().take(MAX_TITLE_CHARS).collect()
    } else {
        t
    }
}

/// `^[A-Za-z][A-Za-z0-9_]*_by_<bot>$`, <= 64 chars, no `__`.
///
/// The bot username is compared case-insensitively (a leading `@` is ignored).
pub fn valid_pack_name(name: &str, bot_username: &str) -> bool {
    let bot = bot_username.trim_start_matches('@');
    if bot.is_empty() || name.len() > 64 || name.contains("__") {
        return false;
    }
    if !name.starts_with(|c: char| c.is_ascii_alphabetic())
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return false;
    }
    let suffix = format!("_by_{bot}").to_ascii_lowercase();
    let lower = name.to_ascii_lowercase();
    lower.len() > suffix.len() && lower.ends_with(&suffix)
}

/// A pack name as a user types it, with `_by_<bot>` appended when it is
/// missing: `test` becomes `test_by_<bot>`. A name that already ends in this
/// bot's suffix, or in another bot's (`x_by_other_bot`, which this bot cannot
/// edit), is returned unchanged for [`valid_pack_name`] to judge.
pub fn complete_pack_name(name: &str, bot_username: &str) -> String {
    let bot = bot_username.trim_start_matches('@');
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(&format!("_by_{}", bot.to_ascii_lowercase())) {
        return name.to_string();
    }
    if lower.rsplit_once("_by_").is_some_and(|(_, tail)| tail.ends_with("bot")) {
        return name.to_string();
    }
    format!("{}_by_{bot}", name.trim_end_matches('_'))
}

/// `<slug>_by_<bot>` turned into a valid pack name (dashes become
/// underscores, a leading digit gets a `s` prefix, the stem is shortened to
/// fit 64 characters). `None` if the bot username itself makes that
/// impossible.
pub fn default_pack_name(slug: &str, bot_username: &str) -> Option<String> {
    let bot = bot_username.trim_start_matches('@');
    let suffix = format!("_by_{bot}");
    let mut stem = String::new();
    for c in slug.chars() {
        let c = if c.is_ascii_alphanumeric() { c } else { '_' };
        if c == '_' && (stem.is_empty() || stem.ends_with('_')) {
            continue;
        }
        stem.push(c);
    }
    let mut stem = stem.trim_end_matches('_').to_string();
    if !stem.starts_with(|c: char| c.is_ascii_alphabetic()) {
        stem.insert(0, 's');
    }
    let room = 64usize.checked_sub(suffix.len())?;
    if room == 0 {
        return None;
    }
    stem.truncate(room);
    let stem = stem.trim_end_matches('_');
    let name = format!("{stem}{suffix}");
    valid_pack_name(&name, bot).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_names() {
        let bot = "stickpile_bot";
        for ok in [
            "cats_by_stickpile_bot",
            "Cats2_by_StickPile_Bot",
            "a_b_c_by_stickpile_bot",
            "x_by_stickpile_bot",
        ] {
            assert!(valid_pack_name(ok, bot), "{ok}");
        }
        assert!(valid_pack_name("cats_by_stickpile_bot", "@stickpile_bot"));
        for bad in [
            "",
            "_by_stickpile_bot",
            "1cats_by_stickpile_bot",
            "_cats_by_stickpile_bot",
            "cats__x_by_stickpile_bot",
            "cats_by_other_bot",
            "cats-by_stickpile_bot",
            "cats_by_stickpile_bot_",
            "котики_by_stickpile_bot",
            "cats by_stickpile_bot",
        ] {
            assert!(!valid_pack_name(bad, bot), "{bad}");
        }
        let long = format!("{}_by_stickpile_bot", "a".repeat(64 - 17));
        assert_eq!(long.len(), 64);
        assert!(valid_pack_name(&long, bot));
        assert!(!valid_pack_name(&format!("a{long}"), bot));
        assert!(!valid_pack_name("cats_by_stickpile_bot", ""));
    }

    #[test]
    fn completed_names() {
        let bot = "@Stickpile_bot";
        assert_eq!(complete_pack_name("test", bot), "test_by_Stickpile_bot");
        assert_eq!(complete_pack_name("test_", bot), "test_by_Stickpile_bot");
        assert_eq!(
            complete_pack_name("Cats_by_stickpile_BOT", bot),
            "Cats_by_stickpile_BOT"
        );
        assert_eq!(complete_pack_name("cats_by_other_bot", bot), "cats_by_other_bot");
        assert_eq!(complete_pack_name("made_by_hand", bot), "made_by_hand_by_Stickpile_bot");
        assert!(valid_pack_name(&complete_pack_name("test", bot), bot));
        assert!(!valid_pack_name(&complete_pack_name("cats_by_other_bot", bot), bot));
    }

    #[test]
    fn default_names() {
        let bot = "stickpile_bot";
        assert_eq!(
            default_pack_name("my-cats", bot).as_deref(),
            Some("my_cats_by_stickpile_bot")
        );
        assert_eq!(
            default_pack_name("2024-memes", bot).as_deref(),
            Some("s2024_memes_by_stickpile_bot")
        );
        assert_eq!(
            default_pack_name("--a--b--", bot).as_deref(),
            Some("a_b_by_stickpile_bot")
        );
        assert_eq!(default_pack_name("", bot).as_deref(), Some("s_by_stickpile_bot"));
        let long = default_pack_name(&"x".repeat(100), bot).unwrap();
        assert_eq!(long.len(), 64);
        assert!(valid_pack_name(&long, bot));
        assert_eq!(default_pack_name("cats", &"b".repeat(70)), None);
    }

    fn item(emojis: &[&str], keywords: &[&str]) -> ExportItem {
        ExportItem {
            key: "k".into(),
            content_key: "c".into(),
            bytes: vec![],
            format: StickerFormat::Static,
            emojis: emojis.iter().map(|s| s.to_string()).collect(),
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn sanitising() {
        assert_eq!(emoji_list(&item(&[], &[])), [DEFAULT_EMOJI]);
        assert_eq!(emoji_list(&item(&[" ", "😼", "😼", "🐱"], &[])), ["😼", "🐱"]);
        assert_eq!(emoji_list(&item(&["😀"; 30].map(|_| "x"), &[])).len(), 1);
        let many: Vec<String> = (0..30)
            .map(|i| char::from_u32(0x1F600 + i).unwrap().to_string())
            .collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(emoji_list(&item(&many, &[])).len(), 20);
        // Words typed into the emoji field are left out; Telegram would
        // refuse them and fail the export.
        assert_eq!(emoji_list(&item(&["cat", "😺", "кіт", "lol😂", "12"], &[])), ["😺"]);
        assert_eq!(emoji_list(&item(&["sad"], &[])), [DEFAULT_EMOJI]);
        for e in ["😂", "❤️", "👨‍👩‍👧", "🇺🇦", "1️⃣", "#️⃣", "🅰️", "Ⓜ️", "™️", "🏳️‍🌈"]
        {
            assert!(is_emoji(e), "{e}");
        }
        for e in ["x", "cat", "кіт", "猫", "😂 😂", "12", "a😂", "\u{7}"] {
            assert!(!is_emoji(e), "{e:?}");
        }

        let kws = keyword_list(&item(&[], &["cat", " cat ", "", &"x".repeat(70), "stare"]));
        assert_eq!(kws, ["cat", "stare"]);
        let long: Vec<String> = (0..30).map(|i| format!("k{i:02}")).collect();
        let long: Vec<&str> = long.iter().map(String::as_str).collect();
        let kws = keyword_list(&item(&[], &long));
        assert!(
            kws.len() <= 20 && kws.iter().map(|k| k.chars().count()).sum::<usize>() <= 64,
            "{kws:?}"
        );

        assert_eq!(clamp_title("  ", "fallback"), "fallback");
        assert_eq!(clamp_title(&"é".repeat(100), "x").chars().count(), 64);
    }

    #[test]
    fn png_static_stickers_are_sent_as_png() {
        let mut it = item(&["😼"], &[]);
        it.bytes = b"\x89PNG\r\n\x1a\nrest".to_vec();
        assert_eq!(input_sticker(&it).filename, "sticker.png");
        it.bytes = b"RIFF....WEBP".to_vec();
        assert_eq!(input_sticker(&it).filename, "sticker.webp");
        it.format = StickerFormat::Video;
        assert_eq!(input_sticker(&it).filename, "sticker.webm");
    }
}
