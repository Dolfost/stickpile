//! GTK4/libadwaita layer: a thin renderer of the `stickpile-quick` model.
//! Needs a display; everything testable lives outside this module.
//!
//! * [`shell`]: the window, rendering and command execution
//! * [`tabbar`], [`help`]: the tab row and the key help overlay
//! * [`item`], [`packs`]: grid cells of items and of the pack list
//! * [`me`], [`settings`]: the Me and Settings pages, with [`nav`], their
//!   Normal-mode cursor
//! * [`drag`]: drag source and clipboard providers

use gtk::prelude::*;

pub mod drag;
pub mod help;
pub mod item;
pub mod me;
pub mod nav;
pub mod packs;
pub mod settings;
pub mod shell;
pub mod tabbar;

/// A page's callback into the shell.
pub(crate) type Handler<A> = std::rc::Rc<dyn Fn(A)>;

pub(crate) fn set_class(widget: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if on {
        widget.add_css_class(class);
    } else {
        widget.remove_css_class(class);
    }
}
