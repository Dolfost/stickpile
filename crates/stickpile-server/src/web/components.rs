//! Reusable pieces: item and set cards, badges, copy controls, like button,
//! time and size formatting.

use maud::{html, Markup, PreEscaped};
use stickpile_api::routes as r;
use stickpile_api::{
    FieldSource, Item, ItemSummary, MediaFormat, MediaKind, SetSource, SetSummary, Variant, VariantInfo, Visibility,
};
use time::OffsetDateTime;

use super::Ctx;

pub fn item_url(id: &str) -> String {
    format!("/items/{id}")
}

pub fn set_url(id: &str) -> String {
    format!("/sets/{id}")
}

pub fn file_url(id: &str, variant: Variant) -> String {
    r::path(r::ITEM_FILE, &[id, variant.as_str()])
}

/// What a grid card needs, from either an [`Item`] or an [`ItemSummary`].
pub struct Card<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub name_ai: bool,
    pub kind: MediaKind,
    pub format: MediaFormat,
    pub width: u32,
    pub height: u32,
    pub quality: Option<u8>,
    pub likes: u64,
    /// `None` when unknown (summaries do not carry the viewer's like).
    pub liked: Option<bool>,
    pub variants: &'a [VariantInfo],
    /// (set id, set name) when the card is shown outside its set.
    pub set: Option<(&'a str, &'a str)>,
}

impl<'a> Card<'a> {
    pub fn from_item(i: &'a Item) -> Card<'a> {
        Card {
            id: &i.id,
            name: &i.name,
            name_ai: i.name_source == FieldSource::Ai,
            kind: i.media.kind,
            format: i.media.format,
            width: i.media.width,
            height: i.media.height,
            quality: i.media.quality,
            likes: i.likes,
            liked: Some(i.liked_by_me),
            variants: &i.media.variants,
            set: None,
        }
    }
    pub fn from_summary(s: &'a ItemSummary) -> Card<'a> {
        Card {
            id: &s.id,
            name: &s.name,
            name_ai: false,
            kind: s.kind,
            format: s.format,
            width: s.width,
            height: s.height,
            quality: s.quality,
            likes: s.likes,
            liked: None,
            variants: &s.variants,
            set: Some((&s.set_id, &s.set_name)),
        }
    }
}

fn has_variant(variants: &[VariantInfo], v: Variant) -> bool {
    variants.iter().any(|x| x.variant == v)
}

/// The file a still is copied from: the original if it is a PNG, else the
/// PNG variant if it exists, else the original (app.js converts it).
pub fn still_copy_source(id: &str, format: MediaFormat, variants: &[VariantInfo]) -> (String, Variant) {
    if format != MediaFormat::Png && has_variant(variants, Variant::Png) {
        (file_url(id, Variant::Png), Variant::Png)
    } else {
        (file_url(id, Variant::Original), Variant::Original)
    }
}

/// Thumbnail `<img>` (lazy, sized to keep the layout stable).
pub fn thumb(id: &str, name: &str, width: u32, height: u32) -> Markup {
    // Thumbs are bounded to 256 px; keep the aspect ratio for layout.
    let (w, h) = if width == 0 || height == 0 {
        (256, 256)
    } else if width >= height {
        (256, (256 * height / width).max(1))
    } else {
        ((256 * width / height).max(1), 256)
    };
    html! {
        img src=(file_url(id, Variant::Thumb)) alt=(name) loading="lazy" decoding="async" width=(w) height=(h);
    }
}

pub fn quality_class(q: u8) -> &'static str {
    match q {
        85..=100 => "q-good",
        65..=84 => "q-ok",
        40..=64 => "q-meh",
        _ => "q-poor",
    }
}

pub fn quality_badge(q: Option<u8>) -> Markup {
    html! {
        @if let Some(q) = q {
            span class=(format!("badge quality {}", quality_class(q))) title=(format!("Quality {q} of 100")) {
                span.sr-only { "Quality " } (q)
            }
        }
    }
}

pub fn kind_label(kind: MediaKind, format: MediaFormat) -> Option<&'static str> {
    match (kind, format) {
        (MediaKind::Still, _) => None,
        (MediaKind::Animation, MediaFormat::Gif) => Some("GIF"),
        (MediaKind::Animation, _) => Some("animated"),
        (MediaKind::Video, MediaFormat::Webm) => Some("WebM"),
        (MediaKind::Video, _) => Some("video"),
    }
}

pub fn kind_badge(kind: MediaKind, format: MediaFormat) -> Markup {
    html! {
        @if let Some(label) = kind_label(kind, format) {
            span.badge.kind title=(format!("{kind:?} ({})", format.extension())) { (label) }
        }
    }
}

pub fn ai_mark() -> Markup {
    html! { abbr.ai title="Suggested by AI: confirm or edit it" { "AI" } }
}

pub fn visibility_badge(v: Visibility) -> Markup {
    let (label, class) = match v {
        Visibility::Private => ("private", "vis-private"),
        Visibility::Unlisted => ("unlisted", "vis-unlisted"),
        Visibility::Public => ("public", "vis-public"),
    };
    html! { span class=(format!("badge {class}")) { (label) } }
}

pub fn source_badge(s: SetSource) -> Markup {
    html! {
        @match s {
            SetSource::Local => {},
            SetSource::Git => span.badge.src title="Git pack" { "git" },
            SetSource::Telegram => span.badge.src title="Imported from Telegram" { "telegram" },
        }
    }
}

/// Small inline SVG icons (decorative: buttons carry their name as text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Edit,
    Download,
    Delete,
    Up,
    Down,
    More,
}

pub fn icon(i: Icon) -> Markup {
    // 24×24 outlines drawn with the current text colour.
    let paths: &[&str] = match i {
        Icon::Edit => &["M4 20h4L19 9l-4-4L4 16z", "M13.5 6.5l4 4"],
        Icon::Download => &["M12 4v11", "M7 10l5 5 5-5", "M5 20h14"],
        Icon::Delete => &["M4 7h16", "M9 7V4h6v3", "M6 7l1 13h10l1-13", "M10 11v5", "M14 11v5"],
        Icon::Up => &["M12 19V5", "M6 11l6-6 6 6"],
        Icon::Down => &["M12 5v14", "M6 13l6 6 6-6"],
        Icon::More => &["M5 12h.01", "M12 12h.01", "M19 12h.01"],
    };
    let width = if i == Icon::More { "3.2" } else { "2" };
    html! {
        svg.icon viewBox="0 0 24 24" width="16" height="16" aria-hidden="true" focusable="false" fill="none"
            stroke="currentColor" stroke-width=(width) stroke-linecap="round" stroke-linejoin="round" {
            @for d in paths { path d=(d) {} }
        }
    }
}

/// Copy controls. Stills are copied as PNG image data; animations and videos
/// get "Copy link" and "Download" (browsers cannot put GIF or video files on
/// the clipboard in a form chat apps accept). The buttons need app.js and
/// stay hidden without it; the download link always works. `dense` (set
/// editor cards) shows Download as an icon button.
pub fn copy_controls(c: &Card<'_>, long: bool, dense: bool) -> Markup {
    let original = file_url(c.id, Variant::Original);
    html! {
        div.copy-controls {
            @if c.kind == MediaKind::Still {
                @let (src, variant) = still_copy_source(c.id, c.format, c.variants);
                button.copy.primary type="button" hidden data-copy="image" data-item=(c.id) data-src=(src)
                    data-variant=(variant.as_str()) { "Copy" }
            } @else {
                button.copy type="button" hidden data-copy="link" data-item=(c.id) data-src=(original)
                    data-variant="original" { "Copy link" }
            }
            @if dense {
                a.button.download.icon-btn href=(original) download title="Download the original file" {
                    (icon(Icon::Download)) span.sr-only { "Download" }
                }
            } @else {
                a.button.download href=(original) download { "Download" }
            }
            @if long && c.kind != MediaKind::Still {
                p.hint {
                    "Browsers cannot put GIFs or videos on the clipboard in a form chat apps accept. "
                    "Copy the link (it works for people signed in to this server) or download the file. "
                    "The desktop client pastes them directly."
                }
            }
        }
    }
}

/// Like toggle: a small form (works without JS) swapped in place by htmx.
pub fn like_form(ctx: &Ctx, id: &str, liked: bool, likes: u64, next: &str) -> Markup {
    let url = format!("/items/{id}/like");
    html! {
        form.like-form method="post" action=(url) hx-post=(url) hx-target="this" hx-swap="outerHTML" {
            (ctx.csrf_input())
            input type="hidden" name="next" value=(next);
            input type="hidden" name="liked" value=(if liked { "0" } else { "1" });
            button.like type="submit" aria-pressed=(if liked { "true" } else { "false" })
                title=(if liked { "Remove your like" } else { "Like" }) {
                span aria-hidden="true" { (if liked { "♥" } else { "♡" }) }
                span.sr-only { (if liked { "Liked" } else { "Like" }) ", " }
                span.count { (likes) }
                span.sr-only { " likes" }
            }
        }
    }
}

/// Read-only grid card (home, search, popular): thumb, badges, name, like,
/// copy. `extra` is shown under the name (e.g. "12 copies").
pub fn item_card(ctx: &Ctx, c: &Card<'_>, extra: Option<Markup>) -> Markup {
    html! {
        article.card id=(format!("card-{}", c.id)) {
            a.thumb href=(item_url(c.id)) {
                (thumb(c.id, c.name, c.width, c.height))
            }
            div.badges {
                (kind_badge(c.kind, c.format))
                (quality_badge(c.quality))
            }
            div.card-body {
                h3.name {
                    a href=(item_url(c.id)) { (c.name) }
                    @if c.name_ai { " " (ai_mark()) }
                }
                @if let Some((sid, sname)) = c.set {
                    p.meta { "in " a href=(set_url(sid)) { (sname) } }
                }
                @if let Some(e) = extra {
                    p.meta { (e) }
                }
                div.card-actions {
                    @if let Some(liked) = c.liked {
                        (like_form(ctx, c.id, liked, c.likes, &ctx.here))
                    } @else {
                        span.likes title="Likes" { span aria-hidden="true" { "♥ " } (c.likes) span.sr-only { " likes" } }
                    }
                    (copy_controls(c, false, false))
                }
            }
        }
    }
}

pub fn set_card(s: &SetSummary) -> Markup {
    html! {
        article.card.set-card {
            a.thumb href=(set_url(&s.id)) {
                @if let Some(cover) = &s.cover_item_id {
                    img src=(file_url(cover, Variant::Thumb)) alt="" loading="lazy" decoding="async" width="256" height="256";
                } @else {
                    span.empty-cover aria-hidden="true" { "∅" }
                }
            }
            div.card-body {
                h3.name { a href=(set_url(&s.id)) { (s.name) } }
                p.meta {
                    (s.item_count) " items · " (visibility_badge(s.visibility)) " " (source_badge(s.source))
                }
                p.meta.small { "by " (s.owner_username) " · " code { (s.slug) } }
            }
        }
    }
}

pub fn grid(cards: Markup) -> Markup {
    html! { div.grid { (cards) } }
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

pub fn fmt_time(ts: i64) -> String {
    match OffsetDateTime::from_unix_timestamp(ts) {
        Ok(t) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02} UTC",
            t.year(),
            u8::from(t.month()),
            t.day(),
            t.hour(),
            t.minute()
        ),
        Err(_) => "?".into(),
    }
}

pub fn fmt_date(ts: i64) -> String {
    match OffsetDateTime::from_unix_timestamp(ts) {
        Ok(t) => format!("{:04}-{:02}-{:02}", t.year(), u8::from(t.month()), t.day()),
        Err(_) => "?".into(),
    }
}

fn iso(ts: i64) -> String {
    match OffsetDateTime::from_unix_timestamp(ts) {
        Ok(t) => format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            t.year(),
            u8::from(t.month()),
            t.day(),
            t.hour(),
            t.minute(),
            t.second()
        ),
        Err(_) => String::new(),
    }
}

pub fn time_el(ts: i64) -> Markup {
    html! { time datetime=(iso(ts)) { (fmt_time(ts)) } }
}

pub fn opt_time(ts: Option<i64>) -> Markup {
    html! {
        @match ts {
            Some(t) => (time_el(t)),
            None => span.muted { "never" },
        }
    }
}

pub fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

pub fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Trusted markup produced by this module (never user input).
pub fn trusted(s: String) -> PreEscaped<String> {
    PreEscaped(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_and_times() {
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(1536), "1.5 KiB");
        assert_eq!(fmt_bytes(5 * 1024 * 1024), "5.0 MiB");
        assert_eq!(fmt_time(0), "1970-01-01 00:00 UTC");
        assert_eq!(iso(86_400 + 61), "1970-01-02T00:01:01Z");
        assert_eq!(quality_class(90), "q-good");
        assert_eq!(quality_class(10), "q-poor");
    }
}
