//! The Settings tab: a form generated from `settings_schema()`, grouped by
//! `GROUPS`. Every change is written at once (`set_value`, `unset_value` for
//! a reset); the shell then reloads the config and applies it live.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use stickpile_core::settings::{self, LayeredConfig, SettingKind, SettingSpec, GROUPS};
use stickpile_quick::PageKey;

use crate::pages;
use crate::ui::nav::{Control, PageNav, Stop};
use crate::ui::{set_class, Handler};

/// What the page asks the shell to do.
pub enum SettingsAction {
    Set(&'static str, toml::Value),
    /// Remove the key from the file the page writes, so the default (or
    /// the value of a lower layer) applies again.
    Reset(&'static str),
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
    /// Filling the controls from the config: their signals are not edits.
    updating: Cell<bool>,
    /// The values last shown, to tell an edit from a repeated signal.
    shown: RefCell<Vec<Option<toml::Value>>>,
    write_path: RefCell<PathBuf>,
    on_action: RefCell<Option<Handler<SettingsAction>>>,
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
        for group in GROUPS {
            let g = adw::PreferencesGroup::builder().title(*group).build();
            for spec in schema.iter().filter(|s| s.group == *group) {
                let row = adw::ActionRow::builder().title(spec.label).subtitle(spec.help).build();
                let input = input_for(spec);
                let reset = gtk::Button::builder()
                    .icon_name("edit-undo-symbolic")
                    .tooltip_text("Reset to the default")
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
            updating: Cell::new(false),
            shown: RefCell::new(vec![None; n]),
            write_path: RefCell::new(PathBuf::new()),
            on_action: RefCell::new(None),
        });
        page.connect_signals();
        page
    }

    pub fn connect_action(&self, f: impl Fn(SettingsAction) + 'static) {
        self.on_action.replace(Some(Rc::new(f)));
    }

    fn connect_signals(self: &Rc<Self>) {
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
            row.row.set_subtitle(&if locked {
                format!("{} {reason}.", row.spec.help)
            } else {
                row.spec.help.to_string()
            });
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
        for row in &self.rows {
            stops.push(Stop::new(&row.row, row.control()));
            stops.push(Stop::new(&row.reset, Control::Button(row.reset.clone())));
        }
        stops
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
