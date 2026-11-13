//! Tabs and input modes of the popup.
//!
//! Tabs are numbered 1..=7 in [`Tab::ALL`] order (the `1`-`7` and Alt+1..7
//! keys) and have a stable [`Tab::key`] for the config (`ui.start_tab`).

/// One page of the popup, in tab bar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Tab {
    /// Search the library; an empty query lists recently used items.
    #[default]
    Search,
    Recent,
    MostUsed,
    /// The user's sets; one can be opened to show its items.
    Packs,
    Favorites,
    /// Profile, login and logout (a form the shell renders).
    Me,
    /// Config editor (a form the shell renders).
    Settings,
}

impl Tab {
    pub const ALL: [Tab; 7] = [
        Tab::Search,
        Tab::Recent,
        Tab::MostUsed,
        Tab::Packs,
        Tab::Favorites,
        Tab::Me,
        Tab::Settings,
    ];

    /// 1..=7, the tab's position in the tab bar.
    pub fn number(self) -> u8 {
        self.index() as u8 + 1
    }

    pub fn from_number(n: u8) -> Option<Tab> {
        Tab::ALL.get(usize::from(n).checked_sub(1)?).copied()
    }

    /// Config value (`ui.start_tab`).
    pub fn key(self) -> &'static str {
        match self {
            Tab::Search => "search",
            Tab::Recent => "recent",
            Tab::MostUsed => "most_used",
            Tab::Packs => "packs",
            Tab::Favorites => "favorites",
            Tab::Me => "me",
            Tab::Settings => "settings",
        }
    }

    pub fn from_key(s: &str) -> Option<Tab> {
        Tab::ALL.into_iter().find(|t| t.key() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Search => "Search",
            Tab::Recent => "Recent",
            Tab::MostUsed => "Most used",
            Tab::Packs => "Packs",
            Tab::Favorites => "Favorites",
            Tab::Me => "Me",
            Tab::Settings => "Settings",
        }
    }

    pub(crate) fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).expect("listed")
    }

    /// The next tab in the bar, wrapping (Tab, `gt`).
    pub fn next(self) -> Tab {
        Tab::ALL[(self.index() + 1) % Tab::ALL.len()]
    }

    /// The previous tab in the bar, wrapping (Shift+Tab, `gT`).
    pub fn prev(self) -> Tab {
        let n = Tab::ALL.len();
        Tab::ALL[(self.index() + n - 1) % n]
    }

    /// The tab shows the popup's text field: the search query, or the pack
    /// filter on Packs.
    pub fn has_text_field(self) -> bool {
        matches!(self, Tab::Search | Tab::Packs)
    }

    /// A form drawn by the shell (Me, Settings) rather than a grid; the
    /// model only forwards keys to it as [`crate::model::Command::Page`].
    pub fn is_page(self) -> bool {
        matches!(self, Tab::Me | Tab::Settings)
    }

    /// The mode a tab is shown in (with `modal` on): tabs with the text
    /// field start typing right away, the others start in Normal.
    pub fn default_mode(self) -> Mode {
        if self.has_text_field() {
            Mode::Insert
        } else {
            Mode::Normal
        }
    }
}

/// Who gets the keys (only with `modal` on; otherwise the model stays in
/// [`Mode::Insert`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    /// A text field has the keys: the search query, the pack filter, or a
    /// field on the Me / Settings pages.
    #[default]
    Insert,
    /// Vim keys: `hjkl`, counts, `g` sequences, `y`, tabs by number.
    Normal,
}

impl Mode {
    /// Footer text, as vim shows it.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Insert => "INSERT",
            Mode::Normal => "NORMAL",
        }
    }
}

/// A key the model forwards to a page (Me, Settings) in Normal mode; the
/// shell moves its own focus between the page's rows and fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageKey {
    /// `k`, Up
    Up,
    /// `j`, Down
    Down,
    /// `h`, Left
    Left,
    /// `l`, Right
    Right,
    /// Enter, Space: press the focused button or switch.
    Activate,
    /// `i`: focus the text field of the current row. The shell answers with
    /// [`crate::model::Event::TextFocus`] once a field has the focus.
    Edit,
}

/// What the main area shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Items: search results, recent, most used, favorites, an opened pack
    /// ([`crate::Model::entries`]).
    Items,
    /// The pack list ([`crate::Model::packs`]).
    Packs,
    /// The Me or Settings form ([`crate::Model::tab`] says which).
    Page,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_keys_and_labels_round_trip() {
        for (i, t) in Tab::ALL.into_iter().enumerate() {
            assert_eq!(t.number() as usize, i + 1);
            assert_eq!(Tab::from_number(t.number()), Some(t));
            assert_eq!(Tab::from_key(t.key()), Some(t));
        }
        assert_eq!(Tab::from_number(0), None);
        assert_eq!(Tab::from_number(8), None);
        assert_eq!(Tab::from_key("Search"), None);
        assert_eq!(Tab::from_key(""), None);
        assert_eq!(Tab::MostUsed.key(), "most_used");
        assert_eq!(Tab::MostUsed.label(), "Most used");
        assert_eq!(Tab::Settings.number(), 7);
        assert_eq!(Tab::default(), Tab::Search);
    }

    #[test]
    fn next_and_prev_wrap() {
        assert_eq!(Tab::Search.next(), Tab::Recent);
        assert_eq!(Tab::Settings.next(), Tab::Search);
        assert_eq!(Tab::Search.prev(), Tab::Settings);
        assert_eq!(Tab::Packs.prev(), Tab::MostUsed);
    }

    #[test]
    fn modes_per_tab() {
        let insert: Vec<_> = Tab::ALL
            .into_iter()
            .filter(|t| t.default_mode() == Mode::Insert)
            .collect();
        assert_eq!(insert, [Tab::Search, Tab::Packs]);
        let pages: Vec<_> = Tab::ALL.into_iter().filter(|t| t.is_page()).collect();
        assert_eq!(pages, [Tab::Me, Tab::Settings]);
        assert_eq!(Mode::Normal.label(), "NORMAL");
        assert_eq!(Mode::default(), Mode::Insert);
    }
}
