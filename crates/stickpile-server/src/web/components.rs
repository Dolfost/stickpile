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

/// What the browser copies for an item (docs/web.md, "Copy to clipboard").
/// Browsers put only `image/png` on the clipboard, so every item becomes one
/// PNG, like the desktop client's `image/png` offer for "Other / browser":
///
/// * stills: the original if it is a PNG, else the `png` variant (the
///   server's lossless rendering of the original, nothing left to convert)
///   once it exists; until then the original, which app.js converts;
/// * animations and videos: a still frame that app.js decodes from the
///   original at full size: the frame the thumbnail shows (the first frame,
///   about 10 % into a video). The thumbnail is the fallback when the
///   browser cannot decode the original (e.g. a codec it lacks).
pub struct CopySource {
    pub src: String,
    pub variant: Variant,
    /// A frame of an animation or video, not the whole item.
    pub frame: bool,
}

pub fn copy_source(c: &Card<'_>) -> CopySource {
    let (variant, frame) = match c.kind {
        MediaKind::Still if c.format != MediaFormat::Png && has_variant(c.variants, Variant::Png) => {
            (Variant::Png, false)
        }
        MediaKind::Still => (Variant::Original, false),
        MediaKind::Animation | MediaKind::Video => (Variant::Original, true),
    };
    CopySource {
        src: file_url(c.id, variant),
        variant,
        frame,
    }
}

fn kind_key(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Still => "still",
        MediaKind::Animation => "animation",
        MediaKind::Video => "video",
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
    /// The light/dark switch (shown in the light theme).
    Moon,
    /// The light/dark switch (shown in the dark theme).
    Sun,
    /// The "View" (display options) buttons.
    View,
    /// Copy to the clipboard (the card's quick copy button).
    Copy,
    /// "Copied" (the quick copy button after a copy).
    Check,
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
        Icon::Moon => &["M20.5 14.2A8.5 8.5 0 0 1 9.8 3.5a8.5 8.5 0 1 0 10.7 10.7z"],
        Icon::Sun => &[
            "M12 8a4 4 0 1 0 0 8 4 4 0 1 0 0-8z",
            "M12 2.5v2",
            "M12 19.5v2",
            "M2.5 12h2",
            "M19.5 12h2",
            "M5.3 5.3l1.4 1.4",
            "M17.3 17.3l1.4 1.4",
            "M5.3 18.7l1.4-1.4",
            "M17.3 6.7l1.4-1.4",
        ],
        Icon::View => &["M4 6h16", "M4 12h16", "M4 18h16", "M15 3.5v5", "M9 9.5v5", "M17 15.5v5"],
        // The same outlines as app.js's menu icons.
        Icon::Copy => &["M9 9h10v10H9z", "M5 15V5h10"],
        Icon::Check => &["M5 12.5l4.5 4.5L19 7.5"],
    };
    let width = match i {
        Icon::More => "3.2",
        Icon::Check => "2.6",
        _ => "2",
    };
    let class = match i {
        Icon::Moon => "icon icon-moon",
        Icon::Sun => "icon icon-sun",
        Icon::Copy => "icon icon-copy",
        Icon::Check => "icon icon-check",
        _ => "icon",
    };
    html! {
        svg class=(class) viewBox="0 0 24 24" width="16" height="16" aria-hidden="true" focusable="false" fill="none"
            stroke="currentColor" stroke-width=(width) stroke-linecap="round" stroke-linejoin="round" {
            @for d in paths { path d=(d) {} }
        }
    }
}

/// Copy controls: "Copy" puts a PNG on the clipboard ([`copy_source`]; for
/// animations and videos a still frame, labelled so), plus "Download". The
/// item page (`long`) also offers "Copy link" for animations and videos and
/// explains the choice. The wrapper carries what app.js needs (`data-copy-*`)
/// for these buttons, the card's quick copy button, its menu and Ctrl+C.
/// The copy buttons need app.js and stay hidden without it; the download
/// link always works. `dense` (set editor cards) shows Download as an icon
/// button.
pub fn copy_controls(c: &Card<'_>, long: bool, dense: bool) -> Markup {
    let original = file_url(c.id, Variant::Original);
    let source = copy_source(c);
    let label = match (source.frame, long) {
        (false, false) => "Copy",
        (false, true) => "Copy image",
        (true, false) => "Copy frame",
        (true, true) => "Copy still frame",
    };
    html! {
        div.copy-controls data-copy-item=(c.id) data-copy-name=(c.name) data-copy-kind=(kind_key(c.kind))
            data-copy-src=(source.src) data-copy-variant=(source.variant.as_str())
            data-copy-thumb=(file_url(c.id, Variant::Thumb)) data-copy-original=(original) data-copy-page[long] {
            button.copy.primary[!source.frame] type="button" hidden data-copy="image"
                title=[source.frame.then_some("Copy a still frame as a PNG: browsers cannot copy GIFs or videos")] {
                (label)
            }
            @if long && source.frame {
                button.copy type="button" hidden data-copy="link" title="Copy the address of the file" { "Copy link" }
            }
            @if dense {
                a.button.download.icon-btn href=(original) download title="Download the original file" {
                    (icon(Icon::Download)) span.sr-only { "Download" }
                }
            } @else {
                a.button.download href=(original) download { "Download" }
            }
            @if long && source.frame {
                p.hint {
                    "Browsers can put only still images on the clipboard, so “Copy still frame” copies the frame "
                    "the thumbnail shows, as a PNG without motion. To send the "
                    (if c.kind == MediaKind::Video { "video" } else { "animation" })
                    ", download it and drag the file into the chat. “Copy link” copies the file's address, "
                    "which opens only for people signed in to this server. The desktop client pastes GIFs "
                    "and videos directly."
                }
            }
            @if long {
                // Shown by app.js on plain http:// pages (no Clipboard API there).
                p.warning.copy-insecure hidden data-copy-insecure="" {
                    "Copying images needs HTTPS: this page was opened over plain http://, where browsers do not "
                    "let pages put images on the clipboard. Open the image and use your browser's own "
                    "“Copy Image”, or download it."
                }
            }
        }
    }
}

/// The card's quick copy button, on the thumbnail's corner: shown while the
/// pointer is over the card or focus is in it (app.js unhides it; CSS hides
/// it on touch screens). A shortcut for the mouse: keyboards have Ctrl+C on
/// the card and the menu, and screen readers the card's Copy button and the
/// menu, so it is out of the tab order and hidden from them.
pub fn quick_copy(c: &Card<'_>) -> Markup {
    let title = if c.kind == MediaKind::Still {
        "Copy image"
    } else {
        "Copy a still frame"
    };
    html! {
        button.quick-copy type="button" hidden data-copy="image" tabindex="-1" aria-hidden="true" title=(title) {
            (icon(Icon::Copy)) (icon(Icon::Check))
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

/// Read-only grid card (home, search, better versions): thumb, badges,
/// name, like, copy. `extra` is shown under the name (e.g. "12 copies").
/// Like the set editor's cards it has a context menu (app.js: right click,
/// the context-menu key, Shift+F10 or "⋯") with copy, like, open and
/// download, and the quick copy button.
pub fn item_card(ctx: &Ctx, c: &Card<'_>, extra: Option<Markup>) -> Markup {
    html! {
        article.card id=(format!("card-{}", c.id)) data-menu="" data-item-url=(item_url(c.id))
            data-download-url=(file_url(c.id, Variant::Original)) {
            a.thumb href=(item_url(c.id)) {
                (thumb(c.id, c.name, c.width, c.height))
            }
            (quick_copy(c))
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
                    (menu_button(c.name))
                }
            }
        }
    }
}

/// "⋯": opens the card's context menu. Needs app.js, hidden without it.
pub fn menu_button(name: &str) -> Markup {
    html! {
        button.icon-btn.card-menu-btn type="button" hidden data-menu-button="" aria-haspopup="menu"
            aria-expanded="false" title="More actions" {
            (icon(Icon::More)) span.sr-only { "More actions for " (name) }
        }
    }
}

/// A set in a list. Each part has a class of its own, so the list's display
/// options (`hide-*` classes on [`set_list`]) can hide it without gaps. The
/// cover is the set's cover image, else its cover item (`cover_url`).
pub fn set_card(s: &SetSummary) -> Markup {
    let cover = s
        .cover_url
        .clone()
        .or_else(|| s.cover_item_id.as_deref().map(|id| file_url(id, Variant::Thumb)));
    html! {
        article.card.set-card {
            a.thumb href=(set_url(&s.id)) {
                @if let Some(cover) = &cover {
                    img src=(cover) alt="" loading="lazy" decoding="async" width="256" height="256";
                } @else {
                    span.empty-cover aria-hidden="true" { "∅" }
                }
            }
            div.card-body {
                h3.name { a href=(set_url(&s.id)) { (s.name) } }
                @if !s.description.is_empty() {
                    p.set-desc { (s.description) }
                }
                p.meta.set-facts {
                    span.set-count { (plural(s.item_count, "item", "items")) }
                    span.set-badges { (visibility_badge(s.visibility)) " " (source_badge(s.source)) }
                }
                p.meta.small.set-facts {
                    span.set-owner { "by " (s.owner_username) }
                    span.set-slug { code { (s.slug) } }
                }
            }
        }
    }
}

pub fn grid(cards: Markup) -> Markup {
    html! { div.grid { (cards) } }
}

// ---- display options (begin) ----------------------------------------------

/// Show/hide toggles of set lists: (key, label, hint). prefs.js puts
/// `hide-{key}` on the list.
pub const SET_PARTS: [(&str, &str, &str); 6] = [
    ("cover", "Cover", ""),
    ("description", "Description", ""),
    ("count", "Item count", ""),
    ("badges", "Labels", "visibility, source"),
    ("owner", "Owner", ""),
    ("slug", "Slug", ""),
];

/// Styles of set lists: (value, label, hint).
const SET_STYLES: [(&str, &str, &str); 2] = [
    ("regular", "Regular cards", "cover, name and details"),
    ("compact", "Compact", "smaller cards, more per row"),
];

/// Set cards (`/sets`, home) in a wrapper that carries the list's display
/// options. The script as its first child applies the remembered options
/// before the cards are parsed, so they never show up and vanish again.
pub fn set_list(sets: &[SetSummary]) -> Markup {
    html! {
        div.set-list data-view-prefs="sets" {
            script src=(crate::assets::url("prefs.js")) {}
            div.grid.set-grid { @for s in sets { (set_card(s)) } }
        }
    }
}

/// The "View" control of set lists. All of it needs JavaScript (the styles
/// are not in the URL), so it is hidden without.
pub fn set_list_view() -> Markup {
    view_menu("sets", "sets-view", &SET_STYLES, "regular", &SET_PARTS, true)
}

/// A "View" disclosure: a style (radio buttons named `style_name`) and
/// show/hide toggles. prefs.js applies a choice at once to the elements
/// marked `data-view-prefs=(kind)` and remembers it in localStorage; the
/// toggles need it and stay hidden without. `needs_js` hides the whole
/// control without JavaScript (the set page's style instead works through
/// its filter form).
pub fn view_menu(
    kind: &str,
    style_name: &str,
    styles: &[(&str, &str, &str)],
    current: &str,
    parts: &[(&str, &str, &str)],
    needs_js: bool,
) -> Markup {
    html! {
        details.view-menu id=(format!("{kind}-view")) data-view-menu=(kind) hidden[needs_js] {
            summary.button { (icon(Icon::View)) "View" }
            div.view-panel {
                fieldset.view-style {
                    legend { "Style" }
                    @for (value, label, hint) in styles {
                        label.radio {
                            input type="radio" name=(style_name) value=(value) checked[*value == current];
                            span.opt { span.opt-name { (label) } @if !hint.is_empty() { span.opt-hint { (hint) } } }
                        }
                    }
                }
                fieldset.view-show hidden data-view-show="" {
                    legend { "Show" }
                    @for (key, label, hint) in parts {
                        label.check {
                            input type="checkbox" value=(key) checked data-view-part="";
                            // "Show emojis", not "Emojis": the upload tray on the
                            // same page has fields with those names.
                            span.opt { span.opt-name { span.sr-only { "Show " } (label) } @if !hint.is_empty() { span.opt-hint { (hint) } } }
                        }
                    }
                    p.view-note hidden data-view-note="" {}
                }
            }
        }
    }
}

// ---- display options (end) ------------------------------------------------

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
