//! Names, descriptions and tags in the shapes Discord accepts.
//!
//! Rules (Discord developer docs, resources/sticker and resources/emoji):
//! sticker name 2-30 characters, description empty or 2-100 characters,
//! tags at most 200 characters; emoji name 2-32 characters of `A-Z a-z 0-9 _`
//! (the character set is the rule the Discord client enforces and the API
//! answers with when it is broken; the API reference lists only "name").

use std::collections::HashSet;

use unicode_normalization::UnicodeNormalization;

/// Tag for a sticker whose item has no emoji. Discord needs a non-empty
/// `tags` value to create a sticker.
pub const DEFAULT_TAG_EMOJI: &str = "🙂";
pub const STICKER_NAME_CHARS: (usize, usize) = (2, 30);
pub const STICKER_DESCRIPTION_CHARS: (usize, usize) = (2, 100);
pub const STICKER_TAGS_MAX_CHARS: usize = 200;
pub const EMOJI_NAME_CHARS: (usize, usize) = (2, 32);

/// Can `s` be a sticker's emoji? Words typed into an emoji field are left
/// out: no letters (except enclosed ones like 🅰️ Ⓜ️), whitespace or
/// controls, not plain ASCII, and digits only in keycaps (1️⃣). The same rule
/// as `stickpile_telegram::is_emoji`.
pub fn is_emoji(s: &str) -> bool {
    let enclosed = |c: char| matches!(c, '\u{2460}'..='\u{24FF}' | '\u{1F100}'..='\u{1F1FF}');
    !s.is_ascii()
        && !s
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || (c.is_alphabetic() && !enclosed(c)))
        && (s.contains('\u{20E3}') || !s.chars().any(|c| c.is_ascii_digit()))
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clamp(s: &str, max: usize) -> String {
    s.chars().take(max).collect::<String>().trim_end().to_string()
}

/// A sticker name (2-30 characters) from an item name: whitespace
/// collapsed, control characters dropped, cut to 30 characters. A name that
/// ends up shorter than 2 characters is replaced by `fallback` (cut the same
/// way), and a fallback that is too short by `"sticker"`.
pub fn sticker_name(name: &str, fallback: &str) -> String {
    let (min, max) = STICKER_NAME_CHARS;
    for candidate in [name, fallback, "sticker"] {
        let clean: String = candidate.chars().filter(|c| !c.is_control()).collect();
        let s = clamp(&collapse_whitespace(&clean), max);
        if s.chars().count() >= min {
            return s;
        }
    }
    unreachable!("\"sticker\" is a valid name")
}

/// A sticker description: empty, or 2-100 characters. Text of a single
/// character becomes empty; longer text is cut to 100 characters.
pub fn sticker_description(text: &str) -> String {
    let (min, max) = STICKER_DESCRIPTION_CHARS;
    let clean: String = text.chars().filter(|c| !c.is_control()).collect();
    let s = clamp(&collapse_whitespace(&clean), max);
    if s.chars().count() < min {
        String::new()
    } else {
        s
    }
}

/// The `tags` of a guild sticker: the item's first emoji (entries that are
/// not emoji are skipped), or [`DEFAULT_TAG_EMOJI`].
///
/// Discord's docs call the field "autocomplete/suggestion tags" and note that
/// the Discord client fills it with a name generated from an emoji; discord.py
/// documents it as "ideally a raw Unicode emoji". One raw emoji is what the
/// sticker upload dialog's "related emoji" field holds, so that is what
/// stickpile sends.
pub fn sticker_tags(emojis: &[String]) -> String {
    let tag = emojis
        .iter()
        .map(|e| e.trim())
        .find(|e| is_emoji(e))
        .unwrap_or(DEFAULT_TAG_EMOJI);
    tag.chars().take(STICKER_TAGS_MAX_CHARS).collect()
}

/// Emoji found in a sticker's `tags` (import): every comma-separated entry
/// that is an emoji. Other entries are returned as keywords.
pub fn split_tags(tags: &str) -> (Vec<String>, Vec<String>) {
    let mut emojis = Vec::new();
    let mut keywords = Vec::new();
    for t in tags.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        if is_emoji(t) {
            if !emojis.iter().any(|e| e == t) {
                emojis.push(t.to_string());
            }
        } else if !keywords.iter().any(|k| k == t) {
            keywords.push(t.to_string());
        }
    }
    (emojis, keywords)
}

/// An emoji name (2-32 characters of `A-Z a-z 0-9 _`) from an item name.
/// Accents are removed (`café` -> `cafe`), every other character becomes
/// `_`, runs of `_` collapse and the ends are trimmed. A result shorter than
/// 2 characters falls back to `fallback` (sanitised the same way), then to
/// `"emoji"`.
pub fn emoji_name(name: &str, fallback: &str) -> String {
    let (min, max) = EMOJI_NAME_CHARS;
    for candidate in [name, fallback, "emoji"] {
        let mut out = String::new();
        for c in candidate.nfkd() {
            if c.is_ascii_alphanumeric() {
                out.push(c);
            } else if unicode_normalization::char::is_combining_mark(c) {
                continue;
            } else if !out.is_empty() && !out.ends_with('_') {
                out.push('_');
            }
        }
        out.truncate(max);
        let out = out.trim_matches('_');
        if out.len() >= min {
            return out.to_string();
        }
    }
    unreachable!("\"emoji\" is a valid name")
}

/// `base`, or `base_2`, `base_3`, ... (cut to fit 32 characters): the first
/// one not in `taken` (compared case-insensitively). The result is added to
/// `taken`.
pub fn unique_emoji_name(base: &str, taken: &mut HashSet<String>) -> String {
    let max = EMOJI_NAME_CHARS.1;
    let mut n = 1u32;
    loop {
        let candidate = if n == 1 {
            base.to_string()
        } else {
            let suffix = format!("_{n}");
            let stem: String = base.chars().take(max - suffix.len()).collect();
            format!("{}{suffix}", stem.trim_end_matches('_'))
        };
        if taken.insert(candidate.to_ascii_lowercase()) {
            return candidate;
        }
        n += 1;
    }
}

/// True if `name` is a valid emoji name.
pub fn valid_emoji_name(name: &str) -> bool {
    let (min, max) = EMOJI_NAME_CHARS;
    (min..=max).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticker_names() {
        assert_eq!(sticker_name("  grumpy   cat ", "x"), "grumpy cat");
        assert_eq!(sticker_name("a", "sticker 7"), "sticker 7");
        assert_eq!(sticker_name("", ""), "sticker");
        assert_eq!(sticker_name(&"é".repeat(40), "x").chars().count(), 30);
        assert_eq!(sticker_name("кіт\u{7}", "x"), "кіт");
    }

    #[test]
    fn descriptions() {
        assert_eq!(sticker_description(""), "");
        assert_eq!(sticker_description("x"), "");
        assert_eq!(sticker_description(" a  cat "), "a cat");
        assert_eq!(sticker_description(&"y".repeat(150)).len(), 100);
    }

    #[test]
    fn tags() {
        let e = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(sticker_tags(&e(&["cat", " 😼 ", "🐱"])), "😼");
        assert_eq!(sticker_tags(&e(&["sad"])), DEFAULT_TAG_EMOJI);
        assert_eq!(sticker_tags(&[]), DEFAULT_TAG_EMOJI);
        assert_eq!(
            split_tags("😼, cat,🐱, cat, ,stare"),
            (e(&["😼", "🐱"]), e(&["cat", "stare"]))
        );
        assert_eq!(split_tags("grinning"), (vec![], e(&["grinning"])));
    }

    #[test]
    fn emoji_names() {
        assert_eq!(emoji_name("Grumpy cat!", "x"), "Grumpy_cat");
        assert_eq!(emoji_name("café crème", "x"), "cafe_creme");
        assert_eq!(emoji_name("кіт", "item 12"), "item_12");
        assert_eq!(emoji_name("", "!"), "emoji");
        assert_eq!(emoji_name("__a__b__", "x"), "a_b");
        let long = emoji_name(&"x".repeat(50), "y");
        assert_eq!(long.len(), 32);
        for n in ["Grumpy_cat", "cafe_creme", "item_12", "emoji", &long] {
            assert!(valid_emoji_name(n), "{n}");
        }
        assert!(!valid_emoji_name("a"));
        assert!(!valid_emoji_name("a-b"));
    }

    #[test]
    fn unique_names() {
        let mut taken: HashSet<String> = ["cat".to_string()].into();
        assert_eq!(unique_emoji_name("Cat", &mut taken), "Cat_2");
        assert_eq!(unique_emoji_name("cat", &mut taken), "cat_3");
        assert_eq!(unique_emoji_name("dog", &mut taken), "dog");
        let long = "x".repeat(32);
        taken.insert(long.clone());
        let next = unique_emoji_name(&long, &mut taken);
        assert_eq!(next.len(), 32);
        assert!(next.ends_with("_2"));
    }
}
