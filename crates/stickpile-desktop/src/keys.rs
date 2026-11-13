//! Key press → popup event mapping (display-free).
//!
//! The window intercepts keys in the capture phase, before any text field;
//! whatever [`map_key`] maps is consumed and never reaches the field. What
//! is mapped depends on the model's [`Mode`]:
//!
//! * Insert, on Search and Packs (and on every grid with `modal` off): grid
//!   navigation, Enter, Escape and the Ctrl shortcuts; everything else
//!   (typing, Backspace, Ctrl+A, Ctrl+Left/Right word jumps, Shift+Home/End
//!   selection) goes to the text field.
//! * Insert on the Me / Settings pages: only Escape and the tab keys; the
//!   page's widgets get everything else.
//! * Normal: every key except bare modifiers and Super, as
//!   [`Event::Normal`]. Letters, digits and `-` `/` `?` `$` are read from
//!   the first (Latin) layout when the active one has no such key, so vim
//!   keys work with a Cyrillic layout active.
//!
//! Tab / Shift+Tab, Alt+1..7 and F1 work in every mode.

use stickpile_quick::model::{Event, Nav};
use stickpile_quick::{Mode, NormalKey, Tab};

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

/// Shift, Ctrl, Alt, Caps Lock, layout switches...: they only modify the
/// next key, so they must not cancel a pending `g` or count.
fn is_modifier(name: &str) -> bool {
    matches!(
        name,
        "Shift_L"
            | "Shift_R"
            | "Control_L"
            | "Control_R"
            | "Alt_L"
            | "Alt_R"
            | "Meta_L"
            | "Meta_R"
            | "Super_L"
            | "Super_R"
            | "Hyper_L"
            | "Hyper_R"
            | "Caps_Lock"
            | "Shift_Lock"
            | "Num_Lock"
            | "Scroll_Lock"
            | "Mode_switch"
    ) || (name.starts_with("ISO_") && ["Shift", "Lock", "Latch", "Group"].iter().any(|w| name.contains(w)))
}

/// The printable characters Normal mode uses, by keyval name.
fn char_of(name: &str) -> Option<char> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return c.is_ascii_alphanumeric().then_some(c);
    }
    match name {
        "slash" | "KP_Divide" => Some('/'),
        "question" => Some('?'),
        "minus" | "KP_Subtract" => Some('-'),
        "dollar" => Some('$'),
        _ => name
            .strip_prefix("KP_")
            .and_then(char_of)
            .filter(|c| c.is_ascii_digit()),
    }
}

/// The Normal-mode character of a key: from the active layout, else from
/// the same physical key in the first layout (with Shift applied as on a
/// US keyboard, since the first layout reports the unshifted key).
fn normal_char(k: &KeyPress<'_>) -> Option<char> {
    if let Some(c) = char_of(k.name) {
        return Some(c);
    }
    let base = char_of(k.latin?)?;
    if !k.mods.shift {
        return Some(base);
    }
    match base {
        c if c.is_ascii_lowercase() => Some(c.to_ascii_uppercase()),
        '/' => Some('?'),
        '4' => Some('$'),
        _ => None,
    }
}

/// Lowercase Latin letter of a Ctrl shortcut, through the first layout.
fn ctrl_letter(k: &KeyPress<'_>) -> Option<char> {
    let base = if is_latin_letter(k.name) {
        k.name
    } else {
        k.latin.filter(|l| is_latin_letter(l))?
    };
    base.chars().next().map(|c| c.to_ascii_lowercase())
}

fn nav_of(name: &str) -> Option<Nav> {
    Some(match name {
        "Up" | "KP_Up" => Nav::Up,
        "Down" | "KP_Down" => Nav::Down,
        "Left" | "KP_Left" => Nav::Left,
        "Right" | "KP_Right" => Nav::Right,
        "Home" | "KP_Home" => Nav::Home,
        "End" | "KP_End" => Nav::End,
        "Page_Up" | "Prior" | "KP_Page_Up" | "KP_Prior" => Nav::PageUp,
        "Page_Down" | "Next" | "KP_Page_Down" | "KP_Next" => Nav::PageDown,
        _ => return None,
    })
}

fn is_enter(name: &str) -> bool {
    matches!(name, "Return" | "KP_Enter" | "ISO_Enter")
}

/// Map a key press for the model's current `mode` and `tab`. `None`: let
/// the focused widget have it.
pub fn map_key(k: &KeyPress<'_>, mode: Mode, tab: Tab) -> Option<Event> {
    let m = k.mods;
    if m.logo || is_modifier(k.name) {
        return None;
    }
    if !m.ctrl && !m.alt {
        match k.name {
            "Tab" | "KP_Tab" => return Some(Event::CycleTab { backwards: m.shift }),
            "ISO_Left_Tab" => return Some(Event::CycleTab { backwards: true }),
            "F1" if !m.shift => return Some(Event::ToggleHelp),
            _ => {}
        }
    }
    if m.alt {
        if !m.ctrl && !m.shift {
            if let Some(tab) = normal_char(k)
                .and_then(|c| c.to_digit(10))
                .and_then(|d| Tab::from_number(d as u8))
            {
                return Some(Event::ShowTab(tab));
            }
        }
        // Never typed into a field in Normal mode.
        return (mode == Mode::Normal).then_some(Event::Normal(NormalKey::Other));
    }
    match mode {
        Mode::Normal => Some(normal_key(k)),
        Mode::Insert if tab.is_page() => (k.name == "Escape").then_some(Event::Escape),
        Mode::Insert => insert_key(k),
    }
}

fn insert_key(k: &KeyPress<'_>) -> Option<Event> {
    let m = k.mods;
    match k.name {
        // Ctrl/Shift+Left/Right edit the query (word jump, selection), and
        // Shift+Home/End select text in it.
        "Left" | "KP_Left" | "Right" | "KP_Right" if m.ctrl || m.shift => return None,
        "Home" | "KP_Home" | "End" | "KP_End" if m.shift => return None,
        name if is_enter(name) => {
            return Some(Event::CopySelected {
                toggle_sticker: m.shift,
            })
        }
        "Escape" => return Some(Event::Escape),
        name => {
            if let Some(nav) = nav_of(name) {
                return Some(Event::Navigate(nav));
            }
        }
    }
    if !m.ctrl {
        return None;
    }
    match ctrl_letter(k)? {
        't' => Some(Event::CycleTarget { backwards: m.shift }),
        's' if !m.shift => Some(Event::ToggleSticker),
        'd' if !m.shift => Some(Event::ToggleFavorite),
        _ => None,
    }
}

fn normal_key(k: &KeyPress<'_>) -> Event {
    let m = k.mods;
    let key = Event::Normal;
    if let Some(nav) = nav_of(k.name) {
        return key(NormalKey::Nav(nav));
    }
    match k.name {
        name if is_enter(name) => return key(NormalKey::Enter { shift: m.shift }),
        "Escape" => return key(NormalKey::Escape),
        "BackSpace" => return key(NormalKey::Backspace),
        "space" | "KP_Space" => return key(NormalKey::Space),
        _ => {}
    }
    if m.ctrl {
        return match ctrl_letter(k) {
            Some('t') => Event::CycleTarget { backwards: m.shift },
            Some('s') if !m.shift => Event::ToggleSticker,
            Some(c @ ('d' | 'u')) if !m.shift => key(NormalKey::Ctrl(c)),
            _ => key(NormalKey::Other),
        };
    }
    key(normal_char(k).map_or(NormalKey::Other, NormalKey::Char))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods {
        ctrl: false,
        shift: false,
        alt: false,
        logo: false,
    };
    const CTRL: Mods = Mods { ctrl: true, ..NONE };
    const SHIFT: Mods = Mods { shift: true, ..NONE };
    const ALT: Mods = Mods { alt: true, ..NONE };
    const CTRL_SHIFT: Mods = Mods {
        ctrl: true,
        shift: true,
        ..NONE
    };

    fn press(name: &str, latin: Option<&str>, mods: Mods, mode: Mode, tab: Tab) -> Option<Event> {
        map_key(&KeyPress { name, latin, mods }, mode, tab)
    }

    /// Insert mode on Search, as the popup opens.
    fn key(name: &str, mods: Mods) -> Option<Event> {
        press(name, None, mods, Mode::Insert, Tab::Search)
    }

    fn normal(name: &str, mods: Mods) -> Option<Event> {
        press(name, None, mods, Mode::Normal, Tab::Recent)
    }

    /// Normal mode with a Ukrainian layout active: `name` is the Cyrillic
    /// keyval, `latin` the same key in the first (US) layout.
    fn ua(name: &str, latin: &str, mods: Mods) -> Option<Event> {
        press(name, Some(latin), mods, Mode::Normal, Tab::Recent)
    }

    fn ch(c: char) -> Option<Event> {
        Some(Event::Normal(NormalKey::Char(c)))
    }

    const OTHER: Option<Event> = Some(Event::Normal(NormalKey::Other));

    // -- Insert mode ---------------------------------------------------------------

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
        assert_eq!(key("j", NONE), None);
        assert_eq!(key("slash", NONE), None);
        assert_eq!(key("a", CTRL), None);
        assert_eq!(key("BackSpace", NONE), None);
        assert_eq!(key("space", NONE), None);
        assert_eq!(key("Cyrillic_ie", NONE), None);
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
    fn escape_goes_to_the_model() {
        // The model decides: Normal mode, or hide with `modal` off.
        assert_eq!(key("Escape", NONE), Some(Event::Escape));
    }

    #[test]
    fn ctrl_shortcuts() {
        assert_eq!(key("t", CTRL), Some(Event::CycleTarget { backwards: false }));
        assert_eq!(key("T", CTRL_SHIFT), Some(Event::CycleTarget { backwards: true }));
        assert_eq!(key("s", CTRL), Some(Event::ToggleSticker));
        assert_eq!(key("S", CTRL_SHIFT), None);
        assert_eq!(key("d", CTRL), Some(Event::ToggleFavorite));
        assert_eq!(key("D", CTRL_SHIFT), None);
        assert_eq!(key("t", NONE), None, "plain t is typed into the query");
    }

    #[test]
    fn ctrl_shortcuts_work_in_non_latin_layouts() {
        let ua = |name, latin, mods| press(name, Some(latin), mods, Mode::Insert, Tab::Search);
        assert_eq!(
            ua("Cyrillic_ie", "t", CTRL),
            Some(Event::CycleTarget { backwards: false })
        );
        assert_eq!(ua("Cyrillic_i", "s", CTRL), Some(Event::ToggleSticker));
        assert_eq!(ua("Cyrillic_ve", "d", CTRL), Some(Event::ToggleFavorite));
        assert_eq!(ua("Cyrillic_ie", "Cyrillic_ie", CTRL), None);
        assert_eq!(ua("Cyrillic_ie", "t", NONE), None);
    }

    #[test]
    fn super_is_left_alone() {
        assert_eq!(key("Return", Mods { logo: true, ..NONE }), None);
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
        assert_eq!(normal("j", Mods { logo: true, ..NONE }), None);
    }

    #[test]
    fn insert_on_packs_is_insert_on_search() {
        let packs = |name, mods| press(name, None, mods, Mode::Insert, Tab::Packs);
        assert_eq!(packs("Down", NONE), Some(Event::Navigate(Nav::Down)));
        assert_eq!(
            packs("Return", NONE),
            Some(Event::CopySelected { toggle_sticker: false })
        );
        assert_eq!(packs("minus", NONE), None);
        assert_eq!(packs("BackSpace", NONE), None);
    }

    #[test]
    fn insert_on_a_page_leaves_keys_to_its_widgets() {
        for tab in [Tab::Me, Tab::Settings] {
            let page = |name, mods| press(name, None, mods, Mode::Insert, tab);
            assert_eq!(page("Escape", NONE), Some(Event::Escape));
            for name in ["Up", "Down", "Left", "Right", "Home", "Return", "a", "j", "space"] {
                assert_eq!(page(name, NONE), None, "{name}");
            }
            assert_eq!(page("s", CTRL), None, "Ctrl+S may mean something to a form");
            assert_eq!(page("Tab", NONE), Some(Event::CycleTab { backwards: false }));
            assert_eq!(page("2", ALT), Some(Event::ShowTab(Tab::Recent)));
        }
    }

    // -- every mode ----------------------------------------------------------------------

    #[test]
    fn tab_keys_in_every_mode() {
        for mode in [Mode::Insert, Mode::Normal] {
            for tab in Tab::ALL {
                let p = |name, mods| press(name, None, mods, mode, tab);
                assert_eq!(p("Tab", NONE), Some(Event::CycleTab { backwards: false }));
                assert_eq!(p("Tab", SHIFT), Some(Event::CycleTab { backwards: true }));
                assert_eq!(p("ISO_Left_Tab", SHIFT), Some(Event::CycleTab { backwards: true }));
                assert_eq!(p("1", ALT), Some(Event::ShowTab(Tab::Search)));
                assert_eq!(p("7", ALT), Some(Event::ShowTab(Tab::Settings)));
                assert_eq!(p("KP_4", ALT), Some(Event::ShowTab(Tab::Packs)));
                assert_eq!(p("F1", NONE), Some(Event::ToggleHelp));
            }
        }
    }

    #[test]
    fn other_alt_keys() {
        assert_eq!(key("8", ALT), None);
        assert_eq!(key("a", ALT), None);
        assert_eq!(key("Return", ALT), None);
        assert_eq!(normal("8", ALT), OTHER, "never typed in Normal mode");
        assert_eq!(normal("a", ALT), OTHER);
        // A layout with symbols on the digit row: the physical key counts.
        assert_eq!(
            press("quotedbl", Some("3"), ALT, Mode::Insert, Tab::Search),
            Some(Event::ShowTab(Tab::MostUsed))
        );
        assert_eq!(key("3", Mods { shift: true, ..ALT }), None);
        assert_eq!(key("Tab", CTRL), None, "Ctrl+Tab: GTK focus keys");
    }

    // -- Normal mode -------------------------------------------------------------------------

    #[test]
    fn normal_letters_digits_and_punctuation() {
        for c in [
            'h', 'j', 'k', 'l', 'g', 'y', 'f', 'q', 'o', 'G', 'Y', 'T', '0', '3', '7', '9',
        ] {
            assert_eq!(normal(&c.to_string(), NONE), ch(c));
        }
        assert_eq!(normal("G", SHIFT), ch('G'));
        assert_eq!(normal("slash", NONE), ch('/'));
        assert_eq!(normal("question", SHIFT), ch('?'));
        assert_eq!(normal("minus", NONE), ch('-'));
        assert_eq!(normal("dollar", SHIFT), ch('$'));
        assert_eq!(normal("KP_5", NONE), ch('5'));
        assert_eq!(normal("KP_Subtract", NONE), ch('-'));
        assert_eq!(normal("KP_Divide", NONE), ch('/'));
    }

    #[test]
    fn normal_special_keys() {
        let nk = |k| Some(Event::Normal(k));
        assert_eq!(normal("Down", NONE), nk(NormalKey::Nav(Nav::Down)));
        assert_eq!(normal("Left", SHIFT), nk(NormalKey::Nav(Nav::Left)));
        assert_eq!(normal("Next", NONE), nk(NormalKey::Nav(Nav::PageDown)));
        assert_eq!(normal("Return", NONE), nk(NormalKey::Enter { shift: false }));
        assert_eq!(normal("KP_Enter", SHIFT), nk(NormalKey::Enter { shift: true }));
        assert_eq!(normal("Escape", NONE), nk(NormalKey::Escape));
        assert_eq!(normal("BackSpace", NONE), nk(NormalKey::Backspace));
        assert_eq!(normal("space", NONE), nk(NormalKey::Space));
        assert_eq!(normal("d", CTRL), nk(NormalKey::Ctrl('d')));
        assert_eq!(normal("u", CTRL), nk(NormalKey::Ctrl('u')));
        assert_eq!(normal("t", CTRL), Some(Event::CycleTarget { backwards: false }));
        assert_eq!(normal("T", CTRL_SHIFT), Some(Event::CycleTarget { backwards: true }));
        assert_eq!(normal("s", CTRL), Some(Event::ToggleSticker));
    }

    #[test]
    fn every_printable_key_is_consumed_in_normal_mode() {
        for name in [
            "exclam",
            "period",
            "comma",
            "z",
            "Cyrillic_ie",
            "F5",
            "Delete",
            "Insert",
            "",
            "BackSpace",
        ] {
            assert!(normal(name, NONE).is_some(), "{name}");
        }
        assert_eq!(normal("exclam", SHIFT), OTHER, "Shift+1 is not 1");
        assert_eq!(normal("a", CTRL), OTHER);
        assert_eq!(normal("D", CTRL_SHIFT), OTHER);
        assert_eq!(normal("Cyrillic_ie", NONE), OTHER, "no Latin key known");
    }

    #[test]
    fn modifiers_alone_never_cancel_a_sequence() {
        for name in [
            "Shift_L",
            "Shift_R",
            "Control_L",
            "Alt_R",
            "Super_L",
            "Caps_Lock",
            "ISO_Level3_Shift",
            "ISO_Next_Group",
            "ISO_Level5_Lock",
        ] {
            assert_eq!(normal(name, NONE), None, "{name}");
            assert_eq!(normal(name, SHIFT), None, "{name}");
            assert_eq!(key(name, NONE), None, "{name}");
        }
        assert_eq!(
            normal("ISO_Enter", NONE),
            Some(Event::Normal(NormalKey::Enter { shift: false }))
        );
        assert_eq!(normal("ISO_Left_Tab", SHIFT), Some(Event::CycleTab { backwards: true }));
    }

    #[test]
    fn normal_keys_by_physical_key_in_a_cyrillic_layout() {
        // Ukrainian layout on a US keyboard: Cyrillic_o is on j's key,
        // Cyrillic_er on h's, Cyrillic_pe on g's.
        assert_eq!(ua("Cyrillic_o", "j", NONE), ch('j'));
        assert_eq!(ua("Cyrillic_er", "h", NONE), ch('h'));
        assert_eq!(ua("Cyrillic_pe", "g", NONE), ch('g'));
        assert_eq!(ua("Cyrillic_PE", "g", SHIFT), ch('G'), "Shift from the first layout");
        assert_eq!(ua("Cyrillic_en", "y", SHIFT), ch('Y'));
        // Punctuation moves around: the US slash key types a period in UA.
        assert_eq!(ua("period", "slash", NONE), ch('/'));
        assert_eq!(ua("comma", "slash", SHIFT), ch('?'));
        assert_eq!(ua("semicolon", "4", SHIFT), ch('$'));
        assert_eq!(ua("quotedbl", "2", SHIFT), OTHER, "Shift+2 is no digit");
        // Keys that exist in both layouts read the same.
        assert_eq!(ua("3", "3", NONE), ch('3'));
        assert_eq!(ua("minus", "minus", NONE), ch('-'));
        // A Ukrainian ? (Shift+7) is a ? too.
        assert_eq!(ua("question", "7", SHIFT), ch('?'));
        // Ctrl+d / Ctrl+u by physical key as well.
        assert_eq!(ua("Cyrillic_ve", "d", CTRL), Some(Event::Normal(NormalKey::Ctrl('d'))));
        assert_eq!(ua("Cyrillic_ghe", "u", CTRL), Some(Event::Normal(NormalKey::Ctrl('u'))));
    }

    #[test]
    fn normal_mode_on_pages_maps_the_same_keys() {
        let page = |name, mods| press(name, None, mods, Mode::Normal, Tab::Me);
        assert_eq!(page("j", NONE), ch('j'));
        assert_eq!(page("i", NONE), ch('i'));
        assert_eq!(page("space", NONE), Some(Event::Normal(NormalKey::Space)));
        assert_eq!(
            page("Return", NONE),
            Some(Event::Normal(NormalKey::Enter { shift: false }))
        );
    }
}
