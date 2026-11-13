//! The keyboard cursor of the Me and Settings pages. In Normal mode no
//! widget has the keyboard focus there, so the page keeps its own cursor:
//! one highlighted control that `Command::Page` keys move and act on.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use stickpile_quick::PageKey;

use crate::pages;

/// What a stop of the cursor is, and so what the page keys do to it.
#[derive(Clone)]
pub enum Control {
    Button(gtk::Button),
    Switch(gtk::Switch),
    Choice(gtk::DropDown),
    Number(gtk::SpinButton),
    /// A text field (an entry or an entry row).
    Text(gtk::Widget),
}

impl Control {
    pub fn widget(&self) -> gtk::Widget {
        match self {
            Control::Button(w) => w.clone().upcast(),
            Control::Switch(w) => w.clone().upcast(),
            Control::Choice(w) => w.clone().upcast(),
            Control::Number(w) => w.clone().upcast(),
            Control::Text(w) => w.clone(),
        }
    }

    fn editable(&self) -> bool {
        matches!(self, Control::Number(_) | Control::Text(_))
    }
}

/// One stop of the cursor.
#[derive(Clone)]
pub struct Stop {
    /// Highlighted and scrolled into view.
    pub row: gtk::Widget,
    pub control: Control,
}

impl Stop {
    pub fn new(row: &impl IsA<gtk::Widget>, control: Control) -> Stop {
        Stop {
            row: row.clone().upcast(),
            control,
        }
    }

    /// The control and every container up to the page are visible.
    fn shown(&self, page: &gtk::Widget) -> bool {
        let mut widget = Some(self.control.widget());
        while let Some(w) = widget {
            if !w.is_visible() {
                return false;
            }
            if &w == page {
                break;
            }
            widget = w.parent();
        }
        self.row.is_visible()
    }
}

pub struct PageNav {
    viewport: gtk::Viewport,
    /// The control the cursor is on; the first stop when it is gone.
    cursor: RefCell<Option<gtk::Widget>>,
    highlighted: RefCell<Option<gtk::Widget>>,
    /// The cursor is drawn (modes on, the page shown).
    on: Cell<bool>,
}

const CURSOR_CLASS: &str = "stickpile-cursor";

impl PageNav {
    pub fn new(viewport: &gtk::Viewport) -> PageNav {
        PageNav {
            viewport: viewport.clone(),
            cursor: RefCell::new(None),
            highlighted: RefCell::new(None),
            on: Cell::new(false),
        }
    }

    fn shown<'a>(&self, stops: &'a [Stop]) -> Vec<&'a Stop> {
        let page = self.viewport.upcast_ref();
        stops.iter().filter(|s| s.shown(page)).collect()
    }

    /// Position of the cursor among the `shown` stops of `all`. A cursor
    /// on a stop that is hidden now (a reset button after the reset) falls
    /// back to the nearest shown stop before it.
    fn index(&self, all: &[Stop], shown: &[&Stop]) -> usize {
        let cursor = self.cursor.borrow();
        let Some(cursor) = cursor.as_ref() else {
            return 0;
        };
        let position = |w: &gtk::Widget| shown.iter().position(|s| &s.control.widget() == w);
        if let Some(i) = position(cursor) {
            return i;
        }
        let Some(at) = all.iter().position(|s| &s.control.widget() == cursor) else {
            return 0;
        };
        all[..at]
            .iter()
            .rev()
            .find_map(|s| position(&s.control.widget()))
            .unwrap_or(0)
    }

    /// Back to the first stop (the page changed its content).
    pub fn reset(&self) {
        self.cursor.replace(None);
    }

    /// A page key in Normal mode.
    pub fn key(&self, all: &[Stop], key: PageKey) {
        let stops = self.shown(all);
        if stops.is_empty() {
            return;
        }
        let i = self.index(all, &stops);
        let stop = stops[i];
        let usable = stop.control.widget().is_sensitive();
        tracing::debug!(
            "page key {key:?} on stop {i} ({})",
            stop.control.widget().type_().name()
        );
        match key {
            PageKey::Up | PageKey::Down => {
                if let Some(j) = pages::step(stops.len(), Some(i), key == PageKey::Down) {
                    self.cursor.replace(Some(stops[j].control.widget()));
                }
            }
            PageKey::Left | PageKey::Right if usable => {
                let forward = key == PageKey::Right;
                match &stop.control {
                    Control::Choice(dd) => {
                        let n = dd.model().map_or(0, |m| m.n_items());
                        let cur = dd.selected();
                        let next = match (forward, cur) {
                            (_, gtk::INVALID_LIST_POSITION) => 0,
                            (true, c) => (c + 1).min(n.saturating_sub(1)),
                            (false, c) => c.saturating_sub(1),
                        };
                        if n > 0 && next != cur {
                            dd.set_selected(next);
                        }
                    }
                    Control::Number(spin) => {
                        let dir = if forward {
                            gtk::SpinType::StepForward
                        } else {
                            gtk::SpinType::StepBackward
                        };
                        spin.spin(dir, 1.0);
                    }
                    Control::Switch(sw) => sw.set_active(forward),
                    Control::Button(_) | Control::Text(_) => {}
                }
            }
            PageKey::Activate if usable => match &stop.control {
                Control::Button(b) => b.emit_clicked(),
                Control::Switch(sw) => sw.set_active(!sw.is_active()),
                Control::Choice(dd) => dd.emit_activate(),
                Control::Number(_) | Control::Text(_) => {
                    stop.control.widget().grab_focus();
                }
            },
            PageKey::Edit if usable && stop.control.editable() => {
                stop.control.widget().grab_focus();
            }
            _ => {}
        }
        self.show(all, true);
    }

    /// Draw the cursor (`on`), or take it away.
    pub fn show(&self, stops: &[Stop], on: bool) {
        self.on.set(on);
        self.refresh(stops);
    }

    /// Draw the cursor again after the page changed what it shows.
    pub fn refresh(&self, stops: &[Stop]) {
        let shown = self.shown(stops);
        let i = self.index(stops, &shown);
        let row = if self.on.get() {
            shown.get(i).map(|s| s.row.clone())
        } else {
            None
        };
        let mut highlighted = self.highlighted.borrow_mut();
        if *highlighted == row {
            return;
        }
        if let Some(old) = highlighted.take() {
            old.remove_css_class(CURSOR_CLASS);
        }
        if let Some(row) = &row {
            row.add_css_class(CURSOR_CLASS);
            // On the first stop, the page's top (notes, warnings) shows too.
            match self.viewport.vadjustment().filter(|_| i == 0) {
                Some(adj) => adj.set_value(adj.lower()),
                None => self.viewport.scroll_to(row, None),
            }
        }
        *highlighted = row;
    }

    /// The keyboard focus moved (a click, a field taking the focus): the
    /// cursor follows it.
    pub fn follow(&self, stops: &[Stop], focus: &gtk::Widget) {
        if let Some(stop) = stops.iter().find(|s| {
            let w = s.control.widget();
            focus == &w || focus.is_ancestor(&w)
        }) {
            self.cursor.replace(Some(stop.control.widget()));
        }
    }
}
