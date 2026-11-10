//! The spike window: pick a file, tick MIME types, copy (optionally quitting
//! after N seconds), drag the file out, and record what the target app did.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::fixtures::{self, Fixture};
use crate::payload::{self, MIME_TYPES, PRESETS};
use crate::record::{self, OUTCOMES, ROUTES, TARGETS};

pub struct GuiArgs {
    pub file: PathBuf,
    pub fixtures_dir: PathBuf,
    pub results: PathBuf,
    /// Initially ticked MIME types (empty: provisional default for the file).
    pub mimes: Vec<String>,
}

struct Ui {
    window: gtk::ApplicationWindow,
    file: RefCell<PathBuf>,
    fixtures_dir: PathBuf,
    results: PathBuf,
    session: record::Session,
    path_label: gtk::Label,
    info_label: gtk::Label,
    preview: gtk::Picture,
    checks: Vec<(&'static str, gtk::CheckButton)>,
    file_list: gtk::CheckButton,
    route: gtk::DropDown,
    target: gtk::DropDown,
    outcome: gtk::DropDown,
    notes: gtk::Entry,
    status: gtk::Label,
    last_offers: RefCell<String>,
}

pub fn run(args: GuiArgs) -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("io.github.dolfost.Clipspike")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let args = Rc::new(RefCell::new(Some(args)));
    app.connect_activate(move |app| {
        if let Some(args) = args.borrow_mut().take() {
            build(app, args);
        }
    });
    app.run_with_args(&["clipspike"])
}

fn section(title: &str, child: &impl IsA<gtk::Widget>) -> gtk::Frame {
    let frame = gtk::Frame::new(Some(title));
    child.as_ref().set_margin_top(6);
    child.as_ref().set_margin_bottom(6);
    child.as_ref().set_margin_start(8);
    child.as_ref().set_margin_end(8);
    frame.set_child(Some(child));
    frame
}

fn hbox() -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, 6)
}

fn build(app: &gtk::Application, args: GuiArgs) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("clipspike — phase 0")
        .default_width(760)
        .build();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_top(10);
    root.set_margin_bottom(10);
    root.set_margin_start(10);
    root.set_margin_end(10);

    // -- file -------------------------------------------------------------
    let path_label = gtk::Label::builder().xalign(0.0).selectable(true).wrap(true).build();
    let info_label = gtk::Label::builder().xalign(0.0).build();
    info_label.add_css_class("dim-label");
    let preview = gtk::Picture::builder()
        .width_request(160)
        .height_request(120)
        .can_shrink(true)
        .build();
    let open = gtk::Button::with_label("Open file…");
    let fixture_labels: Vec<&str> = Fixture::ALL.iter().map(|f| f.label()).collect();
    let fixture = gtk::DropDown::from_strings(&fixture_labels);
    let use_fixture = gtk::Button::with_label("Use fixture");
    let file_row = hbox();
    file_row.append(&open);
    file_row.append(&fixture);
    file_row.append(&use_fixture);
    let file_text = gtk::Box::new(gtk::Orientation::Vertical, 4);
    file_text.set_hexpand(true);
    file_text.append(&path_label);
    file_text.append(&info_label);
    file_text.append(&file_row);
    let file_box = hbox();
    file_box.append(&preview);
    file_box.append(&file_text);
    root.append(&section("File", &file_box));

    // -- offers -------------------------------------------------------------
    let offers_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let checks: Vec<(&'static str, gtk::CheckButton)> = MIME_TYPES
        .iter()
        .map(|m| (*m, gtk::CheckButton::with_label(m)))
        .collect();
    let grid = gtk::Grid::builder().column_spacing(12).row_spacing(2).build();
    for (i, (_, c)) in checks.iter().enumerate() {
        grid.attach(c, (i % 3) as i32, (i / 3) as i32, 1, 1);
    }
    offers_box.append(&grid);
    let file_list = gtk::CheckButton::with_label(
        "also a GdkFileList value (GDK adds text/uri-list and the portal file-transfer types)",
    );
    offers_box.append(&file_list);
    let mut preset_labels = vec!["provisional default (this file)", "provisional default (sticker)"];
    preset_labels.extend(PRESETS.iter().map(|(name, _)| *name));
    let preset = gtk::DropDown::from_strings(&preset_labels);
    let apply_preset = gtk::Button::with_label("Apply preset");
    let preset_row = hbox();
    preset_row.append(&gtk::Label::new(Some("Preset:")));
    preset_row.append(&preset);
    preset_row.append(&apply_preset);
    offers_box.append(&preset_row);
    root.append(&section("Offer these MIME types (in this order)", &offers_box));

    // -- actions -------------------------------------------------------------
    let copy = gtk::Button::with_label("Copy");
    copy.add_css_class("suggested-action");
    let quit_secs = gtk::SpinButton::with_range(1.0, 300.0, 1.0);
    quit_secs.set_value(5.0);
    let copy_quit = gtk::Button::with_label("Copy and quit after");
    let clear = gtk::Button::with_label("Clear clipboard");
    let inspect = gtk::Button::with_label("Inspect clipboard");
    let actions = hbox();
    actions.append(&copy);
    actions.append(&copy_quit);
    actions.append(&quit_secs);
    actions.append(&gtk::Label::new(Some("s")));
    actions.append(&clear);
    actions.append(&inspect);
    let drag_label = gtk::Label::new(Some(
        "⇲  Drag this into a chat window (GdkFileList + text/uri-list + gnome-copied-files)",
    ));
    drag_label.set_margin_top(14);
    drag_label.set_margin_bottom(14);
    let drag_frame = gtk::Frame::new(None);
    drag_frame.set_child(Some(&drag_label));
    let action_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    action_box.append(&actions);
    action_box.append(&drag_frame);
    root.append(&section("Copy / drag", &action_box));

    // -- recorder ---------------------------------------------------------------
    let session = record::current_session();
    let target = gtk::DropDown::from_strings(&TARGETS);
    let route = gtk::DropDown::from_strings(&ROUTES);
    let outcome = gtk::DropDown::from_strings(&OUTCOMES);
    let notes = gtk::Entry::builder()
        .placeholder_text("notes (e.g. 'sent as file, no preview')")
        .hexpand(true)
        .build();
    let record_btn = gtk::Button::with_label("Record");
    let rec_grid = gtk::Grid::builder().column_spacing(8).row_spacing(4).build();
    let label = |s: &str| gtk::Label::builder().label(s).xalign(0.0).build();
    rec_grid.attach(&label("Session"), 0, 0, 1, 1);
    rec_grid.attach(&label(&session.describe()), 1, 0, 3, 1);
    rec_grid.attach(&label("Target"), 0, 1, 1, 1);
    rec_grid.attach(&target, 1, 1, 1, 1);
    rec_grid.attach(&label("Route"), 2, 1, 1, 1);
    rec_grid.attach(&route, 3, 1, 1, 1);
    rec_grid.attach(&label("Result"), 0, 2, 1, 1);
    rec_grid.attach(&outcome, 1, 2, 1, 1);
    rec_grid.attach(&notes, 2, 2, 1, 1);
    rec_grid.attach(&record_btn, 3, 2, 1, 1);
    root.append(&section(
        &format!("Record result → {}", args.results.display()),
        &rec_grid,
    ));

    let status = gtk::Label::builder().xalign(0.0).wrap(true).selectable(true).build();
    root.append(&status);
    window.set_child(Some(&root));

    let ui = Rc::new(Ui {
        window: window.clone(),
        file: RefCell::new(PathBuf::new()),
        fixtures_dir: args.fixtures_dir,
        results: args.results,
        session,
        path_label,
        info_label,
        preview,
        checks,
        file_list,
        route,
        target,
        outcome,
        notes,
        status,
        last_offers: RefCell::new(String::new()),
    });
    ui.set_file(&args.file);
    if !args.mimes.is_empty() {
        let wanted: Vec<&str> = args.mimes.iter().map(String::as_str).collect();
        ui.tick(&wanted);
    }

    // -- signals --------------------------------------------------------------------
    let u = ui.clone();
    open.connect_clicked(move |_| {
        let dialog = gtk::FileDialog::builder().title("Media file to offer").build();
        let u2 = u.clone();
        dialog.open(Some(&u.window), gio::Cancellable::NONE, move |res| {
            if let Ok(Some(p)) = res.map(|f| f.path()) {
                u2.set_file(&p);
            }
        });
    });
    let u = ui.clone();
    use_fixture.connect_clicked(move |_| {
        let f = Fixture::ALL[fixture.selected() as usize % Fixture::ALL.len()];
        match fixtures::generate(f, &u.fixtures_dir) {
            Ok(p) => u.set_file(&p),
            Err(e) => u.say(&format!("fixture: {e:#}")),
        }
    });
    let u = ui.clone();
    apply_preset.connect_clicked(move |_| {
        let i = preset.selected() as usize;
        let kind = u.kind();
        let mimes: Vec<&str> = match i {
            0 => payload::provisional_default(kind, false).to_vec(),
            1 => payload::provisional_default(kind, true).to_vec(),
            n => PRESETS.get(n - 2).map(|(_, m)| m.to_vec()).unwrap_or_default(),
        };
        u.tick(&mimes);
    });
    let u = ui.clone();
    copy.connect_clicked(move |_| {
        if u.copy() {
            u.route.set_selected(0);
        }
    });
    let u = ui.clone();
    let app2 = app.clone();
    copy_quit.connect_clicked(move |_| {
        if u.copy() {
            let secs = quit_secs.value() as u32;
            u.route.set_selected(1);
            u.say(&format!(
                "{} — quitting in {secs} s; paste after that to test persistence",
                u.status.label()
            ));
            let app = app2.clone();
            glib::timeout_add_seconds_local_once(secs, move || app.quit());
        }
    });
    let u = ui.clone();
    clear.connect_clicked(move |_| {
        let _ = u.window.clipboard().set_content(gdk::ContentProvider::NONE);
        u.say("clipboard cleared");
    });
    let u = ui.clone();
    inspect.connect_clicked(move |_| {
        let formats = u.window.clipboard().formats();
        let mimes: Vec<String> = formats.mime_types().iter().map(|m| m.to_string()).collect();
        let types: Vec<String> = formats.types().iter().map(|t| t.name().to_string()).collect();
        u.say(&format!("clipboard offers: {} {}", mimes.join(", "), types.join(", ")));
    });
    let u = ui.clone();
    record_btn.connect_clicked(move |_| u.record());

    let source = gtk::DragSource::new();
    source.set_actions(gdk::DragAction::COPY);
    let u = ui.clone();
    source.connect_prepare(move |_, _, _| {
        let file = u.file.borrow().clone();
        let mut providers = vec![gdk::ContentProvider::for_value(
            &gdk::FileList::from_array(&[gio::File::for_path(&file)]).to_value(),
        )];
        let (offers, _) = payload::build(&file, &["text/uri-list", "x-special/gnome-copied-files"]).ok()?;
        providers.extend(
            offers
                .iter()
                .map(|o| gdk::ContentProvider::for_bytes(&o.mime, &glib::Bytes::from(&o.data[..]))),
        );
        *u.last_offers.borrow_mut() = "GdkFileList, text/uri-list, x-special/gnome-copied-files".into();
        Some(gdk::ContentProvider::new_union(&providers))
    });
    let u = ui.clone();
    source.connect_drag_end(move |_, _, _| {
        u.route.set_selected(2);
        u.say("drag finished — record what the target did");
    });
    drag_frame.add_controller(source);

    window.present();
}

impl Ui {
    fn say(&self, text: &str) {
        self.status.set_label(text);
    }

    fn kind(&self) -> payload::Kind {
        std::fs::read(&*self.file.borrow())
            .map(|b| payload::sniff(&b))
            .unwrap_or(payload::Kind::Other)
    }

    fn tick(&self, mimes: &[&str]) {
        for (m, c) in &self.checks {
            c.set_active(mimes.contains(m));
        }
    }

    fn selected(&self) -> Vec<&'static str> {
        self.checks
            .iter()
            .filter(|(_, c)| c.is_active())
            .map(|(m, _)| *m)
            .collect()
    }

    fn set_file(&self, path: &Path) {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        *self.file.borrow_mut() = path.clone();
        self.path_label.set_label(&path.display().to_string());
        let bytes = std::fs::read(&path).unwrap_or_default();
        let kind = payload::sniff(&bytes);
        let mut info = format!("{:?}, {} KiB", kind, bytes.len() / 1024);
        match image::load_from_memory(&bytes) {
            Ok(img) => {
                info.push_str(&format!(", {}×{}", img.width(), img.height()));
                let rgba = img.thumbnail(320, 320).to_rgba8();
                let (w, h) = (rgba.width() as i32, rgba.height() as i32);
                let tex = gdk::MemoryTexture::new(
                    w,
                    h,
                    gdk::MemoryFormat::R8g8b8a8,
                    &glib::Bytes::from_owned(rgba.into_raw()),
                    w as usize * 4,
                );
                self.preview.set_paintable(Some(&tex));
            }
            Err(_) => self.preview.set_paintable(gdk::Paintable::NONE),
        }
        self.info_label.set_label(&info);
        self.tick(payload::provisional_default(kind, false));
    }

    /// Put the ticked offers on the clipboard. Returns false on error.
    fn copy(&self) -> bool {
        let file = self.file.borrow().clone();
        let mimes = self.selected();
        let (offers, skipped) = match payload::build(&file, &mimes) {
            Ok(x) => x,
            Err(e) => {
                self.say(&format!("reading {}: {e}", file.display()));
                return false;
            }
        };
        let mut providers: Vec<gdk::ContentProvider> = offers
            .iter()
            .map(|o| gdk::ContentProvider::for_bytes(&o.mime, &glib::Bytes::from(&o.data[..])))
            .collect();
        let mut summary = payload::summary(&offers);
        if self.file_list.is_active() {
            let list = gdk::FileList::from_array(&[gio::File::for_path(&file)]);
            providers.push(gdk::ContentProvider::for_value(&list.to_value()));
            summary.push_str(if summary.is_empty() {
                "GdkFileList"
            } else {
                ", GdkFileList"
            });
        }
        if providers.is_empty() {
            self.say("nothing to offer: tick at least one MIME type");
            return false;
        }
        if let Err(e) = self
            .window
            .clipboard()
            .set_content(Some(&gdk::ContentProvider::new_union(&providers)))
        {
            self.say(&format!("set_content failed: {e}"));
            return false;
        }
        let mut msg = format!("copied at {}: {summary}", record::now_utc());
        if !skipped.is_empty() {
            msg.push_str(&format!(" (cannot produce: {})", skipped.join(", ")));
        }
        *self.last_offers.borrow_mut() = summary;
        self.say(&msg);
        true
    }

    fn record(&self) {
        let file = self.file.borrow().clone();
        let size = std::fs::metadata(&file).map(|m| m.len() / 1024).unwrap_or(0);
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let pick =
            |d: &gtk::DropDown, list: &[&str]| list.get(d.selected() as usize).copied().unwrap_or("").to_string();
        let row = record::Row {
            when: record::now_utc(),
            session: self.session.describe(),
            route: pick(&self.route, &ROUTES),
            file: format!("{name} ({:?}, {size} KiB)", self.kind()),
            offers: self.last_offers.borrow().clone(),
            target: pick(&self.target, &TARGETS),
            outcome: pick(&self.outcome, &OUTCOMES),
            notes: self.notes.text().to_string(),
        };
        match record::append(&self.results, &row) {
            Ok(()) => {
                self.notes.set_text("");
                self.say(&format!("recorded to {}", self.results.display()));
            }
            Err(e) => self.say(&format!("recording to {}: {e}", self.results.display())),
        }
    }
}
