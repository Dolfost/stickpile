//! Item page and item actions: inline field editing with AI confirm /
//! override, likes, delete, move, re-run AI, "find a better version".
//!
//! Inline editing: a field is shown as a `.field` block; "Edit" swaps in a
//! small form (`/items/{id}/field/{f}/edit`), saving posts it and swaps the
//! view back. Edited fields become user-sourced; "Confirm" keeps an AI value
//! and marks it as the user's (PATCH `confirm`). Without JavaScript "Edit"
//! leads to the item page's full form (`?edit=1`).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Form;
use maud::{html, Markup};
use serde::Deserialize;
use stickpile_api::{FieldSource, Item, ItemDetail, MediaKind, Set, UpdateItem, Variant};

use super::components::{
    ai_mark, copy_controls, file_url, fmt_bytes, grid, icon, item_card, item_url, kind_badge, like_form, plural,
    quality_badge, quality_class, quick_copy, set_url, thumb, Card, Icon,
};
use super::layout::{self, Nav};
use super::{integrations, Ctx};
use crate::error::{AppError, AppResult};
use crate::i18n::{t, tc, tf};
use crate::service::{items as svc, ui};
use crate::state::{AiStatus, AppState};

// ---------------------------------------------------------------------------
// Fields
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    Keywords,
    Emojis,
    Vibe,
    Tags,
}

impl Field {
    pub const ALL: [Field; 5] = [Field::Name, Field::Keywords, Field::Emojis, Field::Vibe, Field::Tags];

    fn parse(s: &str) -> AppResult<Field> {
        Field::ALL
            .into_iter()
            .find(|f| f.key() == s)
            .ok_or_else(AppError::not_found)
    }
    pub fn key(self) -> &'static str {
        match self {
            Field::Name => "name",
            Field::Keywords => "keywords",
            Field::Emojis => "emojis",
            Field::Vibe => "vibe",
            Field::Tags => "tags",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Field::Name => t("Name"),
            Field::Keywords => t("Keywords"),
            Field::Emojis => t("Emojis"),
            Field::Vibe => t("Vibe"),
            Field::Tags => t("Tags"),
        }
    }
    /// The field's name inside a sentence ("Edit keywords", "Confirm name").
    fn object(self) -> &'static str {
        match self {
            Field::Name => tc("object", "name"),
            Field::Keywords => tc("object", "keywords"),
            Field::Emojis => tc("object", "emojis"),
            Field::Vibe => tc("object", "vibe"),
            Field::Tags => tc("object", "tags"),
        }
    }
    fn hint(self) -> &'static str {
        match self {
            Field::Name => "",
            Field::Keywords => t("What is in it, e.g. cat, stare. Separate with commas. Sent to Telegram as the sticker's search words."),
            Field::Emojis => t("Separate with spaces."),
            Field::Vibe => t("The mood in one word, e.g. tired or smug. Empty clears it."),
            Field::Tags => t("Your own labels for sorting and filtering, e.g. reaction, nsfw (tag:nsfw). Separate with commas. Not sent to Telegram."),
        }
    }
    fn source(self, item: &Item) -> FieldSource {
        match self {
            Field::Name => item.name_source,
            Field::Keywords => item.keywords_source,
            Field::Emojis => item.emojis_source,
            Field::Vibe => item.vibe_source,
            Field::Tags => FieldSource::User,
        }
    }
    /// Current value as the edit form shows it.
    fn text(self, item: &Item) -> String {
        match self {
            Field::Name => item.name.clone(),
            Field::Keywords => item.keywords.join(", "),
            Field::Emojis => item.emojis.join(" "),
            Field::Vibe => item.vibe.clone().unwrap_or_default(),
            Field::Tags => item.tags.join(", "),
        }
    }
    fn is_empty(self, item: &Item) -> bool {
        match self {
            Field::Name => item.name.is_empty(),
            Field::Keywords => item.keywords.is_empty(),
            Field::Emojis => item.emojis.is_empty(),
            Field::Vibe => item.vibe.is_none(),
            Field::Tags => item.tags.is_empty(),
        }
    }
    /// PATCH body setting this field from form text.
    fn update(self, value: &str) -> UpdateItem {
        let mut u = UpdateItem::default();
        match self {
            Field::Name => u.name = Some(value.trim().to_string()),
            Field::Keywords => u.keywords = Some(split_list(value)),
            Field::Emojis => u.emojis = Some(split_emojis(value)),
            Field::Vibe => u.vibe = Some(Some(value.trim().to_string()).filter(|v| !v.is_empty())),
            Field::Tags => u.tags = Some(split_list(value)),
        }
        u
    }
}

/// Comma-separated list (entries may contain spaces).
pub fn split_list(s: &str) -> Vec<String> {
    s.split([',', '\n'])
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .collect()
}

/// Emojis separated by spaces or commas.
pub fn split_emojis(s: &str) -> Vec<String> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .collect()
}

fn field_id(item: &Item, f: Field) -> String {
    format!("f-{}-{}", item.id, f.key())
}

fn field_value(item: &Item, f: Field) -> Markup {
    html! {
        @if f.is_empty(item) {
            span.muted { "—" }
        } @else {
            @match f {
                Field::Name => span.value { (item.name) },
                Field::Vibe => span.value { (item.vibe.as_deref().unwrap_or_default()) },
                Field::Emojis => span.value.emojis { (item.emojis.join(" ")) },
                Field::Keywords => ul.chips { @for k in &item.keywords { li { (k) } } },
                Field::Tags => ul.chips.tags { @for t in &item.tags { li { (t) } } },
            }
        }
    }
}

/// `class` of a field block: `field field-{key}` plus `ai-field` (an AI
/// suggestion), `empty` and `editing`. The per-field class lets the set
/// editor lay fields out differently (and hide most of them in the compact
/// view) without touching the swap unit.
fn field_class(f: Field, ai: bool, empty: bool, editing: bool) -> String {
    let mut c = format!("field field-{}", f.key());
    for (on, name) in [(ai, " ai-field"), (empty, " empty"), (editing, " editing")] {
        if on {
            c.push_str(name);
        }
    }
    c
}

/// Display block of one field (the htmx swap unit).
pub fn field_block(ctx: &Ctx, item: &Item, f: Field, editable: bool) -> Markup {
    let empty = f.is_empty(item);
    let ai = f.source(item) == FieldSource::Ai && !empty;
    let base = format!("/items/{}", item.id);
    let label = f.object();
    html! {
        div class=(field_class(f, ai, empty, false)) id=(field_id(item, f)) {
            dt {
                span.field-label { (f.label()) }
                @if ai { " " (ai_mark()) }
            }
            dd {
                (field_value(item, f))
                @if editable {
                    span.field-actions {
                        // The set editor shows the pencil only; the text
                        // stays for screen readers ("Edit keywords").
                        a.edit href=(format!("{base}?edit=1#edit")) hx-get=(format!("{base}/field/{}/edit", f.key()))
                            hx-target="closest .field" hx-swap="outerHTML" title=(tf("Edit {field}", &[("field", &label)])) {
                            (icon(Icon::Edit)) span.edit-text { (t("Edit")) } span.sr-only { " " (label) }
                        }
                        @if ai {
                            form.inline method="post" action=(format!("{base}/confirm/{}", f.key()))
                                hx-post=(format!("{base}/confirm/{}", f.key())) hx-target="closest .field" hx-swap="outerHTML" {
                                (ctx.csrf_input())
                                input type="hidden" name="next" value=(ctx.here);
                                button.link.confirm type="submit" title=(t("Keep the AI suggestion as yours")) {
                                    span aria-hidden="true" { "✓" } span.confirm-text { " " (t("Confirm")) } span.sr-only { " " (label) }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn field_input(ctx_vibes: &[String], item: &Item, f: Field, value: &str) -> Markup {
    let input_id = format!("in-{}-{}", item.id, f.key());
    let hint_id = format!("{input_id}-hint");
    html! {
        @match f {
            Field::Name => input id=(input_id) type="text" name="value" value=(value) required maxlength="200" autofocus;,
            Field::Vibe => {
                input id=(input_id) type="text" name="value" value=(value) maxlength="100" list="vibe-list"
                    autofocus aria-describedby=(hint_id);
                datalist #vibe-list { @for v in ctx_vibes { option value=(v) {} } }
            },
            // emoji.js adds its picker to fields marked data-emoji-picker.
            Field::Emojis => input id=(input_id) type="text" name="value" value=(value) autofocus
                aria-describedby=(hint_id) data-emoji-picker="";,
            _ => input id=(input_id) type="text" name="value" value=(value) autofocus aria-describedby=(hint_id);,
        }
        @if !f.hint().is_empty() { span.hint id=(hint_id) { (f.hint()) } }
    }
}

/// Edit form of one field (replaces the display block).
fn field_form(ctx: &Ctx, vibes: &[String], item: &Item, f: Field, value: &str, error: Option<&str>) -> Markup {
    let base = format!("/items/{}", item.id);
    let action = format!("{base}/field/{}", f.key());
    html! {
        div class=(field_class(f, false, false, true)) id=(field_id(item, f)) {
            dt { label for=(format!("in-{}-{}", item.id, f.key())) { (f.label()) } }
            dd {
                form.inline-edit method="post" action=(action) hx-post=(action) hx-target="closest .field" hx-swap="outerHTML" {
                    (ctx.csrf_input())
                    input type="hidden" name="next" value=(item_url(&item.id));
                    (field_input(vibes, item, f, value))
                    @if let Some(e) = error { span.error-text role="alert" { (e) } }
                    span.form-buttons {
                        button.primary.small type="submit" { (t("Save")) }
                        a.button.small.cancel href=(item_url(&item.id)) hx-get=(action) hx-target="closest .field" hx-swap="outerHTML" {
                            (t("Cancel"))
                        }
                    }
                }
            }
        }
    }
}

pub(super) fn vibes(state: &AppState) -> Vec<String> {
    state.vocab.vibes.iter().map(|v| v.label.clone()).collect()
}

/// An item in the set editor: thumb (with the quick copy button), badges,
/// editable fields, and one row of small buttons (like, copy, download,
/// reorder, delete, "⋯"). `movable` = (is first, is last) when reordering
/// is possible.
///
/// The `data-*` attributes describe what app.js may offer in the item's
/// context menu (right click, the context-menu key, or "⋯"): only actions
/// this viewer can take here. Like reuses the card's own button, copy the
/// copy controls' `data-copy-*`.
/// `is_cover`: the item is the set's chosen cover (a badge for everyone).
pub fn editor_card(ctx: &Ctx, item: &Item, rights: Rights, movable: Option<(bool, bool)>, is_cover: bool) -> Markup {
    editor_card_in(ctx, item, None, rights, movable, is_cover)
}

/// What the viewer may do with one item where it is shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rights {
    /// Edit its fields.
    pub edit: bool,
    /// Remove it from its set.
    pub remove: bool,
    /// Make it the set's cover.
    pub cover: bool,
}

impl Rights {
    /// From the set's permissions for the viewer and who added the item.
    pub fn of(ctx: &Ctx, set: &Set, item: &Item) -> Rights {
        let p = set.permissions;
        let own = item.added_by_username.as_deref() == Some(ctx.user.user.username.as_str());
        Rights {
            edit: p.edit_all || (p.edit_own && own),
            remove: p.remove_all || (p.edit_own && own),
            cover: p.edit_pack,
        }
    }
}

/// [`editor_card`] shown outside its set (the start page's recently copied
/// items): `set` (id, name) adds a "Set" field linking to it, after the name.
pub fn editor_card_in(
    ctx: &Ctx,
    item: &Item,
    set: Option<(&str, &str)>,
    rights: Rights,
    movable: Option<(bool, bool)>,
    is_cover: bool,
) -> Markup {
    let card = Card::from_item(item);
    let page = item_url(&item.id);
    let move_url = format!("/sets/{}/items/{}/move", item.set_id, item.id);
    let delete_url = format!("/items/{}/delete", item.id);
    let cover_url = format!("/sets/{}/cover", item.set_id);
    let (first, last) = movable.unwrap_or((false, false));
    html! {
        article.card.editor id=(format!("item-{}", item.id)) data-menu=""
            data-item-url=(page) data-download-url=(file_url(&item.id, Variant::Original))
            data-edit-url=[rights.edit.then(|| format!("{page}?edit=1#edit"))]
            data-delete-url=[rights.remove.then_some(&delete_url)]
            data-move-url=[movable.map(|_| &move_url)] data-first[first] data-last[last]
            data-cover-url=[rights.cover.then_some(&cover_url)] data-cover[is_cover] {
            a.thumb href=(page) {
                (thumb(&item.id, &item.name, item.media.width, item.media.height))
            }
            (quick_copy(&card))
            div.badges {
                (kind_badge(item.media.kind, item.media.format))
                (quality_badge(item.media.quality))
                @if is_cover {
                    span.badge.cover-badge title=(t("The set's cover: shown in set lists and on Telegram")) { (t("Cover")) }
                }
            }
            div.card-body {
                dl.fields {
                    @for f in Field::ALL {
                        (field_block(ctx, item, f, rights.edit))
                        @if let (Field::Name, Some((sid, sname))) = (f, set) {
                            div.field.field-set {
                                dt { span.field-label { (t("Set")) } }
                                dd { a href=(set_url(sid)) { (sname) } }
                            }
                        }
                    }
                }
                div.card-actions {
                    (like_form(ctx, &item.id, item.liked_by_me, item.likes, &ctx.here))
                    (copy_controls(&card, false, true))
                    @if rights.remove || movable.is_some() {
                        div.manage {
                            @if movable.is_some() {
                                @for (dir, label, ico, off) in [("up", t("Move up"), Icon::Up, first), ("down", t("Move down"), Icon::Down, last)] {
                                    form.inline method="post" action=(move_url) hx-post=(move_url) hx-target="#set-items" hx-swap="outerHTML" {
                                        (ctx.csrf_input())
                                        input type="hidden" name="dir" value=(dir);
                                        button.icon-btn type="submit" title=(label) disabled[off] {
                                            (icon(ico)) span.sr-only { (label) ": " (item.name) }
                                        }
                                    }
                                }
                            }
                            @if rights.remove {
                                a.button.icon-btn.danger href=(delete_url) hx-post=(delete_url)
                                    hx-confirm=(tf("Delete “{name}” from this set?", &[("name", &item.name)]))
                                    hx-target="closest .card" hx-swap="delete" title=(t("Delete")) {
                                    (icon(Icon::Delete)) span.sr-only { (tf("Delete {name}", &[("name", &item.name)])) }
                                }
                            }
                        }
                    }
                    // Opens the context menu; needs app.js, hidden without it.
                    button.icon-btn.card-menu-btn type="button" hidden data-menu-button="" aria-haspopup="menu"
                        aria-expanded="false" title=(t("More actions")) {
                        (icon(Icon::More)) span.sr-only { (tf("More actions for {name}", &[("name", &item.name)])) }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Item page
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct PageQuery {
    #[serde(default)]
    edit: Option<String>,
    #[serde(default)]
    saved: Option<String>,
}

fn preview(item: &Item) -> Markup {
    let src = file_url(&item.id, Variant::Original);
    html! {
        figure.preview {
            @if item.media.kind == MediaKind::Video {
                video src=(src) controls loop muted playsinline preload="metadata" aria-label=(item.name) {
                    a href=(src) { (t("Download the video")) }
                }
            } @else {
                img src=(src) alt=(item.name) width=(item.media.width) height=(item.media.height);
            }
        }
    }
}

fn full_form(ctx: &Ctx, vibes: &[String], item: &Item) -> Markup {
    html! {
        form #edit.stack method="post" action=(item_url(&item.id)) {
            (ctx.csrf_input())
            @for f in Field::ALL {
                div.field-row {
                    label for=(format!("all-{}", f.key())) {
                        (f.label())
                        @if f.source(item) == FieldSource::Ai && !f.is_empty(item) { " " (ai_mark()) }
                    }
                    @if f == Field::Vibe {
                        input id=(format!("all-{}", f.key())) type="text" name=(f.key()) value=(f.text(item)) list="vibe-list-all";
                        datalist #vibe-list-all { @for v in vibes { option value=(v) {} } }
                    } @else {
                        input id=(format!("all-{}", f.key())) type="text" name=(f.key()) value=(f.text(item))
                            data-emoji-picker=[(f == Field::Emojis).then_some("")];
                    }
                    @if !f.hint().is_empty() { p.hint { (f.hint()) } }
                }
            }
            p.hint { (t("Only fields you change are saved; AI suggestions you leave alone stay marked as AI.")) }
            div.inline-row {
                button.primary type="submit" { (t("Save")) }
                a.button href=(item_url(&item.id)) { (t("Cancel")) }
            }
        }
    }
}

fn quality_section(d: &ItemDetail) -> Markup {
    html! {
        section aria-labelledby="q-head" {
            h2 #q-head { (t("Quality")) " " (quality_badge(d.item.media.quality)) }
            @match &d.quality {
                None => p.muted { (t("No quality report yet.")) },
                Some(q) => {
                    table.data {
                        caption.sr-only { (t("Quality score components")) }
                        thead {
                            tr {
                                th scope="col" { (t("Component")) } th scope="col" { (t("Score")) }
                                th scope="col" { (t("Weight")) } th scope="col" { (t("Detail")) }
                            }
                        }
                        tbody {
                            @for c in &q.components {
                                tr {
                                    th scope="row" { (component_label(&c.name)) }
                                    td {
                                        meter class=(quality_class(c.score)) min="0" max="100" low="40" high="85" optimum="100" value=(c.score) {}
                                        " " (c.score)
                                    }
                                    td { (format!("{:.1}", c.weight)) }
                                    td { (crate::i18n::t_template(&c.detail, stickpile_media::quality::DETAILS)) }
                                }
                            }
                        }
                        tfoot { tr { th scope="row" { (t("Total")) } td { (q.score) } td {} td.muted { (t("weighted mean of the components")) } } }
                    }
                }
            }
        }
    }
}

/// A quality component's name (`jpeg_quality`) for people.
fn component_label(name: &str) -> String {
    let label = match name {
        "resolution" => t("resolution"),
        "jpeg_quality" => t("jpeg quality"),
        "recompression" => t("recompression"),
        "blockiness" => t("blockiness"),
        "ringing" => t("ringing"),
        "sharpness" => t("sharpness"),
        "alpha" => t("alpha"),
        "frame_rate" => t("frame rate"),
        "palette" => t("palette"),
        other => return other.replace('_', " "),
    };
    label.to_string()
}

fn variants_section(item: &Item) -> Markup {
    html! {
        section aria-labelledby="v-head" {
            h2 #v-head { (t("Files")) }
            table.data {
                thead {
                    tr {
                        th scope="col" { (t("Variant")) } th scope="col" { (t("Type")) } th scope="col" { (t("Size")) }
                        th scope="col" { (t("Pixels")) } th scope="col" { span.sr-only { (t("Download")) } }
                    }
                }
                tbody {
                    @for v in &item.media.variants {
                        tr {
                            th scope="row" { (variant_label(v.variant)) }
                            td { code { (v.mime) } }
                            td { (fmt_bytes(v.size_bytes)) }
                            td { (v.width) "×" (v.height) }
                            td { a href=(file_url(&item.id, v.variant)) download { (t("Download")) span.sr-only { " " (variant_label(v.variant)) } } }
                        }
                    }
                }
            }
            @if item.media.variants.len() <= 2 {
                p.muted.small { (t("More variants (Telegram sticker, Discord GIF, …) are generated in the background shortly after upload.")) }
            }
        }
    }
}

fn variant_label(v: Variant) -> &'static str {
    match v {
        Variant::Original => t("Original"),
        Variant::Thumb => t("Thumbnail"),
        Variant::Png => "PNG",
        Variant::TgSticker => t("Telegram sticker (WebP)"),
        Variant::TgVideoSticker => t("Telegram video sticker"),
        Variant::DiscordGif => "Discord GIF",
        Variant::Mp4 => "MP4",
    }
}

pub async fn page(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> AppResult<Markup> {
    let d = svc::detail(&state, &ctx.user, &id).await?;
    let access = ui::readable_item(&state, &ctx.user, &id).await?;
    let editable = access.can_edit();
    let removable = access.can_remove();
    let access_set = crate::service::sets::load_api_for(&state.db, &access.access).await?;
    let vibes = vibes(&state);
    let item = &d.item;
    let card = Card::from_item(item);
    let editing = editable && q.edit.is_some();
    let move_targets = if removable {
        ui::editable_sets(&state, &ctx.user)
            .await?
            .into_iter()
            .filter(|s| s.id != item.set_id)
            .collect()
    } else {
        Vec::new()
    };
    let ai_ready = state.ai_status() == AiStatus::Ready;
    let m = &item.media;
    let body = html! {
        p.crumbs {
            a href=(set_url(&d.set.id)) { "← " (d.set.name) }
            " " (tf("by {owner}", &[("owner", &d.set.owner_username)]))
        }
        div.item-layout {
            (preview(item))
            div.item-side {
                h1 {
                    (item.name)
                    @if item.name_source == FieldSource::Ai { " " (ai_mark()) }
                }
                p.badges { (kind_badge(m.kind, m.format)) " " (quality_badge(m.quality)) }
                @if q.saved.is_some() { (layout::alert("success", t("Saved."))) }
                (copy_controls(&card, true, false))
                div.inline-row.like-row {
                    (like_form(&ctx, &item.id, item.liked_by_me, item.likes, &ctx.here))
                    span.muted { (plural(item.copies, "copy", "copies")) }
                }
                p.meta {
                    (m.format.extension().to_uppercase()) " · " (m.width) "×" (m.height) " · " (fmt_bytes(m.size_bytes))
                    @if let Some(ms) = m.duration_ms {
                        " · " (tf("{seconds} s", &[("seconds", &format!("{:.1}", ms as f64 / 1000.0))]))
                    }
                    @if let Some(fps) = m.fps { " · " (tf("{fps} fps", &[("fps", &format!("{fps:.0}"))])) }
                    @if m.has_alpha { " · " (t("transparent")) }
                }
                @if editing {
                    h2 { (t("Edit")) }
                    (full_form(&ctx, &vibes, item))
                } @else {
                    dl.fields.wide {
                        @for f in Field::ALL { (field_block(&ctx, item, f, editable)) }
                    }
                }
                @if !editable && super::collab::can_propose(&access_set) {
                    p { a.button href=(format!("/items/{}/suggest", item.id)) { (t("Suggest changes…")) } }
                }
                @if editable || removable {
                    div.item-actions {
                        @if ai_ready && editable {
                            form.inline method="post" action=(format!("/items/{}/enrich", item.id))
                                hx-post=(format!("/items/{}/enrich", item.id)) hx-target="#job-slot" hx-swap="innerHTML" {
                                (ctx.csrf_input())
                                button type="submit"
                                    title=(t("Suggest keywords, emojis and vibe again (fields you wrote or confirmed are kept)")) {
                                    (t("Re-run AI"))
                                }
                            }
                        }
                        @if !move_targets.is_empty() {
                            form.inline-row method="post" action=(format!("/items/{}/move", item.id)) {
                                (ctx.csrf_input())
                                label for="move-to" { (t("Move to")) }
                                select #move-to name="set_id" {
                                    @for s in &move_targets { option value=(s.id) { (s.name) } }
                                }
                                button type="submit" { (t("Move")) }
                            }
                        }
                        @if removable {
                            a.button.danger href=(format!("/items/{}/delete", item.id)) { (t("Delete…")) }
                        }
                    }
                    div #job-slot {}
                }
                @if !d.incompatible.is_empty() {
                    section.incompatible aria-labelledby="inc-head" {
                        h2 #inc-head { (t("Cannot be sent to")) }
                        ul {
                            @for i in &d.incompatible {
                                li { strong { (target_label(i.target)) } ": " (crate::i18n::t_template(&i.reason, stickpile_api::profiles::INCOMPATIBLE)) }
                            }
                        }
                    }
                }
            }
        }
        (quality_section(&d))
        section aria-labelledby="dup-head" {
            h2 #dup-head { (t("Looks the same")) }
            @if d.near_duplicates.is_empty() {
                p.muted { (t("No near-duplicates in your library.")) }
            } @else {
                ul.dups {
                    @for n in &d.near_duplicates {
                        li {
                            a href=(item_url(&n.existing.id)) { (n.existing.name) }
                            " " (t("in")) " " a href=(set_url(&n.existing.set_id)) { (n.existing.set_name) }
                            " · "
                            @if n.same_content {
                                (t("same file"))
                            } @else {
                                (tf("distance {distance}", &[("distance", &n.distance)]))
                            }
                            " · " (quality_badge(n.existing.quality))
                        }
                    }
                }
            }
            div #better-slot {
                a.button href=(format!("/items/{}/better", item.id)) hx-get=(format!("/items/{}/better", item.id))
                    hx-target="#better-slot" hx-swap="innerHTML" { (t("Find a better version")) }
            }
        }
        (variants_section(item))
    };
    Ok(ctx.page(&item.name, Nav::Sets, body))
}

fn target_label(target: stickpile_api::TargetApp) -> &'static str {
    use stickpile_api::TargetApp as T;
    match target {
        T::Telegram => "Telegram",
        T::Discord => "Discord",
        T::Signal => "Signal",
        T::Whatsapp => "WhatsApp",
        T::Slack => "Slack",
        T::Generic => t("Other apps"),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct AllFieldsForm {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    keywords: Option<String>,
    #[serde(default)]
    emojis: Option<String>,
    #[serde(default)]
    vibe: Option<String>,
    #[serde(default)]
    tags: Option<String>,
}

/// The no-JS edit form: only fields whose text changed are sent, so AI
/// values left alone stay AI-sourced.
pub async fn update_all(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Form(f): Form<AllFieldsForm>,
) -> AppResult<Response> {
    let current = svc::detail(&state, &ctx.user, &id).await?.item;
    let mut u = UpdateItem::default();
    for (field, value) in [
        (Field::Name, &f.name),
        (Field::Keywords, &f.keywords),
        (Field::Emojis, &f.emojis),
        (Field::Vibe, &f.vibe),
        (Field::Tags, &f.tags),
    ] {
        let Some(v) = value else { continue };
        let patch = field.update(v);
        let changed = match field {
            Field::Name => patch.name.as_deref() != Some(current.name.as_str()),
            Field::Keywords => patch.keywords.as_ref() != Some(&current.keywords),
            Field::Emojis => patch.emojis.as_ref() != Some(&current.emojis),
            Field::Vibe => patch.vibe.as_ref() != Some(&current.vibe),
            Field::Tags => patch.tags.as_ref() != Some(&current.tags),
        };
        if changed {
            u.name = u.name.or(patch.name);
            u.keywords = u.keywords.or(patch.keywords);
            u.emojis = u.emojis.or(patch.emojis);
            u.vibe = u.vibe.or(patch.vibe);
            u.tags = u.tags.or(patch.tags);
        }
    }
    svc::update(&state, &ctx.user, &id, u).await?;
    Ok(ctx.redirect(&format!("/items/{id}?saved=1")))
}

/// The item and whether the viewer may edit it.
async fn item_for(state: &AppState, ctx: &Ctx, id: &str) -> AppResult<(Item, bool)> {
    let d = svc::detail(state, &ctx.user, id).await?;
    let access = ui::readable_item(state, &ctx.user, id).await?;
    Ok((d.item, access.can_edit()))
}

pub async fn field_view(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, field)): Path<(String, String)>,
) -> AppResult<Response> {
    let f = Field::parse(&field)?;
    let (item, editable) = item_for(&state, &ctx, &id).await?;
    Ok(ctx.done(field_block(&ctx, &item, f, editable), None, &item_url(&id)))
}

pub async fn field_edit(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, field)): Path<(String, String)>,
) -> AppResult<Response> {
    let f = Field::parse(&field)?;
    let (item, editable) = item_for(&state, &ctx, &id).await?;
    if !editable {
        return Err(AppError::forbidden("you cannot edit this item"));
    }
    if !ctx.htmx {
        return Ok(ctx.redirect(&format!("/items/{id}?edit=1#edit")));
    }
    Ok(field_form(&ctx, &vibes(&state), &item, f, &f.text(&item), None).into_response())
}

#[derive(Debug, Default, Deserialize)]
pub struct ValueForm {
    #[serde(default)]
    value: String,
    #[serde(default)]
    next: Option<String>,
}

pub async fn field_save(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, field)): Path<(String, String)>,
    Form(form): Form<ValueForm>,
) -> AppResult<Response> {
    let f = Field::parse(&field)?;
    match svc::update(&state, &ctx.user, &id, f.update(&form.value)).await {
        Ok(item) => Ok(ctx.done(field_block(&ctx, &item, f, true), form.next.as_deref(), &item_url(&id))),
        Err(e) if e.status == StatusCode::BAD_REQUEST && ctx.htmx => {
            let (item, _) = item_for(&state, &ctx, &id).await?;
            let markup = field_form(&ctx, &vibes(&state), &item, f, &form.value, Some(&e.message));
            Ok((StatusCode::UNPROCESSABLE_ENTITY, markup).into_response())
        }
        Err(e) => Err(e),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct NextForm {
    #[serde(default)]
    next: Option<String>,
}

pub async fn confirm(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, field)): Path<(String, String)>,
    Form(form): Form<NextForm>,
) -> AppResult<Response> {
    let f = Field::parse(&field)?;
    if f == Field::Tags {
        return Err(AppError::bad_request("tags are never AI suggestions"));
    }
    let u = UpdateItem {
        confirm: vec![f.key().to_string()],
        ..Default::default()
    };
    let item = svc::update(&state, &ctx.user, &id, u).await?;
    Ok(ctx.done(field_block(&ctx, &item, f, true), form.next.as_deref(), &item_url(&id)))
}

#[derive(Debug, Default, Deserialize)]
pub struct LikeForm {
    #[serde(default)]
    liked: String,
    #[serde(default)]
    next: Option<String>,
}

pub async fn like(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Form(form): Form<LikeForm>,
) -> AppResult<Response> {
    let s = if form.liked == "0" {
        svc::unlike(&state, &ctx.user, &id).await?
    } else {
        svc::like(&state, &ctx.user, &id).await?
    };
    let next = form.next.clone().unwrap_or_else(|| item_url(&id));
    Ok(ctx.done(
        like_form(&ctx, &id, s.liked, s.likes, &next),
        form.next.as_deref(),
        &item_url(&id),
    ))
}

pub async fn delete_page(State(state): State<AppState>, ctx: Ctx, Path(id): Path<String>) -> AppResult<Markup> {
    let item = svc::detail(&state, &ctx.user, &id).await?.item;
    if !ui::readable_item(&state, &ctx.user, &id).await?.can_remove() {
        return Err(AppError::forbidden("you cannot delete this item"));
    }
    let body = html! {
        p.crumbs { a href=(item_url(&item.id)) { "← " (item.name) } }
        h1 { (tf("Delete “{name}”?", &[("name", &item.name)])) }
        p { (t("The item is removed from its set. Other sets that contain the same file keep their copy.")) }
        div.preview-small { (thumb(&item.id, &item.name, item.media.width, item.media.height)) }
        form.inline-row method="post" action=(format!("/items/{}/delete", item.id)) {
            (ctx.csrf_input())
            input type="hidden" name="next" value=(set_url(&item.set_id));
            button.danger type="submit" { (t("Delete item")) }
            a.button href=(item_url(&item.id)) { (t("Cancel")) }
        }
    };
    Ok(ctx.page(t("Delete item"), Nav::Sets, body))
}

pub async fn delete_submit(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Form(form): Form<NextForm>,
) -> AppResult<Response> {
    let set_id = svc::detail(&state, &ctx.user, &id).await?.item.set_id;
    svc::delete(&state, &ctx.user, &id).await?;
    Ok(ctx.done(html! {}, form.next.as_deref(), &set_url(&set_id)))
}

pub async fn enrich(State(state): State<AppState>, ctx: Ctx, Path(id): Path<String>) -> AppResult<Response> {
    let job = svc::enrich(&state, &ctx.user, &id).await?;
    if ctx.htmx {
        Ok(integrations::job_fragment(&job, Some(&item_url(&id))).into_response())
    } else {
        Ok(ctx.redirect(&format!("/jobs/{}?back={}", job.id, item_url(&id))))
    }
}

pub async fn better(State(state): State<AppState>, ctx: Ctx, Path(id): Path<String>) -> AppResult<Markup> {
    let list = svc::better(&state, &ctx.user, &id).await?;
    let fragment = html! {
        div.better {
            h3 { (t("Better versions")) }
            @if list.is_empty() {
                p.muted { (t("No higher-quality copy of this picture in your library or in public sets.")) }
            } @else {
                (grid(html! {
                    @for s in &list {
                        (item_card(&ctx, &Card::from_summary(s), Some(html! { (tf("by {owner}", &[("owner", &s.owner_username)])) })))
                    }
                }))
            }
        }
    };
    if ctx.htmx {
        Ok(fragment)
    } else {
        Ok(ctx.page(
            t("Better versions"),
            Nav::Sets,
            html! { p.crumbs { a href=(item_url(&id)) { "← " (t("back to the item")) } } (fragment) },
        ))
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct MoveForm {
    #[serde(default)]
    set_id: String,
}

pub async fn move_to_set(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Form(form): Form<MoveForm>,
) -> AppResult<Response> {
    let u = UpdateItem {
        set_id: Some(form.set_id.clone()),
        ..Default::default()
    };
    svc::update(&state, &ctx.user, &id, u).await?;
    Ok(ctx.redirect(&format!("/items/{id}?saved=1")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_split() {
        assert_eq!(
            split_list(" cat, big chungus ,,meme"),
            vec!["cat", "big chungus", "meme"]
        );
        assert_eq!(split_emojis("😂 🤣,👍"), vec!["😂", "🤣", "👍"]);
        let u = Field::Vibe.update("  ");
        assert_eq!(u.vibe, Some(None));
        let u = Field::Vibe.update("tired");
        assert_eq!(u.vibe, Some(Some("tired".into())));
    }
}
