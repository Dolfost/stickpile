//! The key help drawn over the popup (`?`, F1), from `stickpile_quick::HELP`.

use gtk::prelude::*;
use stickpile_i18n::global::{t, t_dyn};
use stickpile_quick::HELP;

use crate::pages;

pub fn build() -> gtk::Widget {
    let card = gtk::Box::builder()
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    card.add_css_class("stickpile-help");
    card.set_visible(false);
    fill(&card);
    card.upcast()
}

/// Draw the help again in the current language.
pub fn relabel(card: &gtk::Widget) {
    let Some(card) = card.downcast_ref::<gtk::Box>() else {
        return;
    };
    while let Some(child) = card.first_child() {
        card.remove(&child);
    }
    fill(card);
}

/// The key tables (from `stickpile_quick::HELP`, translated) into `card`.
fn fill(card: &gtk::Box) {
    let grid = gtk::Box::builder().spacing(24).build();
    for column in pages::help_columns(HELP) {
        let col = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(3)
            .build();
        for (n, section) in column.into_iter().enumerate() {
            let title = gtk::Label::builder()
                .label(t_dyn(section.title))
                .xalign(0.0)
                .margin_top(if n == 0 { 0 } else { 10 })
                .margin_bottom(2)
                .build();
            title.add_css_class("heading");
            col.append(&title);
            let keys = gtk::Grid::builder().column_spacing(14).row_spacing(2).build();
            for (row, (k, what)) in section.keys.iter().enumerate() {
                let key = gtk::Label::builder()
                    .label(t_dyn(k))
                    .xalign(0.0)
                    .yalign(0.0)
                    .wrap(true)
                    .width_chars(17)
                    .max_width_chars(17)
                    .build();
                key.add_css_class("stickpile-help-key");
                let what = gtk::Label::builder()
                    .label(t_dyn(what))
                    .xalign(0.0)
                    .yalign(0.0)
                    .wrap(true)
                    .width_chars(24)
                    .max_width_chars(24)
                    .build();
                what.add_css_class("dim-label");
                keys.attach(&key, 0, row as i32, 1, 1);
                keys.attach(&what, 1, row as i32, 1, 1);
            }
            col.append(&keys);
        }
        grid.append(&col);
    }
    let hint = gtk::Label::builder()
        .label(t("? or Esc closes this"))
        .xalign(1.0)
        .margin_top(10)
        .build();
    hint.add_css_class("dim-label");
    hint.add_css_class("caption");
    // The padding is inside the scrolled area: at its edge the first
    // headings were clipped by a pixel or two on some renderers.
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(14)
        .margin_bottom(14)
        .margin_start(20)
        .margin_end(20)
        .build();
    content.append(&grid);
    content.append(&hint);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .propagate_natural_width(true)
        .child(&content)
        .build();
    card.append(&scroller);
}
