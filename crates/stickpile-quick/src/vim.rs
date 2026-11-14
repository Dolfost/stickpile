//! Normal-mode key sequences: counts (`3j`), `g` prefixes (`gg`, `gt`,
//! `gx`), `z` prefixes (`zf`, `ze`, `zt`) and the ambiguity between a count
//! and a tab number.
//!
//! A lone `1`-`7` is either tab 1-7 or the start of a count. Like vim with
//! an ambiguous mapping, it waits: the next key decides (`3j` is a count,
//! `3y` is "tab 3, then y"), and if none comes the shell's timer
//! ([`crate::model::Command::KeyTimeout`]) makes it a tab switch. Where
//! counts mean nothing (the Me and Settings pages) digits switch at once.

use crate::model::{Nav, Tile};
use crate::tabs::Tab;

/// Counts are capped here; nothing in the popup is that long.
const MAX_COUNT: usize = 9999;

/// A key pressed in Normal mode, as the shell translated it (letters,
/// digits and punctuation through the Latin layout, so they work with a
/// Cyrillic layout active).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalKey {
    /// A printable key: `j`, `G`, `3`, `/`, `?`, `-`, `$`.
    Char(char),
    /// Ctrl with a letter, lowercase: `Ctrl('d')`.
    Ctrl(char),
    Enter {
        shift: bool,
    },
    Escape,
    Backspace,
    Space,
    /// Arrows, Home / End, Page Up / Page Down.
    Nav(Nav),
    /// Any other key. Consumed so it never reaches a text field; it cancels
    /// a pending sequence.
    Other,
}

/// A complete Normal-mode command, before the model applies it to the
/// current view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cmd {
    /// `hjkl`, arrows, Home / End, Page keys, repeated `count` times.
    Move(Nav, usize),
    /// Ctrl+d / Ctrl+u
    HalfPage {
        down: bool,
    },
    /// `{n}gg`, `{n}G`: the n-th item, 1-based.
    GotoItem(usize),
    /// `0`
    RowStart,
    /// `$`
    RowEnd,
    /// `y` / `Y`
    Yank {
        flip: bool,
    },
    /// Enter / Shift+Enter: copy, open a pack or press a page button.
    Enter {
        shift: bool,
    },
    Space,
    /// `/`
    Search,
    /// `i` / `a`
    Insert {
        append: bool,
    },
    /// `1`-`7`, `{n}gt`
    Tab(Tab),
    /// `gt`
    NextTab(usize),
    /// `gT`, `{n}gT`
    PrevTab(usize),
    /// `f`
    Favorite,
    /// `t` / `T`
    Target {
        backwards: bool,
    },
    /// `s`
    Sticker,
    /// `gx`
    Browser,
    /// `?`
    Help,
    /// `q`
    Quit,
    Escape,
    /// `-`, Backspace
    Back,
    /// `o`
    Open,
    /// `zf`, `ze`, `zt`: show or hide favorite marks, emojis or tags on the tiles.
    Tile(Tile),
}

/// What feeding a key gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fed {
    /// The sequence is incomplete. `timeout`: ask the shell for a
    /// `KeyTimeout` with this sequence number (a lone tab digit).
    Wait { timeout: Option<u64> },
    /// Run these, in order (two when a pending tab digit was flushed by a
    /// key that takes no count). Empty when the sequence was cancelled.
    Run(Vec<Cmd>),
}

/// Keys typed so far in an incomplete command (vim's showcmd).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Pending {
    count: Option<usize>,
    digits: u8,
    g: bool,
    z: bool,
    /// Bumped on every change, so a timeout for an older state is ignored.
    seq: u64,
}

impl Pending {
    pub(crate) fn is_empty(&self) -> bool {
        self.count.is_none() && !self.g && !self.z
    }

    pub(crate) fn clear(&mut self) {
        if !self.is_empty() {
            self.count = None;
            self.digits = 0;
            self.g = false;
            self.z = false;
            self.seq += 1;
        }
    }

    /// `"3"`, `"12"`, `"g"`, `"5g"`, `"z"`, or empty.
    pub(crate) fn display(&self) -> String {
        let mut s = self.count.map(|n| n.to_string()).unwrap_or_default();
        if self.g {
            s.push('g');
        }
        if self.z {
            s.push('z');
        }
        s
    }

    /// A single digit that may still be a tab number.
    fn tab_digit(&self) -> Option<Tab> {
        if self.digits != 1 || self.g || self.z {
            return None;
        }
        Tab::from_number(self.count? as u8)
    }

    /// The shell's timer for `seq` fired: a lone tab digit becomes a tab switch.
    pub(crate) fn timeout(&mut self, seq: u64) -> Option<Tab> {
        if seq != self.seq {
            return None;
        }
        let tab = self.tab_digit()?;
        self.clear();
        Some(tab)
    }

    /// `counts`: the view has a grid, so digits may start a count.
    pub(crate) fn feed(&mut self, key: NormalKey, counts: bool) -> Fed {
        if key == NormalKey::Escape {
            if self.is_empty() {
                return Fed::Run(vec![Cmd::Escape]);
            }
            self.clear();
            return Fed::Run(Vec::new());
        }
        if self.g {
            let count = self.count;
            self.clear();
            let cmd = match key {
                NormalKey::Char('g') => Some(count.map_or(Cmd::Move(Nav::Home, 1), Cmd::GotoItem)),
                NormalKey::Char('t') => match count {
                    Some(n) => u8::try_from(n).ok().and_then(Tab::from_number).map(Cmd::Tab),
                    None => Some(Cmd::NextTab(1)),
                },
                NormalKey::Char('T') => Some(Cmd::PrevTab(count.unwrap_or(1))),
                NormalKey::Char('x') => Some(Cmd::Browser),
                _ => None,
            };
            return Fed::Run(cmd.into_iter().collect());
        }
        if self.z {
            // Takes no count; one typed before the z is dropped.
            self.clear();
            let tile = match key {
                NormalKey::Char('f') => Some(Tile::Favorites),
                NormalKey::Char('e') => Some(Tile::Emojis),
                NormalKey::Char('t') => Some(Tile::Tags),
                _ => None,
            };
            return Fed::Run(tile.map(Cmd::Tile).into_iter().collect());
        }
        if let NormalKey::Char(c @ '0'..='9') = key {
            let d = c as usize - '0' as usize;
            match self.count {
                Some(n) => {
                    self.count = Some((n * 10 + d).min(MAX_COUNT));
                    self.digits = self.digits.saturating_add(1);
                    self.seq += 1;
                    return Fed::Wait { timeout: None };
                }
                None if d == 0 => return Fed::Run(vec![Cmd::RowStart]),
                None if !counts => return Fed::Run(Tab::from_number(d as u8).map(Cmd::Tab).into_iter().collect()),
                None => {
                    self.count = Some(d);
                    self.digits = 1;
                    self.seq += 1;
                    let timeout = Tab::from_number(d as u8).map(|_| self.seq);
                    return Fed::Wait { timeout };
                }
            }
        }
        if key == NormalKey::Char('g') {
            self.g = true;
            self.seq += 1;
            return Fed::Wait { timeout: None };
        }
        if key == NormalKey::Char('z') {
            self.z = true;
            self.seq += 1;
            return Fed::Wait { timeout: None };
        }
        let n = self.count.unwrap_or(1);
        let counted = match key {
            NormalKey::Char('h') => Some(Cmd::Move(Nav::Left, n)),
            NormalKey::Char('j') => Some(Cmd::Move(Nav::Down, n)),
            NormalKey::Char('k') => Some(Cmd::Move(Nav::Up, n)),
            NormalKey::Char('l') => Some(Cmd::Move(Nav::Right, n)),
            NormalKey::Nav(nav) => Some(Cmd::Move(nav, n)),
            NormalKey::Char('G') => Some(self.count.map_or(Cmd::Move(Nav::End, 1), Cmd::GotoItem)),
            NormalKey::Ctrl('d') => Some(Cmd::HalfPage { down: true }),
            NormalKey::Ctrl('u') => Some(Cmd::HalfPage { down: false }),
            _ => None,
        };
        if let Some(cmd) = counted {
            self.clear();
            return Fed::Run(vec![cmd]);
        }
        // Takes no count: a lone digit was a tab number after all, a longer
        // count is dropped (as vim does for commands without one).
        let mut cmds: Vec<Cmd> = self.tab_digit().map(Cmd::Tab).into_iter().collect();
        self.clear();
        cmds.extend(single(key));
        Fed::Run(cmds)
    }
}

fn single(key: NormalKey) -> Option<Cmd> {
    Some(match key {
        NormalKey::Char('y') => Cmd::Yank { flip: false },
        NormalKey::Char('Y') => Cmd::Yank { flip: true },
        NormalKey::Enter { shift } => Cmd::Enter { shift },
        NormalKey::Space => Cmd::Space,
        NormalKey::Char('/') => Cmd::Search,
        NormalKey::Char('i') => Cmd::Insert { append: false },
        NormalKey::Char('a') => Cmd::Insert { append: true },
        NormalKey::Char('f') => Cmd::Favorite,
        NormalKey::Char('t') => Cmd::Target { backwards: false },
        NormalKey::Char('T') => Cmd::Target { backwards: true },
        NormalKey::Char('s') => Cmd::Sticker,
        NormalKey::Char('?') => Cmd::Help,
        NormalKey::Char('q') => Cmd::Quit,
        NormalKey::Char('-') | NormalKey::Backspace => Cmd::Back,
        NormalKey::Char('o') => Cmd::Open,
        NormalKey::Char('$') => Cmd::RowEnd,
        _ => return None,
    })
}

/// One block of the key help overlay (`?`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelpSection {
    pub title: &'static str,
    /// (keys, what they do)
    pub keys: &'static [(&'static str, &'static str)],
}

/// The key help the shell draws when [`crate::Model::help_visible`] is on.
pub const HELP: &[HelpSection] = &[
    HelpSection {
        title: "Normal mode",
        keys: &[
            ("h j k l, arrows", "move"),
            ("3j, 2l", "move with a count"),
            ("gg / G, 5G", "first / last / fifth item"),
            ("0 / $", "start / end of the row"),
            ("Ctrl+d / Ctrl+u", "half a page down / up"),
            ("y, Enter", "copy for the target app"),
            ("Y, Shift+Enter", "copy with sticker mode flipped"),
            ("f", "add to / remove from favorites"),
            ("gx", "open the item in the browser"),
            ("t / T", "next / previous target app"),
            ("s", "toggle sticker mode"),
            ("zf / ze / zt", "favorite marks / emojis / tags on the tiles"),
            ("/", "new search"),
            ("i, a", "edit the query"),
            ("?", "this help"),
            ("q, Esc", "hide"),
        ],
    },
    HelpSection {
        title: "Tabs",
        keys: &[
            ("1-7, Alt+1-7", "go to a tab"),
            ("gt / gT", "next / previous tab"),
            ("Tab / Shift+Tab", "next / previous tab"),
        ],
    },
    HelpSection {
        title: "Packs",
        keys: &[
            ("Enter, o", "open the pack"),
            ("-, Backspace, Esc", "back to the pack list"),
        ],
    },
    HelpSection {
        title: "Insert mode",
        keys: &[
            ("typing", "search, filter packs"),
            ("arrows, Page Up / Down, Home / End", "move"),
            ("Enter / Shift+Enter", "copy / copy with sticker mode flipped"),
            ("Ctrl+T / Ctrl+S / Ctrl+D", "target app / sticker mode / favorite"),
            ("Esc", "Normal mode"),
        ],
    },
    HelpSection {
        title: "Me, Settings",
        keys: &[
            ("j k, arrows", "move between rows"),
            ("h l", "change a choice or a number"),
            ("Enter, Space", "press, toggle, open a choice"),
            ("i", "edit the field; Esc leaves it"),
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(c: char) -> NormalKey {
        NormalKey::Char(c)
    }

    /// Feed a string of plain characters; returns every command run.
    fn keys(p: &mut Pending, s: &str) -> Vec<Cmd> {
        let mut out = Vec::new();
        for c in s.chars() {
            if let Fed::Run(cmds) = p.feed(ch(c), true) {
                out.extend(cmds);
            }
        }
        out
    }

    fn run(s: &str) -> Vec<Cmd> {
        keys(&mut Pending::default(), s)
    }

    #[test]
    fn single_keys() {
        assert_eq!(run("j"), [Cmd::Move(Nav::Down, 1)]);
        assert_eq!(run("k"), [Cmd::Move(Nav::Up, 1)]);
        assert_eq!(run("h"), [Cmd::Move(Nav::Left, 1)]);
        assert_eq!(run("l"), [Cmd::Move(Nav::Right, 1)]);
        assert_eq!(run("G"), [Cmd::Move(Nav::End, 1)]);
        assert_eq!(run("y"), [Cmd::Yank { flip: false }]);
        assert_eq!(run("Y"), [Cmd::Yank { flip: true }]);
        assert_eq!(run("/"), [Cmd::Search]);
        assert_eq!(run("a"), [Cmd::Insert { append: true }]);
        assert_eq!(
            run("tT"),
            [Cmd::Target { backwards: false }, Cmd::Target { backwards: true }]
        );
        assert_eq!(
            run("fs?q-o$0"),
            [
                Cmd::Favorite,
                Cmd::Sticker,
                Cmd::Help,
                Cmd::Quit,
                Cmd::Back,
                Cmd::Open,
                Cmd::RowEnd,
                Cmd::RowStart
            ]
        );
        assert_eq!(run("Z!"), [], "unknown keys do nothing");
    }

    #[test]
    fn counts_repeat_motions() {
        assert_eq!(run("3j"), [Cmd::Move(Nav::Down, 3)]);
        assert_eq!(run("12l"), [Cmd::Move(Nav::Right, 12)]);
        assert_eq!(run("10k"), [Cmd::Move(Nav::Up, 10)], "0 continues a count");
        assert_eq!(run("5G"), [Cmd::GotoItem(5)]);
        assert_eq!(run("99999999j"), [Cmd::Move(Nav::Down, MAX_COUNT)]);
        let mut p = Pending::default();
        assert_eq!(p.feed(ch('4'), true), Fed::Wait { timeout: Some(1) });
        assert_eq!(
            p.feed(NormalKey::Nav(Nav::PageDown), true),
            Fed::Run(vec![Cmd::Move(Nav::PageDown, 4)])
        );
        assert!(p.is_empty());
        assert_eq!(run("8"), [], "8 is no tab: a count waiting for its motion");
        assert_eq!(run("2"), [], "waits for the next key or the timeout");
    }

    #[test]
    fn ctrl_d_and_u_take_no_count_but_consume_it() {
        let mut p = Pending::default();
        p.feed(ch('3'), true);
        assert_eq!(
            p.feed(NormalKey::Ctrl('d'), true),
            Fed::Run(vec![Cmd::HalfPage { down: true }])
        );
        assert_eq!(
            p.feed(NormalKey::Ctrl('u'), true),
            Fed::Run(vec![Cmd::HalfPage { down: false }])
        );
        assert_eq!(p.feed(NormalKey::Ctrl('x'), true), Fed::Run(vec![]));
    }

    #[test]
    fn z_sequences() {
        assert_eq!(run("zf"), [Cmd::Tile(Tile::Favorites)]);
        assert_eq!(run("ze"), [Cmd::Tile(Tile::Emojis)]);
        assert_eq!(run("zt"), [Cmd::Tile(Tile::Tags)]);
        assert_eq!(run("3zt"), [Cmd::Tile(Tile::Tags)], "a count is dropped");
        assert_eq!(run("zZ"), [], "unknown z-sequence is dropped");
        assert_eq!(run("zqj"), [Cmd::Move(Nav::Down, 1)]);
        let mut p = Pending::default();
        assert_eq!(p.feed(ch('z'), true), Fed::Wait { timeout: None });
        assert_eq!(p.display(), "z");
        assert_eq!(p.feed(NormalKey::Escape, true), Fed::Run(vec![]), "Esc cancels z");
        assert!(p.is_empty());
        let mut p = Pending::default();
        p.feed(ch('3'), true);
        p.feed(ch('z'), true);
        assert_eq!(p.display(), "3z");
        assert_eq!(p.timeout(p.seq), None, "3z is no tab switch");
    }

    #[test]
    fn g_sequences() {
        assert_eq!(run("gg"), [Cmd::Move(Nav::Home, 1)]);
        assert_eq!(run("gt"), [Cmd::NextTab(1)]);
        assert_eq!(run("gT"), [Cmd::PrevTab(1)]);
        assert_eq!(run("gx"), [Cmd::Browser]);
        assert_eq!(run("3gg"), [Cmd::GotoItem(3)]);
        assert_eq!(run("4gt"), [Cmd::Tab(Tab::Packs)]);
        assert_eq!(run("9gt"), [], "no tab 9");
        assert_eq!(run("2gT"), [Cmd::PrevTab(2)]);
        assert_eq!(run("gz"), [], "unknown g-sequence is dropped");
        assert_eq!(run("gzj"), [Cmd::Move(Nav::Down, 1)]);
        assert_eq!(run("g3"), [], "a digit does not continue a g");
    }

    #[test]
    fn pending_display_like_showcmd() {
        let mut p = Pending::default();
        assert_eq!(p.display(), "");
        p.feed(ch('1'), true);
        assert_eq!(p.display(), "1");
        p.feed(ch('2'), true);
        assert_eq!(p.display(), "12");
        p.feed(ch('g'), true);
        assert_eq!(p.display(), "12g");
        p.feed(ch('g'), true);
        assert_eq!(p.display(), "");
    }

    #[test]
    fn lone_tab_digit_times_out_into_a_tab_switch() {
        let mut p = Pending::default();
        let Fed::Wait { timeout: Some(seq) } = p.feed(ch('3'), true) else {
            panic!("tab digits ask for a timeout")
        };
        assert_eq!(p.timeout(seq + 1), None, "other sequence");
        assert_eq!(p.timeout(seq), Some(Tab::MostUsed));
        assert!(p.is_empty());
        assert_eq!(p.timeout(seq), None, "only once");
    }

    #[test]
    fn stale_timeouts_are_ignored() {
        let mut p = Pending::default();
        let Fed::Wait { timeout: Some(first) } = p.feed(ch('2'), true) else {
            panic!()
        };
        p.feed(ch('j'), true);
        let Fed::Wait { timeout: Some(second) } = p.feed(ch('2'), true) else {
            panic!()
        };
        assert_ne!(first, second);
        assert_eq!(p.timeout(first), None);
        // A second digit makes it a count: the timeout no longer switches tabs.
        p.feed(ch('5'), true);
        assert_eq!(p.timeout(second), None);
        assert_eq!(p.display(), "25");
    }

    #[test]
    fn lone_tab_digit_flushed_by_a_key_without_count() {
        assert_eq!(run("2y"), [Cmd::Tab(Tab::Recent), Cmd::Yank { flip: false }]);
        assert_eq!(run("5?"), [Cmd::Tab(Tab::Favorites), Cmd::Help]);
        assert_eq!(run("2Z"), [Cmd::Tab(Tab::Recent)]);
        assert_eq!(run("2z"), [], "z waits for its second key");
        assert_eq!(run("2zt"), [Cmd::Tile(Tile::Tags)], "and takes no count");
        assert_eq!(run("23y"), [Cmd::Yank { flip: false }], "a real count is dropped");
        assert_eq!(run("9y"), [Cmd::Yank { flip: false }]);
    }

    #[test]
    fn escape_cancels_a_pending_sequence_first() {
        let mut p = Pending::default();
        assert_eq!(p.feed(NormalKey::Escape, true), Fed::Run(vec![Cmd::Escape]));
        p.feed(ch('g'), true);
        assert_eq!(p.feed(NormalKey::Escape, true), Fed::Run(vec![]));
        assert!(p.is_empty());
        p.feed(ch('3'), true);
        assert_eq!(
            p.feed(NormalKey::Escape, true),
            Fed::Run(vec![]),
            "no tab switch either"
        );
        assert_eq!(p.feed(NormalKey::Escape, true), Fed::Run(vec![Cmd::Escape]));
    }

    #[test]
    fn digits_switch_at_once_without_counts() {
        let mut p = Pending::default();
        assert_eq!(p.feed(ch('6'), false), Fed::Run(vec![Cmd::Tab(Tab::Me)]));
        assert_eq!(p.feed(ch('8'), false), Fed::Run(vec![]));
        assert_eq!(p.feed(ch('0'), false), Fed::Run(vec![Cmd::RowStart]));
        assert!(p.is_empty());
        p.feed(ch('g'), false);
        assert_eq!(p.feed(ch('t'), false), Fed::Run(vec![Cmd::NextTab(1)]));
    }

    #[test]
    fn other_keys_cancel() {
        let mut p = Pending::default();
        p.feed(ch('g'), true);
        assert_eq!(p.feed(NormalKey::Other, true), Fed::Run(vec![]));
        assert!(p.is_empty());
        p.feed(ch('1'), true);
        p.feed(ch('2'), true);
        assert_eq!(p.feed(NormalKey::Other, true), Fed::Run(vec![]));
        assert!(p.is_empty());
    }

    #[test]
    fn enter_space_backspace() {
        let mut p = Pending::default();
        assert_eq!(
            p.feed(NormalKey::Enter { shift: true }, true),
            Fed::Run(vec![Cmd::Enter { shift: true }])
        );
        assert_eq!(p.feed(NormalKey::Space, true), Fed::Run(vec![Cmd::Space]));
        assert_eq!(p.feed(NormalKey::Backspace, true), Fed::Run(vec![Cmd::Back]));
    }

    #[test]
    fn help_lists_every_section() {
        let titles: Vec<_> = HELP.iter().map(|s| s.title).collect();
        assert_eq!(titles, ["Normal mode", "Tabs", "Packs", "Insert mode", "Me, Settings"]);
        assert!(HELP.iter().all(|s| !s.keys.is_empty()));
        assert!(HELP
            .iter()
            .flat_map(|s| s.keys)
            .all(|(k, v)| k.is_ascii() && v.is_ascii()));
    }
}
