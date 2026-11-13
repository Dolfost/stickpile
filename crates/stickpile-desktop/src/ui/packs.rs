//! Pack list cells: a GObject per set (cover, name, item count) and the
//! cell factory of the pack grid.

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use stickpile_api::SetSummary;

use crate::pages;

mod imp {
    use std::cell::{Cell, RefCell};

    use gtk::glib::Properties;

    use super::*;

    #[derive(Default, Properties)]
    #[properties(wrapper_type = super::PackObject)]
    pub struct PackObject {
        #[property(get, set)]
        pub set_id: RefCell<String>,
        #[property(get, set)]
        pub name: RefCell<String>,
        #[property(get, set)]
        pub count: RefCell<String>,
        #[property(get, set)]
        pub tooltip: RefCell<String>,
        #[property(get, set, nullable)]
        pub texture: RefCell<Option<gdk::Texture>>,
        /// No cover to show (yet): the placeholder icon is drawn.
        #[property(get, set)]
        pub placeholder: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PackObject {
        const NAME: &'static str = "StickpilePack";
        type Type = super::PackObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for PackObject {}
}

glib::wrapper! {
    pub struct PackObject(ObjectSubclass<imp::PackObject>);
}

impl PackObject {
    pub fn new(set: &SetSummary, texture: Option<&gdk::Texture>) -> PackObject {
        let mut tooltip = set.name.clone();
        if !set.description.is_empty() {
            tooltip.push('\n');
            tooltip.push_str(&set.description);
        }
        tooltip.push_str(&format!("\nby {}", set.owner_username));
        glib::Object::builder()
            .property("set-id", &set.id)
            .property("name", &set.name)
            .property("count", pages::item_count(set.item_count))
            .property("tooltip", tooltip)
            .property("texture", texture)
            .property("placeholder", texture.is_none())
            .build()
    }

    pub fn show_cover(&self, texture: &gdk::Texture) {
        self.set_texture(Some(texture));
        self.set_placeholder(false);
    }
}

/// Cells: cover (or a placeholder icon), the name and the item count.
pub fn factory(thumb_px: i32) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, obj| {
        let Some(li) = obj.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .width_request(thumb_px)
            .height_request(thumb_px)
            .build();
        let icon = gtk::Image::builder()
            .icon_name("folder-pictures-symbolic")
            .pixel_size((thumb_px / 3).max(16))
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        icon.add_css_class("dim-label");
        let cover = gtk::Overlay::new();
        cover.set_child(Some(&picture));
        cover.add_overlay(&icon);
        let name = gtk::Label::builder()
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(1)
            .hexpand(true)
            .build();
        name.add_css_class("stickpile-pack-name");
        let count = gtk::Label::new(None);
        count.add_css_class("caption");
        count.add_css_class("dim-label");
        let cell = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .build();
        cell.append(&cover);
        cell.append(&name);
        cell.append(&count);
        li.set_child(Some(&cell));

        let item = li.property_expression("item");
        item.chain_property::<PackObject>("texture")
            .bind(&picture, "paintable", gtk::Widget::NONE);
        item.chain_property::<PackObject>("placeholder")
            .bind(&icon, "visible", gtk::Widget::NONE);
        item.chain_property::<PackObject>("name")
            .bind(&name, "label", gtk::Widget::NONE);
        item.chain_property::<PackObject>("count")
            .bind(&count, "label", gtk::Widget::NONE);
        item.chain_property::<PackObject>("tooltip")
            .bind(&cell, "tooltip-text", gtk::Widget::NONE);
    });
    factory
}
