//! The row of numbered tabs at the top of the popup, and the help button.

use gtk::prelude::*;
use stickpile_quick::Tab;

use crate::ui::set_class;

pub struct TabBar {
    pub root: gtk::Box,
    buttons: Vec<(Tab, gtk::Button)>,
    pub help: gtk::Button,
}

impl TabBar {
    pub fn build() -> TabBar {
        let root = gtk::Box::builder().spacing(2).build();
        root.add_css_class("stickpile-tabs");
        let buttons: Vec<(Tab, gtk::Button)> = Tab::ALL
            .into_iter()
            .map(|tab| {
                let number = gtk::Label::new(Some(&tab.number().to_string()));
                number.add_css_class("stickpile-tab-number");
                let label = gtk::Label::new(Some(tab.label()));
                let content = gtk::Box::builder().spacing(5).build();
                content.append(&number);
                content.append(&label);
                // Like the grid, the tabs never take the keyboard focus:
                // the text field keeps it, and the keys reach the tabs anyway.
                let b = gtk::Button::builder()
                    .child(&content)
                    .can_focus(false)
                    .tooltip_text(format!("{} (Alt+{})", tab.label(), tab.number()))
                    .build();
                b.add_css_class("flat");
                b.add_css_class("stickpile-tab");
                b.update_property(&[
                    gtk::accessible::Property::Label(tab.label()),
                    gtk::accessible::Property::KeyShortcuts(&format!("Alt+{}", tab.number())),
                ]);
                root.append(&b);
                (tab, b)
            })
            .collect();
        let spacer = gtk::Box::builder().hexpand(true).build();
        root.append(&spacer);
        let help = gtk::Button::builder()
            .label("?")
            .can_focus(false)
            .tooltip_text("Keys (? or F1)")
            .build();
        help.add_css_class("flat");
        help.add_css_class("stickpile-tab");
        help.update_property(&[
            gtk::accessible::Property::Label("Keys"),
            gtk::accessible::Property::KeyShortcuts("F1"),
        ]);
        root.append(&help);
        TabBar { root, buttons, help }
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
