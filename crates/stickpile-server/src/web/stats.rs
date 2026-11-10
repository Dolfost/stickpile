//! Per-user statistics: totals, a bar chart of daily copies (inline SVG,
//! class-styled for the CSP, with a screen-reader table), top items and top
//! target apps.

use axum::extract::State;
use maud::{html, Markup};
use stickpile_api::{TargetApp, UserStats};

use super::components::{fmt_bytes, fmt_date, item_url, plural, set_url, thumb};
use super::layout::Nav;
use super::Ctx;
use crate::error::AppResult;
use crate::ids::now;
use crate::service::stats;
use crate::state::AppState;

const DAY: i64 = 86_400;

/// Round up to 1, 2 or 5 × 10^k (axis maximum).
pub fn nice_max(v: u64) -> u64 {
    if v <= 4 {
        return v.max(1);
    }
    let mut p = 1u64;
    while p * 10 <= v {
        p *= 10;
    }
    [1, 2, 5, 10]
        .into_iter()
        .map(|m| m * p)
        .find(|c| *c >= v)
        .unwrap_or(10 * p)
}

/// Bar with a 4 px rounded top, square at the baseline.
fn bar_path(x: f64, w: f64, top: f64, base: f64) -> String {
    let r = 4.0f64.min(w / 2.0).min(base - top);
    format!(
        "M{x:.1},{base:.1}V{:.1}Q{x:.1},{top:.1} {:.1},{top:.1}H{:.1}Q{:.1},{top:.1} {:.1},{:.1}V{base:.1}Z",
        top + r,
        x + r,
        x + w - r,
        x + w,
        x + w,
        top + r
    )
}

/// Daily copies chart. `first_day` is the unix day index of `daily[0]`.
pub fn daily_chart(daily: &[u64], first_day: i64) -> Markup {
    const W: f64 = 640.0;
    const H: f64 = 200.0;
    const LEFT: f64 = 36.0;
    const RIGHT: f64 = 8.0;
    const TOP: f64 = 10.0;
    const BOTTOM: f64 = 24.0;
    let n = daily.len().max(1);
    let peak = daily.iter().copied().max().unwrap_or(0);
    let max = nice_max(peak);
    let plot_h = H - TOP - BOTTOM;
    let base = TOP + plot_h;
    let slot = (W - LEFT - RIGHT) / n as f64;
    // Thin bars (about 24 px at full width) with a gap between them.
    let bar_w = (slot - 2.0).clamp(2.0, 12.0);
    let y = |v: u64| base - plot_h * v as f64 / max as f64;
    // Integer ticks only: every step for small maxima, else 0, half, max.
    let ticks: Vec<u64> = if max <= 4 {
        (0..=max).collect()
    } else if (max / 2) * 2 == max {
        vec![0, max / 2, max]
    } else {
        vec![0, max]
    };
    let total: u64 = daily.iter().sum();
    let busiest = daily
        .iter()
        .enumerate()
        .max_by_key(|(i, v)| (**v, std::cmp::Reverse(*i)))
        .filter(|(_, v)| **v > 0)
        .map(|(i, v)| (fmt_date((first_day + i as i64) * DAY), *v));
    let summary = match &busiest {
        Some((d, v)) => format!(
            "{} in the last {n} days; busiest day {d} with {}.",
            plural(total, "copy", "copies"),
            plural(*v, "copy", "copies")
        ),
        None => format!("No copies in the last {n} days."),
    };
    html! {
        figure.chart {
            figcaption { "Copies per day (UTC), last " (n) " days" }
            p.chart-summary { (summary) }
            svg.bar-chart viewBox=(format!("0 0 {W} {H}")) preserveAspectRatio="xMidYMid meet" aria-hidden="true"
                focusable="false" {
                @for t in ticks.iter().copied() {
                    line.gridline x1=(LEFT) x2=(W - RIGHT) y1=(format!("{:.1}", y(t))) y2=(format!("{:.1}", y(t))) {}
                    text.axis x=(LEFT - 6.0) y=(format!("{:.1}", y(t) + 4.0)) text-anchor="end" { (t) }
                }
                @for (i, v) in daily.iter().enumerate() {
                    @let x = LEFT + slot * i as f64 + (slot - bar_w) / 2.0;
                    @let date = fmt_date((first_day + i as i64) * DAY);
                    g.bar {
                        title { (date) ": " (plural(*v, "copy", "copies")) }
                        // Full-height hit area so small bars are easy to hover.
                        rect.hit x=(format!("{:.1}", LEFT + slot * i as f64)) y=(TOP) width=(format!("{slot:.1}"))
                            height=(format!("{plot_h:.1}")) {}
                        @if *v > 0 {
                            path.mark d=(bar_path(x, bar_w, y(*v), base)) {}
                        }
                    }
                }
                @for (i, anchor) in [(0usize, "start"), (n / 2, "middle"), (n - 1, "end")] {
                    text.axis x=(format!("{:.1}", LEFT + slot * i as f64 + slot / 2.0)) y=(H - 6.0) text-anchor=(anchor) {
                        (fmt_date((first_day + i as i64) * DAY))
                    }
                }
            }
            table.sr-only {
                caption { "Copies per day" }
                thead { tr { th scope="col" { "Day" } th scope="col" { "Copies" } } }
                tbody {
                    @for (i, v) in daily.iter().enumerate() {
                        tr { th scope="row" { (fmt_date((first_day + i as i64) * DAY)) } td { (v) } }
                    }
                }
            }
        }
    }
}

fn target_name(t: TargetApp) -> &'static str {
    match t {
        TargetApp::Telegram => "Telegram",
        TargetApp::Discord => "Discord",
        TargetApp::Signal => "Signal",
        TargetApp::Whatsapp => "WhatsApp",
        TargetApp::Slack => "Slack",
        TargetApp::Generic => "Other apps",
    }
}

fn tiles(s: &UserStats) -> Markup {
    let tiles: [(&str, String); 7] = [
        ("Sets", s.sets.to_string()),
        ("Items", s.items.to_string()),
        ("Storage", fmt_bytes(s.storage_bytes)),
        ("Copies", s.copies_total.to_string()),
        ("Copies, last 7 days", s.copies_last_7d.to_string()),
        ("Likes given", s.likes_given.to_string()),
        ("Likes received", s.likes_received.to_string()),
    ];
    html! {
        dl.tiles {
            @for (label, value) in tiles {
                div.tile { dt { (label) } dd { (value) } }
            }
        }
    }
}

pub async fn page(State(state): State<AppState>, ctx: Ctx) -> AppResult<Markup> {
    let s = stats::user_stats(&state, &ctx.user).await?;
    let first_day = now().div_euclid(DAY) - (s.daily_copies.len() as i64 - 1);
    let body = html! {
        h1 { "Your stats" }
        (tiles(&s))
        (daily_chart(&s.daily_copies, first_day))
        div.two-col {
            section aria-labelledby="top-items" {
                h2 #top-items { "Your most copied items" }
                @if s.top_items.is_empty() {
                    p.muted { "Nothing copied yet." }
                } @else {
                    ol.pop-list {
                        @for e in &s.top_items {
                            li {
                                a.mini-thumb href=(item_url(&e.item.id)) tabindex="-1" aria-hidden="true" {
                                    (thumb(&e.item.id, &e.item.name, e.item.width, e.item.height))
                                }
                                div {
                                    a href=(item_url(&e.item.id)) { (e.item.name) }
                                    p.meta { strong { (plural(e.value, "copy", "copies")) } " · " a href=(set_url(&e.item.set_id)) { (e.item.set_name) } }
                                }
                            }
                        }
                    }
                }
            }
            section aria-labelledby="top-targets" {
                h2 #top-targets { "Where you paste" }
                @if s.top_targets.is_empty() {
                    p.muted { "No target apps reported yet (the desktop client reports them)." }
                } @else {
                    table.data {
                        thead { tr { th scope="col" { "App" } th scope="col" { "Copies" } } }
                        tbody {
                            @for (t, n) in &s.top_targets {
                                tr { th scope="row" { (target_name(*t)) } td { (n) } }
                            }
                        }
                    }
                }
            }
        }
    };
    Ok(ctx.page("Stats", Nav::Stats, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_axis() {
        assert_eq!(nice_max(0), 1);
        assert_eq!(nice_max(3), 3);
        assert_eq!(nice_max(7), 10);
        assert_eq!(nice_max(11), 20);
        assert_eq!(nice_max(42), 50);
        assert_eq!(nice_max(100), 100);
        assert_eq!(nice_max(101), 200);
    }

    #[test]
    fn chart_has_no_inline_style() {
        let mut d = vec![0u64; 30];
        d[3] = 5;
        d[29] = 1;
        let html = daily_chart(&d, 20_000).into_string();
        assert!(!html.contains("style="));
        assert!(html.contains("<path"));
        assert!(html.contains("<table"));
    }
}
