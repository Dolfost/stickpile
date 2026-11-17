//! Keeping stickers offline, as the popup sees it (`docs/offline.md`).
//!
//! The core decides what is kept and downloads it; the shell tells the
//! model what the user keeps ([`KeepScope`]), whose files are on the device
//! and whether the server answers, and runs what the model asks for. The
//! model draws the tile marks ([`OfflineMark`]), runs the keep key and the
//! item menu ([`Menu`]), and asks before keeping a pack or the library
//! ([`Ask`]). The types mirror `stickpile_core::offline` (this crate does
//! no I/O); the shells convert.

use stickpile_i18n::global::{t, tf, tp};

/// What the user keeps on this device (`stickpile_core::offline::Scope`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeepScope {
    /// One sticker, by item id.
    Item(String),
    /// A pack, by set id.
    Set(String),
    /// Everything.
    Library,
}

/// Why an item is kept (`stickpile_core::offline::Reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum KeptFor {
    /// Kept by itself.
    Item,
    /// Its pack is kept.
    Set,
    /// The library is kept.
    Library,
    /// One of the most recently copied (`offline.auto.used`).
    Used,
    /// Liked (`offline.auto.liked`).
    Liked,
    /// In a pack of the viewer's (`offline.auto.own_packs`).
    OwnPack,
}

/// What keeping a pack or the library would take
/// (`stickpile_core::offline::Estimate`), shown before the user confirms.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Estimate {
    /// Items it keeps (those the options leave out not counted).
    pub items: u64,
    /// Its files and their size, and how much of that is on the device.
    pub files: u64,
    pub bytes: u64,
    pub present_bytes: u64,
    /// What is not downloaded, and why.
    pub skips: Skips,
    /// Bytes every kept file would take with it kept too.
    pub kept_bytes_after: u64,
    /// `offline.max_mb` in bytes; `None`: no limit.
    pub budget: Option<u64>,
}

impl Estimate {
    /// Whether it fits in the budget.
    pub fn fits(&self) -> bool {
        self.budget.is_none_or(|b| self.kept_bytes_after <= b)
    }
}

/// What is not downloaded, counted by reason
/// (`stickpile_core::offline::SkipReason`): the core reports a sticker of
/// a kind switched off once, and a file over the size limit once each, so
/// the two are counted apart, as the Android app does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Skips {
    /// Stickers of a kind switched off (`offline.stills`, `.animations`,
    /// `.videos`), kept only with a pack, the library or a rule.
    pub kind_off: u64,
    /// Files larger than `offline.max_file_mb`.
    pub too_large: u64,
    /// Stickers kept by themselves that the offline cache does not hold yet.
    pub not_cached: u64,
    /// Stickers with no file a copy could use.
    pub no_rendition: u64,
}

impl Skips {
    pub fn is_empty(&self) -> bool {
        *self == Skips::default()
    }

    /// One sentence per reason that has any, in the current language.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.kind_off > 0 {
            out.push(tp(
                self.kind_off,
                "{n} sticker is left out: its kind is off in the Offline settings.",
                "{n} stickers are left out: their kind is off in the Offline settings.",
            ));
        }
        if self.too_large > 0 {
            out.push(tp(
                self.too_large,
                "{n} file is left out: it is larger than the largest file of the Offline settings.",
                "{n} files are left out: they are larger than the largest file of the Offline settings.",
            ));
        }
        if self.not_cached > 0 {
            out.push(tp(
                self.not_cached,
                "{n} sticker is not in the offline cache yet (a sync brings it).",
                "{n} stickers are not in the offline cache yet (a sync brings them).",
            ));
        }
        if self.no_rendition > 0 {
            out.push(tp(
                self.no_rendition,
                "{n} sticker has no file a copy could use.",
                "{n} stickers have no file a copy could use.",
            ));
        }
        out
    }
}

/// What a tile shows about its file being on the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OfflineMark {
    /// Nothing (unknown, or nothing to say).
    #[default]
    None,
    /// The file a copy needs is on the device: it copies without the
    /// server (drawn with `ui.show_offline` on).
    Available,
    /// The server is unreachable and the file is not on the device: drawn
    /// dimmed, and choosing it explains why it does not copy.
    Unavailable,
}

/// "512 B", "1.5 KiB", "12.3 MiB", in the current language: powers of
/// 1024, as the options (`offline.max_mb` is MiB) and the CLI count.
pub fn human_bytes(n: u64) -> String {
    if n < 1024 {
        return tf("{n} B", &[("n", &n)]);
    }
    let mut v = n as f64 / 1024.0;
    let mut unit = 0;
    // 1023.95 and up would print as "1024.0" of the smaller unit.
    while v >= 1023.95 && unit < 3 {
        v /= 1024.0;
        unit += 1;
    }
    let v = format!("{v:.1}");
    match unit {
        0 => tf("{n} KiB", &[("n", &v)]),
        1 => tf("{n} MiB", &[("n", &v)]),
        2 => tf("{n} GiB", &[("n", &v)]),
        _ => tf("{n} TiB", &[("n", &v)]),
    }
}

/// Why an item is kept, in words: "pack Cats, liked". `pack`: the item's
/// pack name. Kept by itself is left out (the menu says that otherwise).
pub fn reasons_text(reasons: &[KeptFor], pack: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for r in reasons {
        let text = match r {
            KeptFor::Item => continue,
            KeptFor::Set => tf("pack {name}", &[("name", &pack)]),
            KeptFor::Library => t("the whole library").to_string(),
            KeptFor::Used => t("recently used").to_string(),
            KeptFor::Liked => t("liked").to_string(),
            KeptFor::OwnPack => t("your pack").to_string(),
        };
        if !out.contains(&text) {
            out.push(text);
        }
    }
    out.join(", ")
}

// -- questions ---------------------------------------------------------------------

/// A question the popup asks before it acts ([`crate::Model::confirm`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    /// Keep a pack or the library: the estimate first.
    Keep {
        scope: KeepScope,
        /// The pack's name ("the whole library" for the library).
        name: String,
        estimate: Estimate,
    },
    /// "Remove kept files" on the Settings page: the kept files and the
    /// list of what to keep go.
    RemoveKept { files: u64, bytes: u64 },
}

/// A question as the shell draws it: a card over the popup with two
/// buttons. Enter or `y` answers yes, Esc, `n` or `q` no.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
    pub title: String,
    /// One paragraph or two (`\n\n` between them).
    pub body: String,
    /// The yes button.
    pub accept: String,
    pub cancel: String,
    /// The yes button deletes something: draw it so.
    pub destructive: bool,
    /// The line under the buttons: the keys.
    pub hint: String,
}

impl Ask {
    pub fn view(&self) -> Confirm {
        match self {
            Ask::Keep { scope, name, estimate } => keep_view(scope, name, estimate),
            Ask::RemoveKept { files, bytes } => Confirm {
                title: t("Remove kept files?").into(),
                body: format!(
                    "{}\n\n{}",
                    tp(
                        *files,
                        "{n} kept file ({size}) is deleted, and nothing stays marked to keep offline.",
                        "{n} kept files ({size}) are deleted, and nothing stays marked to keep offline.",
                    )
                    .replace("{size}", &human_bytes(*bytes)),
                    t("The automatic rules of the Offline settings download what they cover again; turn them off first to keep nothing.")
                ),
                accept: t("Remove kept files").into(),
                cancel: t("Cancel").into(),
                destructive: true,
                hint: t("Enter: remove, Esc: cancel").into(),
            },
        }
    }
}

fn keep_view(scope: &KeepScope, name: &str, e: &Estimate) -> Confirm {
    let title = match scope {
        KeepScope::Library => t("Keep the whole library offline?").to_string(),
        _ => tf("Keep {name} offline?", &[("name", &name)]),
    };
    let size = human_bytes(e.bytes);
    let mut body = tp(
        e.items,
        "{n} sticker, {files} files, {size}.",
        "{n} stickers, {files} files, {size}.",
    )
    .replace("{files}", &e.files.to_string())
    .replace("{size}", &size);
    if e.present_bytes > 0 {
        body.push(' ');
        body.push_str(&tf(
            "{size} of it is on this device already.",
            &[("size", &human_bytes(e.present_bytes))],
        ));
    }
    for line in e.skips.lines() {
        body.push(' ');
        body.push_str(&line);
    }
    body.push_str("\n\n");
    body.push_str(&match e.budget {
        Some(b) if e.fits() => tf(
            "Kept files would take {after} of the {budget} allowed.",
            &[("after", &human_bytes(e.kept_bytes_after)), ("budget", &human_bytes(b))],
        ),
        Some(b) => tf(
            "Kept files would take {after}, over the {budget} allowed: downloads stop at the limit, and nothing is deleted to make room.",
            &[("after", &human_bytes(e.kept_bytes_after)), ("budget", &human_bytes(b))],
        ),
        None => tf(
            "Kept files would take {after}; there is no limit.",
            &[("after", &human_bytes(e.kept_bytes_after))],
        ),
    });
    Confirm {
        title,
        body,
        accept: t("Keep offline").into(),
        cancel: t("Cancel").into(),
        destructive: false,
        hint: t("Enter: keep offline, Esc: cancel").into(),
    }
}

// -- the menu ----------------------------------------------------------------------

/// What an entry of the menu of a tile does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuAction {
    /// Copy for the target app (`y`, Enter).
    Copy,
    /// Copy with sticker mode flipped (`Y`, Shift+Enter).
    CopyFlipped,
    /// Like or unlike (`f`).
    Favorite,
    /// Open the item's page (`gx`).
    Browser,
    /// Keep the item offline (`K`).
    Keep,
    /// Stop keeping the item (`K`).
    Release,
    /// Open the pack (pack list: Enter, `o`).
    OpenPack,
    /// Keep the pack offline, after the estimate (pack list: `K`).
    KeepPack,
    /// Stop keeping the pack (pack list: `K`).
    ReleasePack,
}

/// One line of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuEntry {
    /// `None`: a line that only says something (drawn dimmed, not chosen).
    pub action: Option<MenuAction>,
    pub label: String,
    /// The key that does the same ("K", "gx"); empty for none.
    pub keys: String,
}

/// The menu of a grid cell (right click, the Menu key):
/// [`crate::Model::menu`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    /// The cell it belongs to (in the shown grid: items or packs).
    pub index: usize,
    pub entries: Vec<MenuEntry>,
}

pub(crate) fn entry(action: MenuAction, label: impl Into<String>, keys: &str) -> MenuEntry {
    MenuEntry {
        action: Some(action),
        label: label.into(),
        keys: keys.into(),
    }
}

pub(crate) fn note(label: impl Into<String>) -> MenuEntry {
    MenuEntry {
        action: None,
        label: label.into(),
        keys: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    #[test]
    fn bytes_in_words() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(3 * MIB / 2), "1.5 MiB");
        assert_eq!(human_bytes(1024 * MIB), "1.0 GiB", "the default budget");
        assert_eq!(human_bytes(2_000_000_000), "1.9 GiB");
    }

    #[test]
    fn reasons_in_words() {
        assert_eq!(reasons_text(&[KeptFor::Item], "Cats"), "");
        assert_eq!(
            reasons_text(&[KeptFor::Item, KeptFor::Set, KeptFor::Liked], "Cats"),
            "pack Cats, liked"
        );
        assert_eq!(
            reasons_text(&[KeptFor::Library, KeptFor::Used, KeptFor::OwnPack], "Cats"),
            "the whole library, recently used, your pack"
        );
    }

    #[test]
    fn the_keep_question_says_what_it_takes() {
        let e = Estimate {
            items: 12,
            files: 30,
            bytes: 34 * MIB / 10,
            present_bytes: 400 * 1024,
            skips: Skips {
                kind_off: 2,
                too_large: 3,
                ..Skips::default()
            },
            kept_bytes_after: 5 * MIB,
            budget: Some(1024 * MIB),
        };
        let c = Ask::Keep {
            scope: KeepScope::Set("s1".into()),
            name: "Cats".into(),
            estimate: e.clone(),
        }
        .view();
        assert_eq!(c.title, "Keep Cats offline?");
        assert!(c.body.starts_with("12 stickers, 30 files, 3.4 MiB."), "{}", c.body);
        assert!(
            c.body.contains("400.0 KiB of it is on this device already."),
            "{}",
            c.body
        );
        // Stickers of a kind switched off are stickers, not files.
        assert!(
            c.body
                .contains("2 stickers are left out: their kind is off in the Offline settings."),
            "{}",
            c.body
        );
        assert!(
            c.body
                .contains("3 files are left out: they are larger than the largest file of the Offline settings."),
            "{}",
            c.body
        );
        assert!(
            c.body
                .ends_with("Kept files would take 5.0 MiB of the 1.0 GiB allowed."),
            "{}",
            c.body
        );
        assert!(!c.destructive);

        let over = Ask::Keep {
            scope: KeepScope::Library,
            name: String::new(),
            estimate: Estimate {
                kept_bytes_after: 2048 * MIB,
                ..e.clone()
            },
        }
        .view();
        assert_eq!(over.title, "Keep the whole library offline?");
        assert!(over.body.contains("over the 1.0 GiB allowed"), "{}", over.body);

        let unlimited = Ask::Keep {
            scope: KeepScope::Library,
            name: String::new(),
            estimate: Estimate { budget: None, ..e },
        }
        .view();
        assert!(unlimited.body.ends_with("there is no limit."), "{}", unlimited.body);
    }

    #[test]
    fn the_other_reasons_are_named_as_such() {
        let lines = Skips {
            not_cached: 1,
            no_rendition: 2,
            ..Skips::default()
        }
        .lines();
        assert_eq!(
            lines,
            [
                "1 sticker is not in the offline cache yet (a sync brings it).",
                "2 stickers have no file a copy could use.",
            ]
        );
        assert!(Skips::default().lines().is_empty());
        assert!(Skips::default().is_empty());
    }

    #[test]
    fn removing_kept_files_is_destructive() {
        let c = Ask::RemoveKept {
            files: 3,
            bytes: 5 * MIB / 2,
        }
        .view();
        assert!(c.destructive);
        assert!(c.body.starts_with("3 kept files (2.5 MiB) are deleted"), "{}", c.body);
        assert_eq!(c.accept, "Remove kept files");
    }
}
