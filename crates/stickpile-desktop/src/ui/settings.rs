//! The Settings tab: a form generated from `settings_schema()`, grouped by
//! `GROUPS`. Every change is written at once (`set_value`, `unset_value` for
//! a reset); the shell then reloads the config and applies it live. Above
//! the "Offline" group, the "On this device" block: keep everything, what
//! is kept, the downloader, clear the cache, remove the kept files.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use stickpile_core::settings::{self, LayeredConfig, SettingKind, SettingSpec, GROUPS};
use stickpile_i18n::global::{t, t_dyn};
use stickpile_quick::PageKey;

use crate::offline::PanelView;
use crate::pages;
use crate::ui::nav::{Control, PageNav, Stop};
use crate::ui::{set_class, Handler};

/// What the page asks the shell to do.
pub enum SettingsAction {
    Set(&'static str, toml::Value),
    /// Remove the key from the file the page writes, so the default (or
    /// the value of a lower layer) applies again.
    Reset(&'static str),
    /// A control of the "On this device" block.
    Offline(OfflineAction),
}

/// The "On this device" block's controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfflineAction {
    /// The "Keep everything offline" switch.
    KeepLibrary(bool),
    Pause,
    Resume,
    Cancel,
    ClearCache,
    RemoveKept,
}

/// The "On this device" block (`crate::offline::Panel`).
struct OfflineBlock {
    group: adw::PreferencesGroup,
    library_row: adw::ActionRow,
    library: gtk::Switch,
    usage: adw::ActionRow,
    state_row: gtk::ListBoxRow,
    state: gtk::Label,
    progress: gtk::ProgressBar,
    notes: gtk::Label,
    pause: gtk::Button,
    resume: gtk::Button,
    cancel: gtk::Button,
    cache_row: adw::ActionRow,
    clear: gtk::Button,
    remove: gtk::Button,
}

fn text_button() -> gtk::Button {
    let b = gtk::Button::builder().valign(gtk::Align::Center).build();
    b.add_css_class("flat");
    b
}

impl OfflineBlock {
    fn new() -> OfflineBlock {
        let group = adw::PreferencesGroup::new();
        let library = gtk::Switch::builder().valign(gtk::Align::Center).build();
        let library_row = adw::ActionRow::new();
        library_row.add_suffix(&library);
        library_row.set_activatable_widget(Some(&library));
        group.add(&library_row);
        let usage = adw::ActionRow::new();
        usage.set_title_lines(2);
        group.add(&usage);

        let state = caption(&[]);
        let progress = gtk::ProgressBar::new();
        let notes = caption(&["dim-label", "caption"]);
        let (pause, resume, cancel) = (text_button(), text_button(), text_button());
        let buttons = gtk::Box::builder().spacing(6).halign(gtk::Align::End).build();
        buttons.append(&pause);
        buttons.append(&resume);
        buttons.append(&cancel);
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(10)
            .margin_bottom(10)
            .margin_start(12)
            .margin_end(12)
            .build();
        column.append(&state);
        column.append(&progress);
        column.append(&notes);
        column.append(&buttons);
        let state_row = gtk::ListBoxRow::builder().activatable(false).child(&column).build();
        group.add(&state_row);

        let (clear, remove) = (text_button(), text_button());
        remove.add_css_class("destructive-action");
        let cache_row = adw::ActionRow::new();
        cache_row.add_suffix(&clear);
        cache_row.add_suffix(&remove);
        group.add(&cache_row);
        OfflineBlock {
            group,
            library_row,
            library,
            usage,
            state_row,
            state,
            progress,
            notes,
            pause,
            resume,
            cancel,
            cache_row,
            clear,
            remove,
        }
    }

    fn relabel(&self) {
        self.group.set_title(t("On this device"));
        self.library_row.set_title(t("Keep everything offline"));
        self.library_row.set_subtitle(t(
            "Every sticker you can see, now and later, within the limit of the Offline settings below. Asks first, with what it takes.",
        ));
        self.pause.set_label(t("Pause"));
        self.resume.set_label(t("Resume"));
        self.cancel.set_label(t("Cancel"));
        self.cache_row.set_title(t("Cached files"));
        self.clear.set_label(t("Clear cache"));
        self.clear.set_tooltip_text(Some(t(
            "Delete the thumbnails and copied files that are not kept; kept files stay",
        )));
        self.remove.set_label(t("Remove kept files"));
        self.remove
            .set_tooltip_text(Some(t("Delete every kept file and forget what to keep (asks first)")));
    }

    fn show(&self, v: &PanelView) {
        if self.library.is_active() != v.library {
            self.library.set_active(v.library);
        }
        self.usage.set_title(&v.kept);
        self.usage.set_subtitle(&v.other);
        self.usage.set_visible(!v.kept.is_empty());
        self.state.set_label(&v.state);
        set_class(&self.state, "warning", v.state_warning);
        self.progress.set_visible(v.fraction.is_some());
        self.progress.set_fraction(v.fraction.unwrap_or(0.0));
        let notes: Vec<&str> = [v.failures.as_str(), v.skipped.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        self.notes.set_label(&notes.join("\n"));
        self.notes.set_visible(!notes.is_empty());
        self.pause.set_visible(v.can_pause);
        self.resume.set_visible(v.can_resume);
        self.cancel.set_visible(v.can_cancel);
        self.state_row.set_visible(!v.state.is_empty() || v.can_resume);
        self.remove.set_sensitive(v.can_remove);
        let message = v
            .message
            .clone()
            .unwrap_or_else(|| t("Clearing leaves the kept files; removing them asks first.").to_string());
        self.cache_row.set_subtitle(&message);
        set_class(&self.cache_row, "stickpile-error-row", v.message_error);
    }

    fn stops(&self) -> Vec<Stop> {
        let mut stops = vec![Stop::new(&self.library_row, Control::Switch(self.library.clone()))];
        for b in [&self.pause, &self.resume, &self.cancel] {
            stops.push(Stop::new(&self.state_row, Control::Button(b.clone())));
        }
        stops.push(Stop::new(&self.cache_row, Control::Button(self.clear.clone())));
        stops.push(Stop::new(&self.cache_row, Control::Button(self.remove.clone())));
        stops
    }
}

enum Input {
    Switch(gtk::Switch),
    Choice(gtk::DropDown, &'static [&'static str]),
    Number(gtk::SpinButton),
    Text(gtk::Entry),
}

struct Row {
    spec: SettingSpec,
    row: adw::ActionRow,
    input: Input,
    reset: gtk::Button,
}

impl Row {
    fn control(&self) -> Control {
        match &self.input {
            Input::Switch(w) => Control::Switch(w.clone()),
            Input::Choice(w, _) => Control::Choice(w.clone()),
            Input::Number(w) => Control::Number(w.clone()),
            Input::Text(w) => Control::Text(w.clone().upcast()),
        }
    }

    fn widget(&self) -> gtk::Widget {
        self.control().widget()
    }

    /// The value the control shows now.
    fn value(&self) -> Option<toml::Value> {
        Some(match &self.input {
            Input::Switch(w) => toml::Value::Boolean(w.is_active()),
            Input::Choice(w, names) => toml::Value::String(names.get(w.selected() as usize)?.to_string()),
            Input::Number(w) => toml::Value::Integer(w.value().round() as i64),
            Input::Text(w) => toml::Value::String(w.text().trim().to_string()),
        })
    }

    fn set(&self, value: Option<&toml::Value>) {
        match (&self.input, value) {
            (Input::Switch(w), Some(toml::Value::Boolean(b))) => w.set_active(*b),
            (Input::Choice(w, names), Some(toml::Value::String(s))) => {
                if let Some(i) = names.iter().position(|n| n == s) {
                    w.set_selected(i as u32);
                }
            }
            (Input::Number(w), Some(toml::Value::Integer(i))) => w.set_value(*i as f64),
            (Input::Text(w), Some(toml::Value::String(s))) => {
                // Being edited (a value that was not saved): the text stays to be fixed.
                let editing = w.state_flags().contains(gtk::StateFlags::FOCUS_WITHIN);
                if !editing && w.text() != s.as_str() {
                    w.set_text(s);
                }
            }
            _ => {}
        }
    }
}

pub struct SettingsPage {
    /// The page, with save errors shown as toasts over it.
    pub root: adw::ToastOverlay,
    nav: PageNav,
    note: gtk::Label,
    warnings: gtk::Label,
    error: gtk::Label,
    rows: Vec<Row>,
    /// The groups' boxes, for their titles.
    groups: Vec<(&'static str, adw::PreferencesGroup)>,
    /// Filling the controls from the config: their signals are not edits.
    updating: Cell<bool>,
    /// The values last shown, to tell an edit from a repeated signal.
    shown: RefCell<Vec<Option<toml::Value>>>,
    write_path: RefCell<PathBuf>,
    on_action: RefCell<Option<Handler<SettingsAction>>>,
    /// The "On this device" block, above the "Offline" group.
    offline: OfflineBlock,
}

fn caption(css: &[&str]) -> gtk::Label {
    let l = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .build();
    for c in css {
        l.add_css_class(c);
    }
    l
}

impl SettingsPage {
    pub fn new() -> Rc<SettingsPage> {
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .margin_top(14)
            .margin_bottom(18)
            .margin_start(12)
            .margin_end(12)
            .build();
        let note = caption(&["dim-label", "caption"]);
        let warnings = caption(&["warning", "caption"]);
        let error = caption(&["error"]);
        column.append(&note);
        column.append(&warnings);
        column.append(&error);

        let schema = settings::settings_schema();
        let mut rows = Vec::new();
        let mut groups = Vec::new();
        let offline = OfflineBlock::new();
        for group in GROUPS {
            if *group == "Offline" {
                column.append(&offline.group);
            }
            let g = adw::PreferencesGroup::new();
            for spec in schema.iter().filter(|s| s.group == *group) {
                let row = adw::ActionRow::new();
                let input = input_for(spec);
                let reset = gtk::Button::builder()
                    .icon_name("edit-undo-symbolic")
                    .valign(gtk::Align::Center)
                    .build();
                reset.add_css_class("flat");
                row.add_suffix(&reset);
                let widget = match &input {
                    Input::Switch(w) => w.clone().upcast::<gtk::Widget>(),
                    Input::Choice(w, _) => w.clone().upcast(),
                    Input::Number(w) => w.clone().upcast(),
                    Input::Text(w) => w.clone().upcast(),
                };
                widget.set_valign(gtk::Align::Center);
                row.add_suffix(&widget);
                if let Input::Switch(sw) = &input {
                    row.set_activatable_widget(Some(sw));
                }
                g.add(&row);
                rows.push(Row {
                    spec: spec.clone(),
                    row,
                    input,
                    reset,
                });
            }
            column.append(&g);
            groups.push((*group, g));
        }

        let clamp = adw::Clamp::builder().maximum_size(720).child(&column).build();
        let viewport = gtk::Viewport::builder().child(&clamp).build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&viewport)
            .build();
        let root = adw::ToastOverlay::new();
        root.set_child(Some(&scroller));
        let n = rows.len();
        let page = Rc::new(SettingsPage {
            root,
            nav: PageNav::new(&viewport),
            note,
            warnings,
            error,
            rows,
            groups,
            updating: Cell::new(false),
            shown: RefCell::new(vec![None; n]),
            write_path: RefCell::new(PathBuf::new()),
            on_action: RefCell::new(None),
            offline,
        });
        page.relabel();
        page.connect_signals();
        page
    }

    /// Titles, help and choice names in the current language; the next
    /// [`SettingsPage::show`] adds the lock notes again.
    pub fn relabel(&self) {
        self.updating.set(true);
        self.offline.relabel();
        for (group, g) in &self.groups {
            g.set_title(&t_dyn(group));
        }
        for row in &self.rows {
            row.row.set_title(&t_dyn(row.spec.label));
            row.row.set_subtitle(&t_dyn(row.spec.help));
            row.reset.set_tooltip_text(Some(t("Reset to the default")));
            if let Input::Choice(dd, names) = &row.input {
                let selected = dd.selected();
                let labels: Vec<String> = names.iter().map(|n| pages::choice_label(row.spec.key, n)).collect();
                let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
                dd.set_model(Some(&gtk::StringList::new(&labels)));
                dd.set_selected(selected);
            }
        }
        self.updating.set(false);
    }

    pub fn connect_action(&self, f: impl Fn(SettingsAction) + 'static) {
        self.on_action.replace(Some(Rc::new(f)));
    }

    fn connect_signals(self: &Rc<Self>) {
        self.connect_offline();
        for (i, row) in self.rows.iter().enumerate() {
            let weak = Rc::downgrade(self);
            let changed = move || {
                if let Some(p) = weak.upgrade() {
                    p.changed(i);
                }
            };
            match &row.input {
                Input::Switch(w) => {
                    w.connect_active_notify(move |_| changed());
                }
                Input::Choice(w, _) => {
                    w.connect_selected_notify(move |_| changed());
                }
                Input::Number(w) => {
                    w.connect_value_changed(move |_| changed());
                }
                Input::Text(w) => {
                    // Committed on Enter or when the field loses the focus.
                    let on_enter = changed.clone();
                    w.connect_activate(move |_| on_enter());
                    // Not when the whole window loses the focus: the text
                    // may be half typed.
                    let focus = gtk::EventControllerFocus::new();
                    focus.connect_leave(move |ctl| {
                        let active = ctl
                            .widget()
                            .and_then(|w| w.root())
                            .and_downcast::<gtk::Window>()
                            .is_some_and(|w| w.is_active());
                        if active {
                            changed();
                        }
                    });
                    w.add_controller(focus);
                }
            }
            let weak = Rc::downgrade(self);
            let key = row.spec.key;
            row.reset.connect_clicked(move |_| {
                if let Some(p) = weak.upgrade() {
                    p.emit(SettingsAction::Reset(key));
                }
            });
        }
    }

    fn emit(&self, action: SettingsAction) {
        let f = self.on_action.borrow().clone();
        if let Some(f) = f {
            f(action);
        }
    }

    fn changed(&self, i: usize) {
        if self.updating.get() {
            return;
        }
        let row = &self.rows[i];
        let Some(value) = row.value() else { return };
        if self.shown.borrow()[i].as_ref() == Some(&value) {
            return;
        }
        tracing::debug!("setting {} = {value}", row.spec.key);
        self.emit(SettingsAction::Set(row.spec.key, value));
    }

    /// The file edits go to.
    pub fn write_path(&self) -> PathBuf {
        self.write_path.borrow().clone()
    }

    /// Fill the form from a freshly loaded config.
    pub fn show(&self, layered: &LayeredConfig, config_path: &Path) {
        self.updating.set(true);
        self.write_path.replace(layered.write_path.clone());
        let home = std::env::var_os("HOME").map(PathBuf::from);
        self.note
            .set_label(&pages::save_note(config_path, &layered.write_path, home.as_deref()));
        self.note
            .set_tooltip_text(Some(&layered.write_path.display().to_string()));
        let warnings = layered.warnings.join("\n");
        self.warnings.set_label(&warnings);
        self.warnings.set_visible(!warnings.is_empty());
        let reason = pages::lock_reason(config_path);
        let mut shown = self.shown.borrow_mut();
        for (i, row) in self.rows.iter().enumerate() {
            let value = settings::get_value(&layered.config, row.spec.key);
            row.set(value.as_ref());
            shown[i] = value.clone();
            let locked = layered.locked.contains(row.spec.key);
            row.widget().set_sensitive(!locked);
            let help = t_dyn(row.spec.help);
            row.row
                .set_subtitle(&if locked { format!("{help} {reason}.") } else { help });
            set_class(&row.row, "stickpile-locked", locked);
            row.reset
                .set_visible(!locked && value.as_ref() != Some(&row.spec.default));
        }
        drop(shown);
        self.updating.set(false);
        self.nav.refresh(&self.stops());
    }

    /// The config cannot be read (`None`: it can).
    pub fn set_error(&self, error: Option<&str>) {
        self.error.set_label(error.unwrap_or_default());
        self.error.set_visible(error.is_some());
    }

    /// A change could not be saved.
    pub fn save_failed(&self, error: &str) {
        let toast = adw::Toast::builder().title(error).timeout(6).build();
        toast.set_use_markup(false);
        self.root.add_toast(toast);
    }

    // -- keys -------------------------------------------------------------------------------

    fn stops(&self) -> Vec<Stop> {
        let mut stops = Vec::new();
        let mut block = Some(self.offline.stops());
        for row in &self.rows {
            // The block sits above the "Offline" group.
            if row.spec.group == "Offline" {
                stops.extend(block.take().unwrap_or_default());
            }
            stops.push(Stop::new(&row.row, row.control()));
            stops.push(Stop::new(&row.reset, Control::Button(row.reset.clone())));
        }
        stops.extend(block.unwrap_or_default());
        stops
    }

    /// The "On this device" block's controls ask the shell.
    fn connect_offline(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.offline.library.connect_active_notify(move |sw| {
            if let Some(p) = weak.upgrade() {
                if !p.updating.get() {
                    p.emit(SettingsAction::Offline(OfflineAction::KeepLibrary(sw.is_active())));
                }
            }
        });
        let b = &self.offline;
        for (button, action) in [
            (&b.pause, OfflineAction::Pause),
            (&b.resume, OfflineAction::Resume),
            (&b.cancel, OfflineAction::Cancel),
            (&b.clear, OfflineAction::ClearCache),
            (&b.remove, OfflineAction::RemoveKept),
        ] {
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(p) = weak.upgrade() {
                    p.emit(SettingsAction::Offline(action));
                }
            });
        }
    }

    /// Draw the "On this device" block.
    pub fn show_offline(&self, v: &PanelView) {
        self.updating.set(true);
        self.offline.show(v);
        self.updating.set(false);
        self.nav.refresh(&self.stops());
    }

    pub fn key(&self, key: PageKey) {
        self.nav.key(&self.stops(), key);
    }

    pub fn show_cursor(&self, on: bool) {
        self.nav.show(&self.stops(), on);
    }

    pub fn follow(&self, focus: &gtk::Widget) {
        self.nav.follow(&self.stops(), focus);
    }
}

fn input_for(spec: &SettingSpec) -> Input {
    match &spec.kind {
        SettingKind::Bool => Input::Switch(gtk::Switch::new()),
        SettingKind::Choice(names) => {
            let labels: Vec<String> = names.iter().map(|n| pages::choice_label(spec.key, n)).collect();
            let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
            let dd = gtk::DropDown::from_strings(&labels);
            Input::Choice(dd, names)
        }
        SettingKind::Int { min, max } => {
            let spin = gtk::SpinButton::with_range(*min as f64, *max as f64, 1.0);
            spin.set_digits(0);
            spin.set_numeric(true);
            spin.set_width_chars(6);
            Input::Number(spin)
        }
        SettingKind::Text => {
            let e = gtk::Entry::builder().width_chars(26).build();
            Input::Text(e)
        }
    }
}
