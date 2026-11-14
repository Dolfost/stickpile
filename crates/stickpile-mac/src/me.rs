//! The Me tab's state: the profile, the login form, and the answers to a
//! login or logout. The same decisions as the GTK page (`ui/me.rs` of
//! `stickpile-desktop`), drawn by the app from [`MeView`].

use stickpile_desktop::backend::{AuthKind, LoginError, Profile};
use stickpile_desktop::pages;
use stickpile_i18n::global::{t, t_dyn, tf};

use crate::bridge::{MeAccount, MeForm, MeView, Message, Stat};

#[derive(Debug, Default)]
pub struct MeState {
    profile: Option<Profile>,
    loading: bool,
    /// Fetching the profile failed.
    error: Option<String>,
    /// The outcome of the last login or logout, `(text, is_error)`.
    message: Option<(String, bool)>,
    totp: bool,
    busy: bool,
    clear_secrets: bool,
}

impl MeState {
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// The code field is shown: the server asked for a two-factor code.
    pub fn wants_totp(&self) -> bool {
        self.totp
    }

    /// A profile request went out.
    pub fn loading(&mut self) {
        self.loading = true;
    }

    /// The server or the account changed: what is shown is about another one.
    pub fn forget(&mut self) {
        self.profile = None;
        self.error = None;
        self.message = None;
        self.totp = false;
    }

    pub fn profile_loaded(&mut self, outcome: Result<Profile, String>) {
        self.loading = false;
        match outcome {
            Ok(p) => {
                self.profile = Some(p);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// The form was sent without a username or a password.
    pub fn missing_credentials(&mut self) {
        self.message = Some((t("Enter a username and a password").into(), true));
    }

    /// A login (`logout: false`) or logout starts; `false` while one runs.
    pub fn start(&mut self, logout: bool) -> bool {
        if self.busy {
            return false;
        }
        self.busy = true;
        let text = if logout {
            t("Logging out...")
        } else {
            t("Logging in...")
        };
        self.message = Some((text.into(), false));
        true
    }

    pub fn login_finished(&mut self, outcome: Result<Profile, LoginError>) {
        self.busy = false;
        match outcome {
            Ok(p) => {
                self.clear_secrets = p.auth != AuthKind::None;
                self.profile = Some(p);
                self.error = None;
                self.totp = false;
                self.message = None;
            }
            Err(LoginError::TotpRequired) => {
                let first = !self.totp;
                self.totp = true;
                self.message = Some((t("Enter the code from your authenticator app").into(), !first));
            }
            Err(LoginError::Failed(e)) => self.message = Some((e, true)),
        }
    }

    pub fn logout_finished(&mut self, outcome: Result<(), String>) {
        self.busy = false;
        self.message = outcome
            .err()
            .map(|e| (tf("Logout failed: {error}", &[("error", &t_dyn(&e))]), true));
    }

    /// What the page shows; a pending "clear the secret fields" is handed out once.
    pub fn view(&mut self) -> MeView {
        let profile = self.profile.as_ref();
        let signed_in = profile.is_some_and(|p| p.auth != AuthKind::None);
        let signed_out = profile.is_some_and(|p| p.auth == AuthKind::None);
        let account = profile.filter(|_| signed_in).map(|p| MeAccount {
            server_url: p.server_url.clone(),
            open_server: p.server_url.starts_with("http://") || p.server_url.starts_with("https://"),
            auth: pages::auth_text(p).to_string(),
            tailnet: p.tailscale_login.clone(),
            two_factor: if p.totp_enabled { t("On") } else { t("Off") }.to_string(),
            stats: pages::stat_tiles(p)
                .into_iter()
                .map(|(value, label)| Stat {
                    value,
                    label: label.to_string(),
                })
                .collect(),
            logout: p.auth == AuthKind::Token,
            store_token: p.auth == AuthKind::Tailnet,
        });
        let form = profile.filter(|_| signed_out).map(|p| MeForm {
            description: tf("Server: {server}", &[("server", &p.server_url)]),
            totp: self.totp,
        });
        let message = match (&self.message, &self.error) {
            (Some((m, e)), _) => Some(Message {
                text: m.clone(),
                error: *e,
            }),
            (None, Some(e)) => Some(Message {
                text: tf("Could not load the profile: {error}", &[("error", &t_dyn(e))]),
                error: true,
            }),
            (None, None) => None,
        };
        MeView {
            loading: self.loading && profile.is_none(),
            who: profile.map(pages::who),
            // Signed out, the form says which server.
            detail: profile.filter(|_| signed_in).map(pages::who_detail).unwrap_or_default(),
            account,
            form,
            // Part of the signed-out form, as on the GTK page.
            tailnet_login: signed_out && profile.is_some_and(|p| p.tailnet_login_available),
            busy: self.busy,
            message,
            retry: self.error.is_some() && !self.loading && self.message.is_none(),
            clear_secrets: std::mem::take(&mut self.clear_secrets),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_out() -> Profile {
        Profile::signed_out("http://stickpile.example.ts.net")
    }

    fn signed_in() -> Profile {
        Profile {
            auth: AuthKind::Token,
            username: Some("bob".into()),
            ..signed_out()
        }
    }

    #[test]
    fn login_flow_with_a_code() {
        let mut s = MeState::default();
        s.loading();
        assert!(s.view().loading);
        s.profile_loaded(Ok(signed_out()));
        let v = s.view();
        assert!(v.form.is_some() && v.account.is_none() && !v.loading);
        assert!(s.start(false));
        assert!(!s.start(false), "one at a time");
        assert!(s.view().busy);
        s.login_finished(Err(LoginError::TotpRequired));
        let v = s.view();
        assert!(v.form.unwrap().totp);
        assert!(!v.message.unwrap().error, "the first ask is not an error");
        s.start(false);
        s.login_finished(Err(LoginError::TotpRequired));
        assert!(s.view().message.unwrap().error, "asked again: the code was wrong");
        s.start(false);
        s.login_finished(Ok(signed_in()));
        let v = s.view();
        assert!(v.clear_secrets && v.account.unwrap().logout && v.form.is_none());
        assert!(!s.view().clear_secrets, "once");
    }

    #[test]
    fn failures_are_shown() {
        let mut s = MeState::default();
        s.profile_loaded(Err("Server unreachable".into()));
        let v = s.view();
        assert!(v.retry && v.message.unwrap().error);
        s.profile_loaded(Ok(signed_in()));
        s.start(true);
        s.logout_finished(Err("boom".into()));
        assert!(s.view().message.unwrap().text.contains("boom"));
    }
}
