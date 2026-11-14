//! `NSEvent` key presses as the key names the GTK client's key map
//! (`stickpile_desktop::keys`) reads, so both popups share one key map.
//!
//! Cmd stands in for Ctrl (Cmd+T, Cmd+S, Cmd+D; Control works too), and
//! Cmd+1..7 for Alt+1..7 (tabs). Option is Alt.

use stickpile_desktop::keys::{KeyPress, Mods};

use crate::bridge::KeyInput;

/// GTK key names of the keys `NSEvent` describes by code (Carbon `kVK_*`).
fn special(code: u16) -> Option<&'static str> {
    Some(match code {
        36 => "Return",
        76 => "KP_Enter",
        48 => "Tab",
        53 => "Escape",
        51 => "BackSpace",
        117 => "Delete",
        123 => "Left",
        124 => "Right",
        125 => "Down",
        126 => "Up",
        115 => "Home",
        119 => "End",
        116 => "Page_Up",
        121 => "Page_Down",
        122 => "F1",
        49 => "space",
        _ => return None,
    })
}

/// The GTK key name of one typed character: `slash`, `question`, `g`, `G`, `й`.
fn char_name(s: &str) -> Option<String> {
    let mut chars = s.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(
        match c {
            '/' => "slash",
            '?' => "question",
            '-' => "minus",
            '$' => "dollar",
            ' ' => "space",
            c => return Some(c.to_string()),
        }
        .to_string(),
    )
}

/// A key press in the GTK client's terms, owning its names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacKey {
    pub name: String,
    pub latin: Option<String>,
    pub mods: (bool, bool, bool),
}

impl MacKey {
    pub fn from_input(k: &KeyInput) -> MacKey {
        let name = special(k.key_code)
            .map(str::to_string)
            .or_else(|| char_name(&k.characters))
            .unwrap_or_default();
        let latin = k.latin.as_deref().and_then(char_name);
        let digit = |s: &str| s.len() == 1 && s.as_bytes()[0].is_ascii_digit();
        // Cmd+1..7 switch tabs, like Alt+1..7 on Linux.
        let cmd_digit = k.command && (digit(&name) || latin.as_deref().is_some_and(digit));
        let ctrl = (k.command || k.control) && !cmd_digit;
        let alt = k.option || cmd_digit;
        MacKey {
            name,
            latin,
            mods: (ctrl, k.shift, alt),
        }
    }

    pub fn press(&self) -> KeyPress<'_> {
        let (ctrl, shift, alt) = self.mods;
        KeyPress {
            name: &self.name,
            latin: self.latin.as_deref(),
            mods: Mods {
                ctrl,
                shift,
                alt,
                logo: false,
            },
        }
    }
}

/// Key names in the help, as a Mac keyboard labels them.
pub fn mac_key_names(keys: &str) -> String {
    keys.replace("Ctrl+", "Cmd+").replace("Alt+", "Cmd+")
}

#[cfg(test)]
mod tests {
    use super::*;
    use stickpile_desktop::keys::map_key;
    use stickpile_quick::model::{Event, Nav};
    use stickpile_quick::{Mode, NormalKey, Tab};

    fn key(code: u16, chars: &str) -> KeyInput {
        KeyInput {
            key_code: code,
            characters: chars.into(),
            latin: None,
            command: false,
            control: false,
            option: false,
            shift: false,
        }
    }

    fn map(k: &KeyInput, mode: Mode, tab: Tab) -> Option<Event> {
        map_key(&MacKey::from_input(k).press(), mode, tab)
    }

    #[test]
    fn named_keys() {
        assert_eq!(
            map(&key(36, "\r"), Mode::Insert, Tab::Search),
            Some(Event::CopySelected { toggle_sticker: false })
        );
        assert_eq!(map(&key(53, "\u{1b}"), Mode::Insert, Tab::Search), Some(Event::Escape));
        assert_eq!(
            map(&key(125, "\u{f701}"), Mode::Insert, Tab::Search),
            Some(Event::Navigate(Nav::Down))
        );
        let shift_tab = KeyInput {
            shift: true,
            ..key(48, "\t")
        };
        assert_eq!(
            map(&shift_tab, Mode::Normal, Tab::Search),
            Some(Event::CycleTab { backwards: true })
        );
    }

    #[test]
    fn typing_goes_to_the_field() {
        assert_eq!(map(&key(0, "a"), Mode::Insert, Tab::Search), None);
        let cmd_a = KeyInput {
            command: true,
            ..key(0, "a")
        };
        assert_eq!(map(&cmd_a, Mode::Insert, Tab::Search), None, "select all");
    }

    #[test]
    fn vim_keys_and_cyrillic() {
        assert_eq!(
            map(&key(38, "j"), Mode::Normal, Tab::Search),
            Some(Event::Normal(NormalKey::Char('j')))
        );
        let cyrillic = KeyInput {
            latin: Some("j".into()),
            ..key(38, "о")
        };
        assert_eq!(
            map(&cyrillic, Mode::Normal, Tab::Search),
            Some(Event::Normal(NormalKey::Char('j')))
        );
        let shifted = KeyInput {
            shift: true,
            ..key(44, "?")
        };
        assert_eq!(
            map(&shifted, Mode::Normal, Tab::Search),
            Some(Event::Normal(NormalKey::Char('?')))
        );
    }

    #[test]
    fn command_is_ctrl_and_digits_switch_tabs() {
        let cmd_t = KeyInput {
            command: true,
            ..key(17, "t")
        };
        assert_eq!(
            map(&cmd_t, Mode::Insert, Tab::Search),
            Some(Event::CycleTarget { backwards: false })
        );
        let cmd_t_cyrillic = KeyInput {
            command: true,
            latin: Some("t".into()),
            ..key(17, "е")
        };
        assert_eq!(
            map(&cmd_t_cyrillic, Mode::Insert, Tab::Search),
            Some(Event::CycleTarget { backwards: false })
        );
        let control_s = KeyInput {
            control: true,
            ..key(1, "s")
        };
        assert_eq!(map(&control_s, Mode::Insert, Tab::Search), Some(Event::ToggleSticker));
        let cmd_3 = KeyInput {
            command: true,
            ..key(20, "3")
        };
        assert_eq!(
            map(&cmd_3, Mode::Insert, Tab::Search),
            Some(Event::ShowTab(Tab::MostUsed))
        );
    }

    #[test]
    fn help_names() {
        assert_eq!(mac_key_names("Ctrl+T / Ctrl+S"), "Cmd+T / Cmd+S");
        assert_eq!(mac_key_names("1-7, Alt+1-7"), "1-7, Cmd+1-7");
    }
}
