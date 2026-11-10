//! Key press → popup event mapping (display-free).
//!
//! The search entry keeps keyboard focus at all times; the window intercepts
//! keys in the capture phase and only the ones mapped here reach the model.
//! Everything else (typing, Backspace, Ctrl+A, Ctrl+Left/Right word jumps,
//! Shift+Home/End selection) goes to the entry.

use stickpile_quick::model::{Event, Nav};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Super/Logo: always left to the compositor.
    pub logo: bool,
}

/// One key press as the shell sees it.
#[derive(Debug, Clone, Copy)]
pub struct KeyPress<'a> {
    /// Keyval name in the active layout (`gdk::Key::name`): `Return`, `Up`, `t`, `Cyrillic_ie`.
    pub name: &'a str,
    /// Name of the same physical key in the first layout group, so Ctrl+T
    /// works with a Ukrainian or Russian layout active.
    pub latin: Option<&'a str>,
    pub mods: Mods,
}

fn is_latin_letter(name: &str) -> bool {
    name.len() == 1 && name.as_bytes()[0].is_ascii_alphabetic()
}

pub fn map_key(k: &KeyPress<'_>) -> Option<Event> {
    let m = k.mods;
    if m.alt || m.logo {
        return None;
    }
    let nav = |n| Some(Event::Navigate(n));
    match k.name {
        "Up" | "KP_Up" => return nav(Nav::Up),
        "Down" | "KP_Down" => return nav(Nav::Down),
        "Page_Up" | "Prior" | "KP_Page_Up" | "KP_Prior" => return nav(Nav::PageUp),
        "Page_Down" | "Next" | "KP_Page_Down" | "KP_Next" => return nav(Nav::PageDown),
        // Ctrl/Shift+Left/Right edit the query (word jump, selection).
        "Left" | "KP_Left" if !m.ctrl && !m.shift => return nav(Nav::Left),
        "Right" | "KP_Right" if !m.ctrl && !m.shift => return nav(Nav::Right),
        // Shift+Home/End select text in the query.
        "Home" | "KP_Home" if !m.shift => return nav(Nav::Home),
        "End" | "KP_End" if !m.shift => return nav(Nav::End),
        "Return" | "KP_Enter" | "ISO_Enter" => {
            return Some(Event::CopySelected {
                toggle_sticker: m.shift,
            })
        }
        "Escape" => return Some(Event::Escape),
        _ => {}
    }
    if m.ctrl {
        let base = if is_latin_letter(k.name) {
            k.name
        } else {
            k.latin.filter(|l| is_latin_letter(l))?
        };
        return match base.to_ascii_lowercase().as_str() {
            "t" => Some(Event::CycleTarget { backwards: m.shift }),
            "s" if !m.shift => Some(Event::ToggleSticker),
            _ => None,
        };
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str, mods: Mods) -> Option<Event> {
        map_key(&KeyPress {
            name,
            latin: None,
            mods,
        })
    }
    const NONE: Mods = Mods {
        ctrl: false,
        shift: false,
        alt: false,
        logo: false,
    };
    const CTRL: Mods = Mods { ctrl: true, ..NONE };
    const SHIFT: Mods = Mods { shift: true, ..NONE };
    const CTRL_SHIFT: Mods = Mods {
        ctrl: true,
        shift: true,
        ..NONE
    };

    #[test]
    fn arrows_and_paging_navigate() {
        assert_eq!(key("Up", NONE), Some(Event::Navigate(Nav::Up)));
        assert_eq!(key("KP_Down", NONE), Some(Event::Navigate(Nav::Down)));
        assert_eq!(key("Left", NONE), Some(Event::Navigate(Nav::Left)));
        assert_eq!(key("Right", NONE), Some(Event::Navigate(Nav::Right)));
        assert_eq!(key("Home", NONE), Some(Event::Navigate(Nav::Home)));
        assert_eq!(key("End", CTRL), Some(Event::Navigate(Nav::End)));
        assert_eq!(key("Page_Up", NONE), Some(Event::Navigate(Nav::PageUp)));
        assert_eq!(key("Next", NONE), Some(Event::Navigate(Nav::PageDown)));
        assert_eq!(key("Prior", NONE), Some(Event::Navigate(Nav::PageUp)));
        assert_eq!(key("Up", SHIFT), Some(Event::Navigate(Nav::Up)));
    }

    #[test]
    fn text_editing_keys_pass_through() {
        assert_eq!(key("Left", CTRL), None);
        assert_eq!(key("Right", SHIFT), None);
        assert_eq!(key("Home", SHIFT), None);
        assert_eq!(key("End", SHIFT), None);
        assert_eq!(key("a", NONE), None);
        assert_eq!(key("a", CTRL), None);
        assert_eq!(key("BackSpace", NONE), None);
        assert_eq!(key("Tab", NONE), None);
    }

    #[test]
    fn enter_copies_shift_toggles_sticker() {
        assert_eq!(key("Return", NONE), Some(Event::CopySelected { toggle_sticker: false }));
        assert_eq!(
            key("KP_Enter", SHIFT),
            Some(Event::CopySelected { toggle_sticker: true })
        );
        assert_eq!(
            key("ISO_Enter", NONE),
            Some(Event::CopySelected { toggle_sticker: false })
        );
    }

    #[test]
    fn escape_hides() {
        assert_eq!(key("Escape", NONE), Some(Event::Escape));
    }

    #[test]
    fn ctrl_shortcuts() {
        assert_eq!(key("t", CTRL), Some(Event::CycleTarget { backwards: false }));
        assert_eq!(key("T", CTRL_SHIFT), Some(Event::CycleTarget { backwards: true }));
        assert_eq!(key("s", CTRL), Some(Event::ToggleSticker));
        assert_eq!(key("S", CTRL_SHIFT), None);
        assert_eq!(key("t", NONE), None, "plain t is typed into the query");
    }

    #[test]
    fn ctrl_shortcuts_work_in_non_latin_layouts() {
        let ua = |name, latin, mods| {
            map_key(&KeyPress {
                name,
                latin: Some(latin),
                mods,
            })
        };
        assert_eq!(
            ua("Cyrillic_ie", "t", CTRL),
            Some(Event::CycleTarget { backwards: false })
        );
        assert_eq!(ua("Cyrillic_i", "s", CTRL), Some(Event::ToggleSticker));
        assert_eq!(ua("Cyrillic_ie", "Cyrillic_ie", CTRL), None);
        assert_eq!(ua("Cyrillic_ie", "t", NONE), None);
    }

    #[test]
    fn alt_and_super_are_left_alone() {
        assert_eq!(key("Return", Mods { alt: true, ..NONE }), None);
        assert_eq!(
            key(
                "t",
                Mods {
                    logo: true,
                    ctrl: true,
                    ..NONE
                }
            ),
            None
        );
    }
}
