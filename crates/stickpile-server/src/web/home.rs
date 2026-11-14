//! Start page: search box, the caller's sets, recently copied items.

use axum::extract::State;
use maud::{html, Markup};

use super::components::{set_list, set_list_view, view_menu};
use super::layout::Nav;
use super::{auth, items, sets as set_pages, MaybeCtx};
use crate::assets;
use crate::error::AppResult;
use crate::i18n::{t, tf};
use crate::service::sets::{self, SetScope};
use crate::service::ui;
use crate::state::AppState;

// ---- display options (begin) ----------------------------------------------

/// Show/hide toggles of the recently copied items (`hide-{key}` on their
/// section): the set page's, plus the item's set. The name always shows.
fn recent_parts() -> [(&'static str, &'static str, &'static str); 6] {
    [
        ("set", t("Set"), ""),
        ("emojis", t("Emojis"), ""),
        ("keywords", t("Keywords"), ""),
        ("vibe", t("Vibe"), ""),
        ("tags", t("Tags"), ""),
        ("labels", t("Labels"), t("kind, quality")),
    ]
}

/// The "View" control of the recently copied items: the set page's styles
/// and toggles, remembered apart from them (`stickpile.home.*`). Not in the
/// URL, so it needs JavaScript and is hidden without.
fn recent_view_menu() -> Markup {
    view_menu(
        "home",
        "home-view",
        &set_pages::item_styles(),
        "regular",
        &recent_parts(),
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
            h1 { (tf("Hi, {name}", &[("name", &ctx.user.user.display_name)])) }
            form.bigsearch role="search" method="get" action="/search" {
                label for="home-q" { (t("Search your library and public sets")) }
                div.inline-row {
                    input #home-q type="search" name="q" placeholder="cat, e:😂, vibe:tired, type:gif …" autofocus
                        autocomplete="off";
                    button.primary type="submit" { (t("Search")) }
                }
            }
        }
        // The set page's cards (read-only here, with their set), styled by
        // the same classes; the script (first child) applies the remembered
        // display options before the cards are parsed.
        section #recent-section .items-section data-view-prefs="home" aria-labelledby="recent-head" {
            script src=(assets::url("prefs.js")) {}
            div.section-head {
                h2 #recent-head { (t("Recently copied")) }
                @if !recent.is_empty() { (recent_view_menu()) }
            }
            @if recent.is_empty() {
                p.muted { (t("Nothing copied yet. Copy an item and it shows up here.")) }
            } @else {
                div.grid.editor-grid {
                    @for (item, set_name) in &recent {
                        (items::editor_card_in(&ctx, item, Some((&item.set_id, set_name)), items::Rights::default(), None, false))
                    }
                }
            }
        }
        section {
            div.section-head {
                h2 { (t("Your sets")) }
                a.button href="/sets/new" { (t("New set")) }
                a.button href="/upload" { (t("Upload")) }
                @if !my_sets.is_empty() { (set_list_view()) }
            }
            @if my_sets.is_empty() {
                p.muted {
                    (t("You have no sets yet.")) " "
                    (set_pages::rich(
                        t("{create}, {import}, or browse {public}."),
                        &[
                            ("create", html! { a href="/sets/new" { (t("Create one")) } }),
                            ("import", html! { a href="/upload" { (t("import a set directory")) } }),
                            ("public", html! { a href="/sets?tab=public" { (t("public sets")) } }),
                        ],
                    ))
                }
            } @else {
                (set_list(&my_sets))
            }
        }
    };
    Ok(ctx.page(t("Home"), Nav::Home, body))
}
