//! Search page: the same hybrid search as `GET /api/v1/search`, a syntax
//! cheat-sheet and a semantic-search indicator. With htmx the results update
//! while typing and "More results" appends the next page.

use axum::extract::{Query, State};
use maud::{html, Markup};
use serde::Deserialize;

use super::components::{item_card, Card};
use super::layout::Nav;
use super::sets::rich;
use super::Ctx;
use crate::error::{AppError, AppResult};
use crate::i18n::{t, tc};
use crate::service::search::{self, SearchParams};
use crate::state::{AiStatus, AppState};

const PAGE: usize = 48;

#[derive(Debug, Default, Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    offset: Option<usize>,
    /// Set by the "More results" button: answer with cards only.
    #[serde(default)]
    more: Option<String>,
}

fn more_url(q: &str, offset: usize) -> String {
    format!(
        "/search?q={}&offset={offset}",
        form_urlencoded::byte_serialize(q.as_bytes()).collect::<String>()
    )
}

pub fn cheat_sheet() -> Markup {
    let rows: [(&str, &str); 12] = [
        (
            "cat \"big chungus\"",
            t("words and phrases in name, keywords, emojis, tags, vibe and set (typos and naahh → nah are forgiven)"),
        ),
        ("-word", t("exclude a word")),
        ("e:😂", t("has this emoji")),
        ("set:cats", t("in a set (slug or name)")),
        ("vibe:tired", t("AI or user vibe label")),
        ("tag:reaction  -tag:nsfw", t("has / has not this tag")),
        (
            "type:gif",
            t("still, animated, video, or a format: png jpeg webp gif mp4 webm"),
        ),
        ("by:alice", t("owner")),
        ("is:mine  is:liked  is:public", t("your items, your likes, public sets")),
        ("q>70", t("quality score (also >= < <= =)")),
        ("likes>=2  copies<5", t("like and copy counts")),
        ("sort:likes", t("relevance (default), recent, likes, copies, quality")),
    ];
    html! {
        details.cheatsheet {
            summary { (t("Search syntax")) }
            table.data {
                caption.sr-only { (t("Search syntax")) }
                thead { tr { th scope="col" { (tc("instruction", "Type")) } th scope="col" { (t("Finds")) } } }
                tbody {
                    @for (syntax, meaning) in rows {
                        tr { td { code { (syntax) } } td { (meaning) } }
                    }
                }
            }
            p.hint {
                (rich(
                    t("Repeating a filter means OR ({example}); different filters must all match."),
                    &[("example", html! { code { "set:cats set:dogs" } })],
                ))
                " " (t("Example:")) " "
                code { "e:😂 set:cats vibe:tired q>70 type:gif tag:reaction -tag:x likes>=2 sort:likes" }
            }
        }
    }
}

pub async fn page(State(state): State<AppState>, ctx: Ctx, Query(q): Query<SearchQuery>) -> AppResult<Markup> {
    if q.q.len() > 1000 {
        return Err(AppError::bad_request("the query is too long (at most 1000 bytes)"));
    }
    let query = q.q.trim().to_string();
    let offset = q.offset.unwrap_or(0).min(100_000);
    let results = if query.is_empty() {
        None
    } else {
        Some(
            search::search(
                &state,
                &ctx.user,
                SearchParams {
                    q: query.clone(),
                    limit: Some(PAGE),
                    offset: Some(offset),
                    target: None,
                },
            )
            .await?,
        )
    };
    let cards = |r: &stickpile_api::SearchResults| {
        let next = offset + r.hits.len();
        html! {
            @for h in &r.hits {
                (item_card(&ctx, &Card::from_summary(&h.item), Some(html! {
                    @if !h.matched.is_empty() { span.matched title=(t("Why this matched")) { (h.matched.join(", ")) } }
                })))
            }
            @if (next as u64) < r.total && !r.hits.is_empty() {
                div.more-cell {
                    a.button href=(more_url(&query, next)) hx-get=(format!("{}&more=1", more_url(&query, next)))
                        hx-target="closest .more-cell" hx-swap="outerHTML" { (t("More results")) }
                }
            }
        }
    };
    if ctx.htmx && q.more.is_some() {
        return Ok(results.as_ref().map(cards).unwrap_or_else(|| html! {}));
    }
    let semantic_note = match state.ai_status() {
        AiStatus::Ready => t("Semantic search is on: results also match image content and related words."),
        AiStatus::Loading => t("Semantic search is still loading; these results match metadata only."),
        _ => t("Semantic search is off on this server; results match metadata only."),
    };
    let results_block = html! {
        div #results aria-live="polite" {
            @if let Some(r) = &results {
                p.result-head {
                    (rich(
                        t("{n} results for {query}"),
                        &[("n", html! { strong { (r.total) } }), ("query", html! { code { (r.query) } })],
                    ))
                    " "
                    @if r.semantic {
                        span.badge.semantic title=(semantic_note) { (t("semantic")) }
                    } @else {
                        span.badge.lexical title=(semantic_note) { (t("metadata only")) }
                    }
                }
                @if r.hits.is_empty() {
                    p.muted { (t("Nothing found. Try fewer words or remove a filter.")) }
                } @else {
                    div.grid { (cards(r)) }
                }
            }
        }
    };
    if ctx.htmx {
        return Ok(results_block);
    }
    let body = html! {
        h1 { (t("Search")) }
        form.bigsearch role="search" method="get" action="/search" hx-get="/search" hx-target="#results"
            hx-swap="outerHTML" hx-push-url="true" hx-trigger="submit, input changed delay:400ms from:#search-q" {
            label for="search-q" { (t("Search your items and public sets")) }
            div.inline-row {
                input #search-q type="search" name="q" value=(query) autofocus autocomplete="off"
                    placeholder="cat e:😂 type:gif q>70";
                button.primary type="submit" { (t("Search")) }
            }
        }
        p.muted.small { (semantic_note) }
        (cheat_sheet())
        (results_block)
    };
    Ok(ctx.page(if query.is_empty() { t("Search") } else { &query }, Nav::Search, body))
}
