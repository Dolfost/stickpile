//! Keeping offline in the model (`docs/offline.md`): the tile marks, `K`,
//! the menu of a cell, and the question before keeping a pack or the
//! library or removing the kept files.
//!
//! * What is kept comes from the shell ([`Event::OfflineKeeps`]) and is
//!   kept in step with the model's own keeps and releases.
//! * Whose files are on the device is asked for the shown items
//!   ([`Command::CheckOffline`], one at a time) whenever they change, the
//!   target app or the sticker flag changes, files arrive
//!   ([`Event::OfflineFiles`]) or the server stops answering. A tile shows
//!   [`OfflineMark::Available`] with `ui.show_offline` on while the server
//!   answers, and
//!   [`OfflineMark::Unavailable`] (dimmed) while the server is unreachable.
//! * `K` keeps or stops keeping the selected sticker at once; on the pack
//!   list it stops keeping a kept pack, or asks for the estimate and then
//!   the user before keeping one. The question takes the keys (Enter or
//!   `y` yes, Esc, `n` or `q` no) until it is answered.

use super::*;
use crate::offline::{entry, note, reasons_text, MenuEntry};

/// The menu of a cell, open.
#[derive(Debug, Clone)]
pub(super) struct MenuState {
    index: usize,
    /// The item it is for (item grids), to drop a stale one.
    item: Option<String>,
    /// Why the item is kept; `None` while asked for (the menu shows once
    /// it is known). Packs need none.
    reasons: Option<Result<Vec<KeptFor>, String>>,
}

#[derive(Debug, Clone)]
pub(super) struct KeepState {
    keeps: Vec<KeepScope>,
    /// By item id: the file a copy needs is on the device, for `avail_for`.
    available: HashMap<String, bool>,
    /// The target app and sticker flag `available` is for.
    avail_for: (TargetApp, bool),
    /// The check in flight: its generation, target and sticker flag.
    checking: Option<(u64, TargetApp, bool)>,
    /// Something changed while it was in flight: check again after it.
    recheck: bool,
    checks: u64,
    /// The server answers, as far as the model knows.
    reachable: bool,
    menu: Option<MenuState>,
    ask: Option<Ask>,
    /// The mode before the question took the keys.
    ask_mode: Option<Mode>,
    /// The keep waiting for its estimate, with its name.
    estimating: Option<(KeepScope, String)>,
    /// Keeps and releases in flight, with the names their status says.
    names: Vec<(KeepScope, String)>,
}

impl KeepState {
    pub(super) fn new(target: TargetApp) -> KeepState {
        KeepState {
            keeps: Vec::new(),
            available: HashMap::new(),
            avail_for: (target, false),
            checking: None,
            recheck: false,
            checks: 0,
            reachable: true,
            menu: None,
            ask: None,
            ask_mode: None,
            estimating: None,
            names: Vec::new(),
        }
    }
}

/// User input that closes the menu of a cell (choosing an entry reads it first).
fn closes_menu(event: &Event) -> bool {
    cancels_pending(event) && !matches!(event, Event::OpenMenu(_) | Event::MenuChosen(_))
}

impl Model {
    // -- read-only view ----------------------------------------------------------------

    /// What the user keeps, as far as the model knows.
    pub fn keeps(&self) -> &[KeepScope] {
        &self.keep.keeps
    }

    /// The whole library is kept (the Settings page's switch).
    pub fn library_kept(&self) -> bool {
        self.keep.keeps.contains(&KeepScope::Library)
    }

    /// The pack is kept by itself (a mark on the pack list).
    pub fn pack_kept(&self, set_id: &str) -> bool {
        self.keep
            .keeps
            .iter()
            .any(|s| matches!(s, KeepScope::Set(id) if id == set_id))
    }

    /// The server answers, as far as the model knows.
    pub fn reachable(&self) -> bool {
        self.keep.reachable
    }

    /// The question to draw over the popup, if one is asked.
    pub fn confirm(&self) -> Option<Confirm> {
        self.keep.ask.as_ref().map(Ask::view)
    }

    /// The menu of a cell to draw, once it is known what it says.
    pub fn menu(&self) -> Option<Menu> {
        let st = self.keep.menu.as_ref()?;
        let entries = match self.view() {
            View::Items => {
                let e = self.entries.get(st.index)?;
                if st.item.as_deref() != Some(e.item.id.as_str()) {
                    return None;
                }
                self.item_menu(e, st.reasons.as_ref()?)
            }
            View::Packs => {
                let set = self.packs.shown.get(st.index)?;
                vec![
                    entry(MenuAction::OpenPack, t("Open the pack"), "o"),
                    self.pack_entry(&set.id, &set.name, "K"),
                ]
            }
            View::Page => return None,
        };
        Some(Menu {
            index: st.index,
            entries,
        })
    }

    fn item_menu(&self, e: &Entry, reasons: &Result<Vec<KeptFor>, String>) -> Vec<MenuEntry> {
        let mut v = vec![entry(MenuAction::Copy, t("Copy"), "y")];
        if supports_stickers(self.target) {
            let label = if self.sticker {
                t("Copy without sticker mode")
            } else {
                t("Copy as a sticker")
            };
            v.push(entry(MenuAction::CopyFlipped, label, "Y"));
        }
        let favorite = match (self.tiles.favorites || self.favorites.loaded, e.liked) {
            (true, true) => t("Remove from favorites"),
            (true, false) => t("Add to favorites"),
            (false, _) => t("add to / remove from favorites"),
        };
        v.push(entry(MenuAction::Favorite, favorite, "f"));
        v.push(entry(MenuAction::Browser, t("Open in the browser"), "gx"));
        let scope = KeepScope::Item(e.item.id.clone());
        let reasons = match reasons {
            Ok(r) => r.clone(),
            // Not known: what the model knows of the keeps.
            Err(_) if self.keep.keeps.contains(&scope) => vec![KeptFor::Item],
            Err(_) => Vec::new(),
        };
        let others = reasons_text(&reasons, &e.item.set_name);
        if reasons.contains(&KeptFor::Item) {
            v.push(entry(MenuAction::Release, t("Stop keeping offline"), "K"));
            if !others.is_empty() {
                v.push(note(tf("Also kept offline: {why}", &[("why", &others)])));
            }
        } else {
            if !others.is_empty() {
                v.push(note(tf("Kept offline: {why}", &[("why", &others)])));
            }
            v.push(entry(MenuAction::Keep, t("Keep offline"), "K"));
        }
        match e.offline {
            OfflineMark::Available => v.push(note(t("Available offline"))),
            OfflineMark::Unavailable => v.push(note(t("Not on this device"))),
            OfflineMark::None => {}
        }
        if let (Tab::Packs, Some(open)) = (self.tab, &self.packs.open) {
            v.push(self.pack_entry(&open.set.id, &open.set.name, ""));
        }
        v
    }

    /// Keep or stop keeping a pack: on the pack list `K` does it too.
    fn pack_entry(&self, set_id: &str, name: &str, keys: &str) -> MenuEntry {
        if self.pack_kept(set_id) {
            entry(
                MenuAction::ReleasePack,
                tf("Stop keeping {pack} offline", &[("pack", &name)]),
                keys,
            )
        } else {
            entry(
                MenuAction::KeepPack,
                tf("Keep {pack} offline...", &[("pack", &name)]),
                keys,
            )
        }
    }

    // -- events ----------------------------------------------------------------------------

    /// With a question asked it takes the user's input: the answer, or
    /// nothing. `None`: the event goes on as usual (answers from the shell,
    /// focus changes; an opening or hiding drops the question first).
    pub(super) fn question_takes(&mut self, event: &Event) -> Option<Vec<Command>> {
        self.keep.ask.as_ref()?;
        match event {
            Event::Confirm(yes) => Some(self.answer(*yes)),
            Event::Normal(NormalKey::Enter { .. } | NormalKey::Char('y')) | Event::CopySelected { .. } => {
                Some(self.answer(true))
            }
            Event::Normal(NormalKey::Escape | NormalKey::Char('n' | 'q')) | Event::Escape => Some(self.answer(false)),
            Event::Open(_) | Event::Toggle { .. } | Event::Hide | Event::Hidden | Event::Reload => {
                self.keep.ask = None;
                self.keep.ask_mode = None;
                None
            }
            e if cancels_pending(e) || matches!(e, Event::Normal(_) | Event::Select(_)) => Some(Vec::new()),
            _ => None,
        }
    }

    /// Close the menu on user input elsewhere.
    pub(super) fn input_closes_menu(&mut self, event: &Event) {
        if closes_menu(event) {
            self.keep.menu = None;
        }
    }

    pub(super) fn keep_event(&mut self, event: Event) -> Vec<Command> {
        match event {
            Event::OfflineKeeps(keeps) => {
                if keeps != self.keep.keeps {
                    self.keep.keeps = keeps;
                    // The pack list's marks.
                    self.entries_revision += 1;
                }
                Vec::new()
            }
            Event::OfflineChecked { generation, outcome } => self.offline_checked(generation, outcome),
            Event::OfflineFiles => self.check_offline().into_iter().collect(),
            Event::Reachable(r) => self.set_reachable(r),
            Event::Keep { scope, keep } => self.keep_from_outside(scope, keep),
            Event::KeptReasons { item_id, outcome } => {
                if let Some(st) = &mut self.keep.menu {
                    if st.item.as_deref() == Some(item_id.as_str()) {
                        st.reasons = Some(outcome);
                    }
                }
                Vec::new()
            }
            Event::Estimated { scope, outcome } => self.estimated(scope, outcome),
            Event::KeepDone { scope, keep, outcome } => self.keep_done(scope, keep, outcome),
            Event::AskRemoveKept { files, bytes } => {
                self.ask(Ask::RemoveKept { files, bytes });
                Vec::new()
            }
            Event::KeptRemoved(outcome) => {
                let status = match outcome {
                    Ok((files, bytes)) => {
                        self.keep.keeps.clear();
                        self.entries_revision += 1;
                        Status::KeptRemoved { files, bytes }
                    }
                    Err(e) => Status::KeepFailed(e),
                };
                if self.copying.is_none() {
                    self.status = status;
                }
                self.check_offline().into_iter().collect()
            }
            // Answered by `question_takes`; without a question there is nothing to answer.
            Event::Confirm(_) => Vec::new(),
            Event::OpenMenu(index) => self.open_menu(index),
            Event::MenuChosen(action) => self.menu_chosen(action),
            // `input_closes_menu` closed it.
            Event::CloseMenu => Vec::new(),
            _ => Vec::new(),
        }
    }

    /// `K`: keep or stop keeping the selected sticker, or pack on the pack list.
    pub(super) fn keep_key(&mut self) -> Vec<Command> {
        match self.view() {
            View::Items => match self.selected_entry() {
                Some(e) => {
                    let (scope, name) = (KeepScope::Item(e.item.id.clone()), e.item.name.clone());
                    let keep = !self.keep.keeps.contains(&scope);
                    self.set_keep(scope, name, keep)
                }
                None => self.nothing_selected(),
            },
            View::Packs => match self.selected_pack() {
                Some(set) => {
                    let (id, name) = (set.id.clone(), set.name.clone());
                    self.toggle_pack(id, name)
                }
                None => self.nothing_selected(),
            },
            View::Page => Vec::new(),
        }
    }

    fn nothing_selected(&mut self) -> Vec<Command> {
        if self.copying.is_none() {
            self.status = Status::NothingSelected;
        }
        Vec::new()
    }

    fn toggle_pack(&mut self, id: String, name: String) -> Vec<Command> {
        let scope = KeepScope::Set(id);
        if self.keep.keeps.contains(&scope) {
            self.set_keep(scope, name, false)
        } else {
            self.ask_keep(scope, name)
        }
    }

    /// The Settings page's switch (or any keep from outside the grids).
    fn keep_from_outside(&mut self, scope: KeepScope, keep: bool) -> Vec<Command> {
        let name = match &scope {
            KeepScope::Item(id) => self
                .entries
                .iter()
                .find(|e| &e.item.id == id)
                .map_or_else(|| id.clone(), |e| e.item.name.clone()),
            KeepScope::Set(id) => self
                .packs
                .sets
                .items
                .iter()
                .find(|s| &s.id == id)
                .map_or_else(|| id.clone(), |s| s.name.clone()),
            KeepScope::Library => String::new(),
        };
        match (&scope, keep) {
            (KeepScope::Item(_), true) | (_, false) => self.set_keep(scope, name, keep),
            _ => self.ask_keep(scope, name),
        }
    }

    fn set_keep(&mut self, scope: KeepScope, name: String, keep: bool) -> Vec<Command> {
        self.keep.names.retain(|(s, _)| *s != scope);
        self.keep.names.push((scope.clone(), name));
        vec![Command::SetKeep { scope, keep }]
    }

    /// A pack or the library: what it takes first.
    fn ask_keep(&mut self, scope: KeepScope, name: String) -> Vec<Command> {
        if self.copying.is_none() {
            self.status = Status::Estimating {
                name: match scope {
                    KeepScope::Library => t("the whole library").into(),
                    _ => name.clone(),
                },
            };
        }
        self.keep.estimating = Some((scope.clone(), name));
        vec![Command::EstimateKeep { scope }]
    }

    fn estimated(&mut self, scope: KeepScope, outcome: Result<Estimate, String>) -> Vec<Command> {
        let Some((_, name)) = self.keep.estimating.take_if(|(s, _)| *s == scope) else {
            return Vec::new();
        };
        match outcome {
            Ok(estimate) => {
                if self.copying.is_none() {
                    self.status = self.view_status();
                }
                self.ask(Ask::Keep { scope, name, estimate });
            }
            Err(e) => {
                if self.copying.is_none() {
                    self.status = Status::KeepFailed(e);
                }
            }
        }
        Vec::new()
    }

    /// Ask: the question takes the keys (Normal mode, out of the text field).
    fn ask(&mut self, ask: Ask) {
        if self.keep.ask_mode.is_none() {
            self.keep.ask_mode = Some(self.mode);
        }
        self.keep.ask = Some(ask);
        self.keep.menu = None;
        self.help = None;
        self.set_mode(Mode::Normal);
    }

    fn answer(&mut self, yes: bool) -> Vec<Command> {
        let Some(ask) = self.keep.ask.take() else {
            return Vec::new();
        };
        if let Some(mode) = self.keep.ask_mode.take() {
            self.set_mode(mode);
        }
        if !yes {
            if self.copying.is_none() {
                self.status = self.view_status();
            }
            return Vec::new();
        }
        match ask {
            Ask::Keep { scope, name, .. } => self.set_keep(scope, name, true),
            Ask::RemoveKept { .. } => vec![Command::RemoveKept],
        }
    }

    fn keep_done(&mut self, scope: KeepScope, keep: bool, outcome: Result<(), String>) -> Vec<Command> {
        let name = match self.keep.names.iter().position(|(s, _)| *s == scope) {
            Some(i) => self.keep.names.remove(i).1,
            None => String::new(),
        };
        let status = match outcome {
            Ok(()) => {
                if keep && !self.keep.keeps.contains(&scope) {
                    self.keep.keeps.push(scope.clone());
                } else if !keep {
                    self.keep.keeps.retain(|s| *s != scope);
                }
                // The pack list's marks.
                self.entries_revision += 1;
                let what = match scope {
                    KeepScope::Item(_) => Kept::Item(name),
                    KeepScope::Set(_) => Kept::Pack(name),
                    KeepScope::Library => Kept::Library,
                };
                Status::Kept { what, keep }
            }
            Err(e) => Status::KeepFailed(e),
        };
        if self.copying.is_none() {
            self.status = status;
        }
        self.check_offline().into_iter().collect()
    }

    // -- the menu --------------------------------------------------------------------------

    fn open_menu(&mut self, index: Option<usize>) -> Vec<Command> {
        let Some(i) = index.or(self.selected) else {
            return Vec::new();
        };
        match self.view() {
            View::Items => {
                let Some(item) = self.entries.get(i).map(|e| e.item.clone()) else {
                    return Vec::new();
                };
                self.selected = Some(i);
                self.touched = true;
                self.keep.menu = Some(MenuState {
                    index: i,
                    item: Some(item.id.clone()),
                    reasons: None,
                });
                vec![Command::LoadKeptReasons { item: Box::new(item) }]
            }
            View::Packs if i < self.packs.shown.len() => {
                self.selected = Some(i);
                self.touched = true;
                self.keep.menu = Some(MenuState {
                    index: i,
                    item: None,
                    reasons: Some(Ok(Vec::new())),
                });
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn menu_chosen(&mut self, action: MenuAction) -> Vec<Command> {
        let Some(menu) = self.menu() else {
            self.keep.menu = None;
            return Vec::new();
        };
        self.keep.menu = None;
        if !menu.entries.iter().any(|e| e.action == Some(action)) {
            return Vec::new();
        }
        let i = menu.index;
        self.selected = Some(i);
        match action {
            MenuAction::Copy => self.copy_at(i, false),
            MenuAction::CopyFlipped => self.copy_at(i, true),
            MenuAction::Favorite => self.toggle_favorite(),
            MenuAction::Browser => self.open_in_browser(),
            MenuAction::Keep | MenuAction::Release => match self.entries.get(i) {
                Some(e) => {
                    let (scope, name) = (KeepScope::Item(e.item.id.clone()), e.item.name.clone());
                    self.set_keep(scope, name, action == MenuAction::Keep)
                }
                None => Vec::new(),
            },
            MenuAction::OpenPack => self.open_pack_at(i),
            MenuAction::KeepPack | MenuAction::ReleasePack => {
                let set = match self.view() {
                    View::Packs => self.packs.shown.get(i),
                    _ => self.packs.open.as_ref().map(|o| &o.set),
                };
                match set.map(|s| (s.id.clone(), s.name.clone())) {
                    Some((id, name)) if action == MenuAction::KeepPack => self.ask_keep(KeepScope::Set(id), name),
                    Some((id, name)) => self.set_keep(KeepScope::Set(id), name, false),
                    None => Vec::new(),
                }
            }
        }
    }

    // -- marks -----------------------------------------------------------------------------

    fn set_reachable(&mut self, reachable: bool) -> Vec<Command> {
        if self.keep.reachable == reachable {
            return Vec::new();
        }
        self.keep.reachable = reachable;
        self.mark_offline();
        self.check_offline().into_iter().collect()
    }

    /// Search results say whether the server answered; `true` when that
    /// is news.
    pub(super) fn results_reached(&mut self, offline: bool) -> bool {
        let changed = self.keep.reachable == offline;
        self.keep.reachable = !offline;
        changed
    }

    /// Ask whose files are on the device, for the shown items, when the
    /// marks need it; one check at a time.
    pub(super) fn check_offline(&mut self) -> Option<Command> {
        let sticker = self.sticker && supports_stickers(self.target);
        if self.keep.avail_for != (self.target, sticker) {
            self.keep.available.clear();
            self.keep.avail_for = (self.target, sticker);
            self.mark_offline();
        }
        let wanted = self.tiles.offline || !self.keep.reachable;
        if !self.visible || !wanted || self.view() != View::Items || self.entries.is_empty() {
            return None;
        }
        if self.keep.checking.is_some() {
            self.keep.recheck = true;
            return None;
        }
        self.keep.checks += 1;
        self.keep.checking = Some((self.keep.checks, self.target, sticker));
        Some(Command::CheckOffline {
            items: self.entries.iter().map(|e| e.item.clone()).collect(),
            target: self.target,
            sticker,
            generation: self.keep.checks,
        })
    }

    fn offline_checked(&mut self, generation: u64, outcome: Result<Vec<(String, bool)>, String>) -> Vec<Command> {
        let Some((_, target, sticker)) = self.keep.checking.take_if(|(g, _, _)| *g == generation) else {
            return Vec::new();
        };
        if let Ok(list) = outcome {
            if (target, sticker) == self.keep.avail_for {
                self.keep.available.extend(list);
                self.mark_offline();
            }
        }
        if std::mem::take(&mut self.keep.recheck) {
            return self.check_offline().into_iter().collect();
        }
        Vec::new()
    }

    /// The tiles' marks from what is known: while the server answers, the
    /// mark of what is available offline (`zo`); while it does not, the
    /// tiles whose file is not here are dimmed and the others are plain
    /// (as on Android).
    pub(super) fn mark_offline(&mut self) {
        let mut changed = false;
        let reachable = self.keep.reachable;
        for e in &mut self.entries {
            let mark = match self.keep.available.get(&e.item.id) {
                Some(true) if self.tiles.offline && reachable => OfflineMark::Available,
                Some(false) if !reachable => OfflineMark::Unavailable,
                _ => OfflineMark::None,
            };
            if e.offline != mark {
                e.offline = mark;
                changed = true;
            }
        }
        if changed {
            self.entries_revision += 1;
        }
    }

    /// Another account: nothing known about its files.
    pub(super) fn forget_offline(&mut self) {
        self.keep.available.clear();
        self.keep.menu = None;
        self.keep.estimating = None;
    }
}
