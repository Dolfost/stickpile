//! The row of numbered tabs at the top of the popup, and the help button.

use gtk::prelude::*;
use stickpile_i18n::global::t;
use stickpile_quick::Tab;

use crate::ui::set_class;

pub struct TabBar {
    pub root: gtk::Box,
    buttons: Vec<(Tab, gtk::Button)>,
    labels: Vec<gtk::Label>,
    pub help: gtk::Button,
}

impl TabBar {
    pub fn build() -> TabBar {
        let root = gtk::Box::builder().spacing(2).build();
        root.add_css_class("stickpile-tabs");
        let mut labels = Vec::new();
        let buttons: Vec<(Tab, gtk::Button)> = Tab::ALL
            .into_iter()
            .map(|tab| {
                let number = gtk::Label::new(Some(&tab.number().to_string()));
                number.add_css_class("stickpile-tab-number");
                let label = gtk::Label::new(None);
                let content = gtk::Box::builder().spacing(5).build();
                content.append(&number);
                content.append(&label);
                // Like the grid, the tabs never take the keyboard focus:
                // the text field keeps it, and the keys reach the tabs anyway.
                let b = gtk::Button::builder().child(&content).can_focus(false).build();
                b.add_css_class("flat");
                b.add_css_class("stickpile-tab");
                b.update_property(&[gtk::accessible::Property::KeyShortcuts(&format!(
                    "Alt+{}",
                    tab.number()
                ))]);
                root.append(&b);
                labels.push(label);
                (tab, b)
            })
            .collect();
        let spacer = gtk::Box::builder().hexpand(true).build();
        root.append(&spacer);
        let help = gtk::Button::builder().label("?").can_focus(false).build();
        help.add_css_class("flat");
        help.add_css_class("stickpile-tab");
        help.update_property(&[gtk::accessible::Property::KeyShortcuts("F1")]);
        root.append(&help);
        let bar = TabBar {
            root,
            buttons,
            labels,
            help,
        };
        bar.relabel();
        bar
    }

    /// The texts, in the current language.
    pub fn relabel(&self) {
        for ((tab, b), label) in self.buttons.iter().zip(&self.labels) {
            label.set_label(tab.label());
            b.set_tooltip_text(Some(&format!("{} (Alt+{})", tab.label(), tab.number())));
            b.update_property(&[gtk::accessible::Property::Label(tab.label())]);
        }
        self.help.set_tooltip_text(Some(t("Keys (? or F1)")));
        self.help
            .update_property(&[gtk::accessible::Property::Label(t("Keys"))]);
    }

    pub fn connect(&self, f: impl Fn(Tab) + Clone + 'static) {
        for (tab, b) in &self.buttons {
            let (tab, f) = (*tab, f.clone());
            b.connect_clicked(move |_| f(tab));
        }
    }

    pub fn render(&self, current: Tab) {
        for (tab, b) in &self.buttons {
            set_class(b, "stickpile-current", *tab == current);
        }
    }
}
