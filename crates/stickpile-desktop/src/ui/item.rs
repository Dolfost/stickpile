//! Grid item: a GObject wrapping one model entry. Cells bind to its
//! properties with expressions, so a thumbnail that arrives later just sets
//! `texture` and the visible cell updates.

use gtk::gdk;
use gtk::glib;
use gtk::subclass::prelude::*;
use stickpile_quick::model::{Entry, Tiles};

mod imp {
    use std::cell::{Cell, RefCell};

    use gtk::glib::prelude::*;
    use gtk::glib::Properties;

    use super::*;

    #[derive(Default, Properties)]
    #[properties(wrapper_type = super::StickerObject)]
    pub struct StickerObject {
        #[property(get, set)]
        pub item_id: RefCell<String>,
        #[property(get, set, nullable)]
        pub texture: RefCell<Option<gdk::Texture>>,
        #[property(get, set)]
        pub tooltip: RefCell<String>,
        #[property(get, set)]
        pub badge: RefCell<String>,
        #[property(get, set)]
        pub has_badge: Cell<bool>,
        #[property(get, set)]
        pub incompatible: Cell<bool>,
        /// Show the favorite mark.
        #[property(get, set)]
        pub liked: Cell<bool>,
        /// The emojis to show; empty: none.
        #[property(get, set)]
        pub emojis: RefCell<String>,
        #[property(get, set)]
        pub has_emojis: Cell<bool>,
        /// The tags to show; empty: none.
        #[property(get, set)]
        pub tags: RefCell<String>,
        #[property(get, set)]
        pub has_tags: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for StickerObject {
        const NAME: &'static str = "StickpileSticker";
        type Type = super::StickerObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for StickerObject {}
}

glib::wrapper! {
    pub struct StickerObject(ObjectSubclass<imp::StickerObject>);
}

impl StickerObject {
    /// `tiles`: what the tile shows besides the thumbnail.
    pub fn new(entry: &Entry, texture: Option<&gdk::Texture>, tiles: Tiles) -> StickerObject {
        let badge = entry.badge();
        let emojis = if tiles.emojis {
            entry.item.emojis.concat()
        } else {
            String::new()
        };
        let tags = if tiles.tags {
            entry.item.tags.join(", ")
        } else {
            String::new()
        };
        glib::Object::builder()
            .property("item-id", &entry.item.id)
            .property("texture", texture)
            .property("tooltip", entry.tooltip())
            .property("badge", badge.unwrap_or(""))
            .property("has-badge", badge.is_some())
            .property("incompatible", entry.incompatible.is_some())
            .property("liked", tiles.favorites && entry.liked)
            .property("has-emojis", !emojis.is_empty())
            .property("emojis", emojis)
            .property("has-tags", !tags.is_empty())
            .property("tags", tags)
            .build()
    }
}
