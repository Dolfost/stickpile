//! Start page: search box, the caller's sets, recently copied items.

use axum::extract::State;
use maud::{html, Markup};

use super::components::{set_list, set_list_view, view_menu};
use super::layout::Nav;
use super::{auth, items, sets as set_pages, MaybeCtx};
use crate::assets;
use crate::error::AppResult;
use crate::service::sets::{self, SetScope};
use crate::service::ui;
use crate::state::AppState;

// ---- display options (begin) ----------------------------------------------

/// Show/hide toggles of the recently copied items (`hide-{key}` on their
/// section): the set page's, plus the item's set. The name always shows.
const RECENT_PARTS: [(&str, &str, &str); 6] = [
    ("set", "Set", ""),
    ("emojis", "Emojis", ""),
    ("keywords", "Keywords", ""),
    ("vibe", "Vibe", ""),
    ("tags", "Tags", ""),
    ("labels", "Labels", "kind, quality"),
];

/// The "View" control of the recently copied items: the set page's styles
/// and toggles, remembered apart from them (`stickpile.home.*`). Not in the
/// URL, so it needs JavaScript and is hidden without.
fn recent_view_menu() -> Markup {
    view_menu(
        "home",
        "home-view",
        &set_pages::item_styles(),
        "regular",
        &RECENT_PARTS,
        true,
    )
}

// ---- display options (end) ------------------------------------------------

/// The start page; anonymous visitors get the sign-in form here (every
/// other page redirects them to `/login`).
pub async fn page(State(state): State<AppState>, MaybeCtx(ctx): MaybeCtx) -> AppResult<Markup> {
    let Some(ctx) = ctx else {
        return Ok(auth::login_form("", "/", None));
    };
    let my_sets = sets::list(&state, &ctx.user, SetScope::Mine).await?;
    let recent = ui::recently_copied(&state, &ctx.user, 12).await?;
    let body = html! {
        section.hero {
            h1 { "Hi, " (ctx.user.user.display_name) }
            form.bigsearch role="search" method="get" action="/search" {
                label for="home-q" { "Search your library and public sets" }
                div.inline-row {
                    input #home-q type="search" name="q" placeholder="cat, e:😂, vibe:tired, type:gif …" autofocus
                        autocomplete="off";
                    button.primary type="submit" { "Search" }
                }
            }
        }
        // The set page's cards (read-only here, with their set), styled by
        // the same classes; the script (first child) applies the remembered
        // display options before the cards are parsed.
        section #recent-section .items-section data-view-prefs="home" aria-labelledby="recent-head" {
            script src=(assets::url("prefs.js")) {}
            div.section-head {
                h2 #recent-head { "Recently copied" }
                @if !recent.is_empty() { (recent_view_menu()) }
            }
            @if recent.is_empty() {
                p.muted { "Nothing copied yet. Copy an item and it shows up here." }
            } @else {
                div.grid.editor-grid {
                    @for (item, set_name) in &recent {
                        (items::editor_card_in(&ctx, item, Some((&item.set_id, set_name)), false, None, false))
                    }
                }
            }
        }
        section {
            div.section-head {
                h2 { "Your sets" }
                a.button href="/sets/new" { "New set" }
                a.button href="/upload" { "Upload" }
                @if !my_sets.is_empty() { (set_list_view()) }
            }
            @if my_sets.is_empty() {
                p.muted {
                    "You have no sets yet. "
                    a href="/sets/new" { "Create one" } ", "
                    a href="/upload" { "import a set directory" } ", or browse "
                    a href="/sets?tab=public" { "public sets" } "."
                }
            } @else {
                (set_list(&my_sets))
            }
        }
    };
    Ok(ctx.page("Home", Nav::Home, body))
}
