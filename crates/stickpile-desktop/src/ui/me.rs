//! The Me tab: who the popup is signed in as, and login / logout.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::glib;
use stickpile_quick::PageKey;

use crate::backend::{AuthKind, Credentials, LoginError, Profile};
use crate::pages;
use crate::ui::nav::{Control, PageNav, Stop};
use crate::ui::{set_class, Handler};

/// What the page asks the shell to do.
pub enum MeAction {
    /// `None`: with the tailnet identity.
    Login(Option<Credentials>),
    Logout,
    /// Fetch the profile again.
    Retry,
    /// Open this URL in the browser.
    Open(String),
}

#[derive(Default)]
struct State {
    profile: Option<Profile>,
    loading: bool,
    /// Fetching the profile failed.
    error: Option<String>,
    /// The outcome of the last login or logout, `(text, is_error)`.
    message: Option<(String, bool)>,
    totp: bool,
    busy: bool,
}

pub struct MePage {
    pub root: gtk::ScrolledWindow,
    nav: PageNav,
    state: RefCell<State>,
    on_action: RefCell<Option<Handler<MeAction>>>,

    loading: gtk::Box,
    avatar: adw::Avatar,
    name: gtk::Label,
    detail: gtk::Label,
    header: gtk::Box,

    account: adw::PreferencesGroup,
    server_row: adw::ActionRow,
    open_server: gtk::Button,
    auth_row: adw::ActionRow,
    tailnet_row: adw::ActionRow,
    totp_row: adw::ActionRow,
    stats: gtk::FlowBox,
    signed_in_buttons: gtk::Box,
    logout: gtk::Button,
    keep_token: gtk::Button,

    form: adw::PreferencesGroup,
    username: adw::EntryRow,
    password: adw::PasswordEntryRow,
    totp: adw::EntryRow,
    signed_out_buttons: gtk::Box,
    login: gtk::Button,
    tailnet_login: gtk::Button,

    message: gtk::Label,
    retry: gtk::Button,
}

fn button(label: &str) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.add_css_class("pill");
    b
}

impl MePage {
    pub fn new() -> Rc<MePage> {
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(14)
            .margin_top(14)
            .margin_bottom(18)
            .margin_start(12)
            .margin_end(12)
            .build();

        let loading = gtk::Box::builder().spacing(8).halign(gtk::Align::Center).build();
        loading.append(&adw::Spinner::new());
        loading.append(&gtk::Label::new(Some("Loading...")));
        column.append(&loading);

        let avatar = adw::Avatar::new(48, None, true);
        let name = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        name.add_css_class("title-2");
        let detail = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        detail.add_css_class("dim-label");
        let names = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .spacing(2)
            .build();
        names.append(&name);
        names.append(&detail);
        let header = gtk::Box::builder().spacing(12).build();
        header.append(&avatar);
        header.append(&names);
        column.append(&header);

        let account = adw::PreferencesGroup::new();
        let row = |title: &str| {
            let r = adw::ActionRow::builder().title(title).subtitle_selectable(true).build();
            r.add_css_class("property");
            r
        };
        let server_row = row("Server");
        let open_server = gtk::Button::builder()
            .icon_name("web-browser-symbolic")
            .tooltip_text("Open the web UI")
            .valign(gtk::Align::Center)
            .build();
        open_server.add_css_class("flat");
        server_row.add_suffix(&open_server);
        let auth_row = row("Signed in");
        let tailnet_row = row("Tailnet account");
        let totp_row = row("Two-factor login");
        for r in [&server_row, &auth_row, &tailnet_row, &totp_row] {
            account.add(r);
        }
        column.append(&account);

        let stats = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .min_children_per_line(4)
            .max_children_per_line(4)
            .column_spacing(8)
            .row_spacing(8)
            .can_focus(false)
            .build();
        column.append(&stats);

        let logout = button("Log out");
        logout.add_css_class("destructive-action");
        let keep_token = button("Store a token");
        keep_token.set_tooltip_text(Some(
            "Turn the tailnet identity into a token, so this device stays signed in off the tailnet",
        ));
        let signed_in_buttons = gtk::Box::builder().spacing(8).build();
        signed_in_buttons.append(&logout);
        signed_in_buttons.append(&keep_token);
        column.append(&signed_in_buttons);

        let form = adw::PreferencesGroup::builder().title("Log in").build();
        let username = adw::EntryRow::builder().title("Username").build();
        username.set_input_purpose(gtk::InputPurpose::FreeForm);
        let password = adw::PasswordEntryRow::builder().title("Password").build();
        let totp = adw::EntryRow::builder().title("Two-factor code").build();
        totp.set_input_purpose(gtk::InputPurpose::Digits);
        form.add(&username);
        form.add(&password);
        form.add(&totp);
        column.append(&form);

        let login = button("Log in");
        login.add_css_class("suggested-action");
        let tailnet_login = button("Log in with tailnet");
        let signed_out_buttons = gtk::Box::builder().spacing(8).build();
        signed_out_buttons.append(&login);
        signed_out_buttons.append(&tailnet_login);
        column.append(&signed_out_buttons);

        let message = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .build();
        column.append(&message);
        let retry = button("Try again");
        retry.set_halign(gtk::Align::Start);
        column.append(&retry);

        let clamp = adw::Clamp::builder().maximum_size(640).child(&column).build();
        let viewport = gtk::Viewport::builder().child(&clamp).build();
        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&viewport)
            .build();

        let page = Rc::new(MePage {
            root,
            nav: PageNav::new(&viewport),
            state: RefCell::new(State {
                loading: true,
                ..State::default()
            }),
            on_action: RefCell::new(None),
            loading,
            avatar,
            name,
            detail,
            header,
            account,
            server_row,
            open_server,
            auth_row,
            tailnet_row,
            totp_row,
            stats,
            signed_in_buttons,
            logout,
            keep_token,
            form,
            username,
            password,
            totp,
            signed_out_buttons,
            login,
            tailnet_login,
            message,
            retry,
        });
        page.connect_signals();
        page.render();
        page
    }

    pub fn connect_action(&self, f: impl Fn(MeAction) + 'static) {
        self.on_action.replace(Some(Rc::new(f)));
    }

    fn emit(&self, action: MeAction) {
        let f = self.on_action.borrow().clone();
        if let Some(f) = f {
            f(action);
        }
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let with = move |f: fn(&MePage)| {
            let weak: Weak<MePage> = weak.clone();
            move || {
                if let Some(p) = weak.upgrade() {
                    f(&p);
                }
            }
        };
        let f = with(MePage::submit);
        self.login.connect_clicked(move |_| f());
        let f = with(|p| p.start(MeAction::Login(None)));
        self.tailnet_login.connect_clicked(move |_| f());
        let f = with(|p| p.start(MeAction::Login(None)));
        self.keep_token.connect_clicked(move |_| f());
        let f = with(|p| p.start(MeAction::Logout));
        self.logout.connect_clicked(move |_| f());
        let f = with(|p| p.emit(MeAction::Retry));
        self.retry.connect_clicked(move |_| f());
        let f = with(|p| {
            let url = p.state.borrow().profile.as_ref().map(|pr| pr.server_url.clone());
            if let Some(url) = url {
                p.emit(MeAction::Open(url));
            }
        });
        self.open_server.connect_clicked(move |_| f());

        // Enter moves on through the form, then logs in.
        let f = with(|p| {
            p.password.grab_focus();
        });
        self.username.connect_entry_activated(move |_| f());
        let f = with(|p| {
            if p.totp.is_visible() && p.totp.text().is_empty() {
                p.totp.grab_focus();
            } else {
                p.submit();
            }
        });
        self.password.connect_entry_activated(move |_| f());
        let f = with(MePage::submit);
        self.totp.connect_entry_activated(move |_| f());
    }

    /// The password form's "Log in".
    fn submit(&self) {
        let username = self.username.text().trim().to_string();
        let password = self.password.text().to_string();
        if username.is_empty() || password.is_empty() {
            self.state.borrow_mut().message = Some(("Enter a username and a password".into(), true));
            self.render();
            return;
        }
        let totp = Some(self.totp.text().trim().to_string()).filter(|t| self.totp.is_visible() && !t.is_empty());
        self.start(MeAction::Login(Some(Credentials {
            username,
            password,
            totp,
        })));
    }

    fn start(&self, action: MeAction) {
        {
            let mut s = self.state.borrow_mut();
            if s.busy {
                return;
            }
            s.busy = true;
            s.message = Some((
                match action {
                    MeAction::Logout => "Logging out...",
                    _ => "Logging in...",
                }
                .into(),
                false,
            ));
        }
        self.render();
        self.emit(action);
    }

    // -- answers from the shell ---------------------------------------------------------

    /// A profile request went out (the tab was shown, or the server changed).
    pub fn loading(&self) {
        self.state.borrow_mut().loading = true;
        self.render();
    }

    /// The server or the token file changed: what is shown is about another account.
    pub fn forget(&self) {
        let mut s = self.state.borrow_mut();
        s.profile = None;
        s.error = None;
        s.message = None;
        s.totp = false;
        drop(s);
        self.render();
    }

    pub fn profile_loaded(&self, outcome: Result<Profile, String>) {
        {
            let mut s = self.state.borrow_mut();
            s.loading = false;
            match outcome {
                Ok(p) => {
                    s.profile = Some(p);
                    s.error = None;
                }
                Err(e) => s.error = Some(e),
            }
        }
        self.render();
    }

    pub fn login_finished(&self, outcome: Result<Profile, LoginError>) {
        let totp_now = {
            let mut s = self.state.borrow_mut();
            s.busy = false;
            match outcome {
                Ok(p) => {
                    s.profile = Some(p);
                    s.error = None;
                    s.totp = false;
                    s.message = None;
                    false
                }
                Err(LoginError::TotpRequired) => {
                    let first = !s.totp;
                    s.totp = true;
                    s.message = Some(("Enter the code from your authenticator app".into(), !first));
                    true
                }
                Err(LoginError::Failed(e)) => {
                    s.message = Some((e, true));
                    false
                }
            }
        };
        let signed_in = self
            .state
            .borrow()
            .profile
            .as_ref()
            .is_some_and(|p| p.auth != AuthKind::None);
        if signed_in {
            self.password.set_text("");
            self.totp.set_text("");
            self.nav.reset();
        }
        self.render();
        if totp_now {
            // Typing in the form goes on in the code field; in Normal mode
            // the cursor moves there.
            let typing = self
                .root
                .root()
                .and_then(|r| r.focus())
                .is_some_and(|f| f.is::<gtk::Text>());
            if typing {
                self.totp.grab_focus();
            } else {
                self.nav.follow(&self.stops(), &self.totp.clone().upcast());
            }
        }
    }

    pub fn logout_finished(&self, outcome: Result<(), String>) {
        {
            let mut s = self.state.borrow_mut();
            s.busy = false;
            s.message = outcome.err().map(|e| (format!("Logout failed: {e}"), true));
        }
        self.nav.reset();
        self.render();
    }

    // -- keys -------------------------------------------------------------------------------

    fn stops(&self) -> Vec<Stop> {
        let button = |b: &gtk::Button| Stop::new(b, Control::Button(b.clone()));
        let text = |r: &adw::EntryRow| Stop::new(r, Control::Text(r.clone().upcast()));
        vec![
            Stop::new(&self.server_row, Control::Button(self.open_server.clone())),
            button(&self.logout),
            button(&self.keep_token),
            text(&self.username),
            text(self.password.upcast_ref()),
            text(&self.totp),
            button(&self.login),
            button(&self.tailnet_login),
            button(&self.retry),
        ]
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

    // -- drawing ------------------------------------------------------------------------------

    fn render(&self) {
        let s = self.state.borrow();
        let profile = s.profile.as_ref();
        let signed_in = profile.is_some_and(|p| p.auth != AuthKind::None);
        let signed_out = profile.is_some_and(|p| p.auth == AuthKind::None);

        self.loading.set_visible(s.loading && profile.is_none());
        self.header.set_visible(profile.is_some());
        if let Some(p) = profile {
            let who = pages::who(p);
            self.name.set_label(&who);
            self.avatar.set_text(signed_in.then_some(who.as_str()));
            self.avatar.set_icon_name(Some("avatar-default-symbolic"));
            // Signed out, the form says which server.
            let detail = if signed_in { pages::who_detail(p) } else { String::new() };
            self.detail.set_label(&detail);
            self.detail.set_visible(!detail.is_empty());
        }

        self.account.set_visible(signed_in);
        self.stats.set_visible(signed_in);
        self.signed_in_buttons.set_visible(signed_in);
        if let Some(p) = profile.filter(|_| signed_in) {
            self.server_row.set_subtitle(&p.server_url);
            self.open_server
                .set_visible(p.server_url.starts_with("http://") || p.server_url.starts_with("https://"));
            self.auth_row.set_subtitle(pages::auth_text(p));
            self.tailnet_row.set_visible(p.tailscale_login.is_some());
            self.tailnet_row
                .set_subtitle(p.tailscale_login.as_deref().unwrap_or_default());
            self.totp_row.set_subtitle(if p.totp_enabled { "On" } else { "Off" });
            while let Some(child) = self.stats.first_child() {
                self.stats.remove(&child);
            }
            for (value, caption) in pages::stat_tiles(p) {
                self.stats.append(&stat_tile(&value, caption));
            }
            self.logout.set_visible(p.auth == AuthKind::Token);
            self.keep_token.set_visible(p.auth == AuthKind::Tailnet);
        }

        self.form.set_visible(signed_out);
        self.signed_out_buttons.set_visible(signed_out);
        self.totp.set_visible(s.totp);
        self.tailnet_login
            .set_visible(profile.is_some_and(|p| p.tailnet_login_available));
        if let Some(p) = profile.filter(|_| signed_out) {
            self.form
                .set_description(Some(&format!("Server: {}", glib::markup_escape_text(&p.server_url))));
        }
        for b in [&self.login, &self.tailnet_login, &self.logout, &self.keep_token] {
            b.set_sensitive(!s.busy);
        }

        let (text, error) = match (&s.message, &s.error) {
            (Some((m, e)), _) => (m.clone(), *e),
            (None, Some(e)) => (format!("Could not load the profile: {e}"), true),
            (None, None) => (String::new(), false),
        };
        self.message.set_label(&text);
        self.message.set_visible(!text.is_empty());
        set_class(&self.message, "error", error);
        set_class(&self.message, "dim-label", !error);
        self.retry
            .set_visible(s.error.is_some() && !s.loading && s.message.is_none());
        drop(s);
        self.nav.refresh(&self.stops());
    }
}

fn stat_tile(value: &str, caption: &str) -> gtk::Widget {
    let v = gtk::Label::new(Some(value));
    v.add_css_class("title-3");
    v.add_css_class("numeric");
    let c = gtk::Label::new(Some(caption));
    c.add_css_class("caption");
    c.add_css_class("dim-label");
    c.set_wrap(true);
    c.set_justify(gtk::Justification::Center);
    let b = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    b.add_css_class("card");
    b.add_css_class("stickpile-stat");
    b.append(&v);
    b.append(&c);
    let child = gtk::FlowBoxChild::builder().child(&b).can_focus(false).build();
    child.upcast()
}
