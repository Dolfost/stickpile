//! Start page: search box, the caller's sets, recently copied items.

use axum::extract::State;
use maud::{html, Markup};

use super::components::{grid, item_card, set_card, Card};
use super::layout::Nav;
use super::{auth, MaybeCtx};
use crate::error::AppResult;
use crate::service::sets::{self, SetScope};
use crate::service::ui;
use crate::state::AppState;

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
        section {
            div.section-head {
                h2 { "Recently copied" }
            }
            @if recent.is_empty() {
                p.muted { "Nothing copied yet. Copy an item and it shows up here." }
            } @else {
                (grid(html! { @for s in &recent { (item_card(&ctx, &Card::from_summary(s), None)) } }))
            }
        }
        section {
            div.section-head {
                h2 { "Your sets" }
                a.button href="/sets/new" { "New set" }
                a.button href="/upload" { "Upload" }
            }
            @if my_sets.is_empty() {
                p.muted {
                    "You have no sets yet. "
                    a href="/sets/new" { "Create one" } ", "
                    a href="/upload" { "import a set directory" } ", or browse "
                    a href="/sets?tab=public" { "public sets" } "."
                }
            } @else {
                (grid(html! { @for s in &my_sets { (set_card(s)) } }))
            }
        }
    };
    Ok(ctx.page("Home", Nav::Home, body))
}
