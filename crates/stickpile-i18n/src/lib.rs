//! Translations (docs/languages.md). The English text is the key: code
//! writes `t(lang, "Sets")`, and a language other than English looks the
//! text up in its catalog, falling back to the English when it is missing.
//!
//! Catalogs are plain text files in `catalog/<lang>/*.tsv`, compiled in:
//! one entry per line, the English text, a tab, the translation. `#` starts
//! a comment line. Escapes: `\t`, `\n`, `\\`.
//!
//! * Placeholders: `{name}` in both texts, filled by [`tf`].
//! * Plurals: the key is `one|many` (`{n} sticker|{n} stickers`) and the
//!   translation lists the language's forms separated by `|`; Ukrainian has
//!   three (one, few, many), see [`plural_form`]. [`tp`] picks one.
//!
//! The server and the clients each keep "the current language" in their own
//! way (per request, per process) and wrap these functions.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// A language the UI is translated into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    En,
    Uk,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Uk];

    /// BCP 47 code, as in `<html lang>` and the settings (`en`, `uk`).
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Uk => "uk",
        }
    }

    /// The language's name in itself, for language pickers.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Uk => "Українська",
        }
    }

    /// A code or a locale: `uk`, `uk-UA`, `uk_UA.UTF-8`, `en-GB`; case
    /// ignored. `None` for anything else (the caller falls back).
    pub fn parse(s: &str) -> Option<Lang> {
        let primary = s
            .trim()
            .split(['-', '_', '.', '@'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match primary.as_str() {
            "en" => Some(Lang::En),
            // `ua` is the country, but people type it for the language.
            "uk" | "ua" => Some(Lang::Uk),
            _ => None,
        }
    }

    /// The best match of an `Accept-Language` header (`uk-UA,uk;q=0.9,en;q=0.8`),
    /// highest quality first; `None` when no listed language is translated.
    pub fn from_accept_language(header: &str) -> Option<Lang> {
        let mut best: Option<(f32, usize, Lang)> = None;
        for (i, part) in header.split(',').enumerate() {
            let mut it = part.split(';');
            let tag = it.next().unwrap_or("").trim();
            let q = it
                .find_map(|p| p.trim().strip_prefix("q="))
                .and_then(|q| q.trim().parse::<f32>().ok())
                .unwrap_or(1.0);
            let Some(lang) = Lang::parse(tag) else { continue };
            if q <= 0.0 {
                continue;
            }
            // Higher q wins; on a tie the earlier one.
            if best.is_none_or(|(bq, _, _)| q > bq) {
                best = Some((q, i, lang));
            }
        }
        best.map(|(_, _, l)| l)
    }

    /// The language of a POSIX locale environment (`LC_ALL`, then
    /// `LC_MESSAGES`, then `LANG`; `LANGUAGE`'s first entry before them).
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Option<Lang> {
        if let Some(first) = get("LANGUAGE").and_then(|l| l.split(':').next().map(str::to_string)) {
            if let Some(l) = Lang::parse(&first) {
                return Some(l);
            }
        }
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .filter_map(|k| get(k))
            .find(|v| !v.is_empty() && v != "C" && v != "POSIX")
            .and_then(|v| Lang::parse(&v))
    }
}

impl std::fmt::Display for Lang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

/// The compiled-in catalog files of a language: (file name, text).
fn named_sources(lang: Lang) -> &'static [(&'static str, &'static str)] {
    match lang {
        Lang::En => &[],
        Lang::Uk => &[
            ("common", include_str!("../catalog/uk/common.tsv")),
            ("web", include_str!("../catalog/uk/web.tsv")),
            ("web-sets", include_str!("../catalog/uk/web-sets.tsv")),
            ("web-items", include_str!("../catalog/uk/web-items.tsv")),
            ("web-collab", include_str!("../catalog/uk/web-collab.tsv")),
            ("web-account", include_str!("../catalog/uk/web-account.tsv")),
            ("web-admin", include_str!("../catalog/uk/web-admin.tsv")),
            ("web-problems", include_str!("../catalog/uk/web-problems.tsv")),
            ("server", include_str!("../catalog/uk/server.tsv")),
            ("desktop", include_str!("../catalog/uk/desktop.tsv")),
            ("js", include_str!("../catalog/uk/js.tsv")),
        ],
    }
}

fn sources(lang: Lang) -> Vec<&'static str> {
    named_sources(lang).iter().map(|(_, text)| *text).collect()
}

/// The entries of one catalog file (`js` for the web UI's scripts).
pub fn file_entries(lang: Lang, name: &str) -> Vec<(String, String)> {
    named_sources(lang)
        .iter()
        .filter(|(n, _)| *n == name)
        .flat_map(|(_, text)| parse_catalog(text).0)
        .collect()
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some(o) => {
                    out.push('\\');
                    out.push(o);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parse catalog text: (English, translation) pairs, in order. Lines that
/// are not entries are reported as errors with their line number.
pub fn parse_catalog(text: &str) -> (Vec<(String, String)>, Vec<String>) {
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        match line.split_once('\t') {
            Some((en, tr)) if !en.is_empty() && !tr.is_empty() && !tr.contains('\t') => {
                entries.push((unescape(en), unescape(tr)));
            }
            _ => errors.push(format!("line {}: expected `English<TAB>translation`: {line:?}", n + 1)),
        }
    }
    (entries, errors)
}

type Catalog = HashMap<String, &'static str>;

fn catalog(lang: Lang) -> &'static Catalog {
    static UK: OnceLock<Catalog> = OnceLock::new();
    static EN: OnceLock<Catalog> = OnceLock::new();
    let cell = match lang {
        Lang::En => &EN,
        Lang::Uk => &UK,
    };
    cell.get_or_init(|| {
        let mut map = HashMap::new();
        for src in sources(lang) {
            let (entries, _) = parse_catalog(src);
            for (en, tr) in entries {
                // Leaked once per process: the catalog lives as long as it.
                map.entry(en).or_insert(&*Box::leak(tr.into_boxed_str()));
            }
        }
        map
    })
}

/// The translation of `en`, if the catalog has one.
pub fn lookup(lang: Lang, en: &str) -> Option<&'static str> {
    if lang == Lang::En {
        return None;
    }
    catalog(lang).get(en).copied()
}

/// `en` in `lang` (English when there is no translation).
pub fn t(lang: Lang, en: &'static str) -> &'static str {
    lookup(lang, en).unwrap_or(en)
}

/// `en` where the same English needs different translations by context
/// (an object, a heading): the catalog key is `{ctx}::{en}`, and without
/// one the English is used.
pub fn tc(lang: Lang, ctx: &'static str, en: &'static str) -> &'static str {
    lookup(lang, &format!("{ctx}::{en}")).unwrap_or(en)
}

/// [`t`] for a text that is not a literal (a server message, say).
pub fn t_dyn(lang: Lang, en: &str) -> String {
    lookup(lang, en).map(str::to_string).unwrap_or_else(|| en.to_string())
}

/// Fill `{name}` placeholders.
pub fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = template.to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), &value.to_string());
    }
    out
}

/// [`t`] with placeholders: `tf(lang, "by {owner}", &[("owner", &name)])`.
pub fn tf(lang: Lang, en: &'static str, args: &[(&str, &dyn Display)]) -> String {
    fill(t(lang, en), args)
}

/// Index of the plural form for `n`: English `one` (0) / `other` (1);
/// Ukrainian `one` (0: 1, 21, 31...), `few` (1: 2-4, 22-24...), `many` (2).
pub fn plural_form(lang: Lang, n: u64) -> usize {
    match lang {
        Lang::En => usize::from(n != 1),
        Lang::Uk => {
            let (m10, m100) = (n % 10, n % 100);
            if m10 == 1 && m100 != 11 {
                0
            } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                1
            } else {
                2
            }
        }
    }
}

/// A counted phrase: `tp(lang, n, "{n} sticker", "{n} stickers")`. `{n}`
/// is filled; other placeholders can follow with [`fill`].
pub fn tp(lang: Lang, n: u64, one: &'static str, many: &'static str) -> String {
    let english = if n == 1 { one } else { many };
    let key = format!("{one}|{many}");
    let text = match lookup(lang, &key) {
        Some(forms) => {
            let forms: Vec<&str> = forms.split('|').collect();
            let i = plural_form(lang, n).min(forms.len().saturating_sub(1));
            forms.get(i).copied().unwrap_or(english)
        }
        None => english,
    };
    text.replace("{n}", &n.to_string())
}

/// Every catalog file of `lang`, for checks (entries and parse errors).
pub fn catalog_files(lang: Lang) -> Vec<&'static str> {
    sources(lang)
}

/// Keys of translatable texts in Rust source: the literal arguments of
/// `t(`, `tf(`, `tp(` / `plural(` (as `{n} one|{n} many` keys) and of the
/// `AppError` constructors that take a message. Used by the coverage tests
/// here and of the clients.
pub fn source_keys(src: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let calls: [(&str, usize); 5] = [("t(", 1), ("tf(", 1), ("tp(", 2), ("plural(", 2), ("tc(", 2)];
    let errors = [
        "AppError::bad_request(",
        "AppError::forbidden(",
        "AppError::conflict(",
        "AppError::unauthorized(",
        "AppError::too_large(",
        "AppError::unsupported_media(",
        "AppError::unavailable(",
        "AppError::not_implemented(",
        "AppError::csrf(",
    ];
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < src.len() {
        let rest = &src[i..];
        // A call: preceded by something that is not part of an identifier.
        let boundary =
            i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_' || bytes[i - 1] == b':');
        let matched = calls
            .iter()
            .find(|(name, _)| boundary && rest.starts_with(name))
            .map(|(name, n)| (name.len(), *n, true))
            .or_else(|| {
                errors
                    .iter()
                    .find(|e| rest.starts_with(*e))
                    .map(|e| (e.len(), 1, false))
            });
        let Some((len, wanted, counted)) = matched else {
            i += rest.chars().next().map_or(1, char::len_utf8);
            continue;
        };
        let mut j = i + len;
        let mut lits = Vec::new();
        // tp(n, "one", "many") / plural(n, "one", "many"): skip the count.
        let skip_first_arg = counted && wanted == 2 && !rest.starts_with("tc(");
        if skip_first_arg {
            while j < src.len() && bytes[j] != b',' && bytes[j] != b')' {
                j += 1;
            }
            j += 1;
        }
        while lits.len() < wanted {
            while j < src.len() && (bytes[j] as char).is_whitespace() {
                j += 1;
            }
            if j >= src.len() || bytes[j] != b'"' {
                break;
            }
            let (lit, end) = read_literal(src, j + 1);
            lits.push(lit);
            j = end;
            while j < src.len() && (bytes[j] as char).is_whitespace() {
                j += 1;
            }
            if j < src.len() && bytes[j] == b',' {
                j += 1;
            }
        }
        match (wanted, lits.as_slice()) {
            (1, [one]) => keys.push(one.clone()),
            (2, [one, many]) if src[i..].starts_with("plural(") => keys.push(format!("{{n}} {one}|{{n}} {many}")),
            (2, [ctx, en]) if src[i..].starts_with("tc(") => keys.push(format!("{ctx}::{en}")),
            (2, [one, many]) => keys.push(format!("{one}|{many}")),
            _ => {}
        }
        i += len;
    }
    keys
}

/// Keys of the web UI's scripts: the double-quoted literal first arguments
/// of `tr(` and `trf(` (their helpers read `<body data-i18n>`).
pub fn js_keys(src: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let bytes = src.as_bytes();
    for name in ["tr(\"", "trf(\""] {
        let mut from = 0;
        while let Some(pos) = src[from..].find(name) {
            let i = from + pos;
            let boundary =
                i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_' || bytes[i - 1] == b'.');
            let (lit, end) = read_literal(src, i + name.len());
            if boundary {
                keys.push(lit);
            }
            from = end.max(i + 1);
        }
    }
    keys
}

/// A Rust string literal's text from just after its opening quote; and the
/// index after the closing quote.
fn read_literal(src: &str, start: usize) -> (String, usize) {
    let mut out = String::new();
    let mut chars = src[start..].char_indices().peekable();
    while let Some((k, c)) = chars.next() {
        match c {
            '"' => return (out, start + k + 1),
            '\\' => match chars.next() {
                Some((_, 'n')) => out.push('\n'),
                Some((_, 't')) => out.push('\t'),
                Some((_, '\n')) => {
                    while chars.peek().is_some_and(|(_, c)| c.is_whitespace()) {
                        chars.next();
                    }
                }
                Some((_, o)) => out.push(o),
                None => break,
            },
            c => out.push(c),
        }
    }
    (out, src.len())
}

/// The language of a single-user program (the desktop client): set once it
/// is known, read by every text it shows. The server keeps one per request
/// instead.
pub mod global {
    use std::fmt::Display;
    use std::sync::atomic::{AtomicU8, Ordering};

    use super::Lang;

    static CURRENT: AtomicU8 = AtomicU8::new(0);

    pub fn set(lang: Lang) {
        let v = match lang {
            Lang::En => 0,
            Lang::Uk => 1,
        };
        CURRENT.store(v, Ordering::Relaxed);
    }

    pub fn get() -> Lang {
        match CURRENT.load(Ordering::Relaxed) {
            1 => Lang::Uk,
            _ => Lang::En,
        }
    }

    pub fn t(en: &'static str) -> &'static str {
        super::t(get(), en)
    }

    pub fn tf(en: &'static str, args: &[(&str, &dyn Display)]) -> String {
        super::tf(get(), en, args)
    }

    pub fn tp(n: u64, one: &'static str, many: &'static str) -> String {
        super::tp(get(), n, one, many)
    }

    pub fn t_dyn(en: &str) -> String {
        super::t_dyn(get(), en)
    }
}

#[cfg(test)]
mod tests;
