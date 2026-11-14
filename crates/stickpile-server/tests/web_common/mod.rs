//! Helpers for the web UI tests: a cookie-keeping "browser" that signs in
//! through the login form, form/htmx/multipart requests carrying the CSRF
//! token, and HTML hygiene assertions (CSP: no inline styles or scripts).
#![allow(dead_code)]

use reqwest::header::HeaderMap;
use reqwest::StatusCode;

use crate::common::{read, Resp, TestServer, TestUser};

pub struct Browser {
    pub client: reqwest::Client,
    pub base: String,
    pub csrf: Option<String>,
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

impl Browser {
    /// A browser without a session.
    pub fn anonymous(server: &TestServer) -> Browser {
        Browser {
            client: client(),
            base: server.base.clone(),
            csrf: None,
        }
    }

    /// Sign in through `POST /login` and pick up the CSRF token.
    pub async fn login(server: &TestServer, user: &TestUser) -> Browser {
        let mut b = Browser::anonymous(server);
        let r = b
            .post_raw("/login", &[("username", &user.username), ("password", &user.password)])
            .await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "login failed: {}", r.text());
        b.refresh_csrf().await;
        b
    }

    pub async fn refresh_csrf(&mut self) {
        let home = self.get("/").await.expect(200);
        self.csrf = Some(csrf_from(&home.text()).expect("data-csrf on <body>"));
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    pub async fn get(&self, path: &str) -> Resp {
        read(self.client.get(self.url(path)).send().await.unwrap()).await
    }

    /// GET as htmx does it.
    pub async fn get_htmx(&self, path: &str) -> Resp {
        read(
            self.client
                .get(self.url(path))
                .header("HX-Request", "true")
                .send()
                .await
                .unwrap(),
        )
        .await
    }

    /// Form POST without adding a CSRF token.
    pub async fn post_raw(&self, path: &str, fields: &[(&str, &str)]) -> Resp {
        read(self.client.post(self.url(path)).form(fields).send().await.unwrap()).await
    }

    /// Form POST with the `csrf_token` field, like a plain HTML form.
    pub async fn post_form(&self, path: &str, fields: &[(&str, &str)]) -> Resp {
        let mut all: Vec<(&str, &str)> = Vec::new();
        if let Some(t) = &self.csrf {
            all.push(("csrf_token", t));
        }
        all.extend_from_slice(fields);
        self.post_raw(path, &all).await
    }

    /// POST as htmx does it: `HX-Request` plus the `X-CSRF-Token` header.
    pub async fn post_htmx(&self, path: &str, fields: &[(&str, &str)]) -> Resp {
        let mut rb = self
            .client
            .post(self.url(path))
            .header("HX-Request", "true")
            .form(fields);
        if let Some(t) = &self.csrf {
            rb = rb.header("X-CSRF-Token", t);
        }
        read(rb.send().await.unwrap()).await
    }

    /// [`Self::post_htmx`] from the page at `current` (`HX-Current-URL`).
    pub async fn post_htmx_from(&self, path: &str, fields: &[(&str, &str)], current: &str) -> Resp {
        let mut rb = self
            .client
            .post(self.url(path))
            .header("HX-Request", "true")
            .header("HX-Current-URL", self.url(current))
            .form(fields);
        if let Some(t) = &self.csrf {
            rb = rb.header("X-CSRF-Token", t);
        }
        read(rb.send().await.unwrap()).await
    }

    /// Multipart POST like a plain form: the CSRF token is the first field
    /// when `with_token`.
    pub async fn post_multipart(&self, path: &str, files: Vec<(&str, &str, Vec<u8>)>, with_token: bool) -> Resp {
        let mut form = reqwest::multipart::Form::new();
        if with_token {
            form = form.text("csrf_token", self.csrf.clone().unwrap_or_default());
        }
        for (field, name, bytes) in files {
            form = form.part(
                field.to_string(),
                reqwest::multipart::Part::bytes(bytes).file_name(name.to_string()),
            );
        }
        read(self.client.post(self.url(path)).multipart(form).send().await.unwrap()).await
    }

    /// Multipart POST with leading text fields (after the token).
    pub async fn post_multipart_fields(
        &self,
        path: &str,
        fields: &[(&str, &str)],
        files: Vec<(&str, &str, Vec<u8>)>,
    ) -> Resp {
        let mut form = reqwest::multipart::Form::new();
        form = form.text("csrf_token", self.csrf.clone().unwrap_or_default());
        for (k, v) in fields {
            form = form.text(k.to_string(), v.to_string());
        }
        for (field, name, bytes) in files {
            form = form.part(
                field.to_string(),
                reqwest::multipart::Part::bytes(bytes).file_name(name.to_string()),
            );
        }
        read(self.client.post(self.url(path)).multipart(form).send().await.unwrap()).await
    }
}

/// The CSRF token from `<body data-csrf="...">`.
pub fn csrf_from(html: &str) -> Option<String> {
    attr_after(html, "data-csrf=\"")
}

/// Text after `marker` up to the next `"`.
pub fn attr_after(html: &str, marker: &str) -> Option<String> {
    let start = html.find(marker)? + marker.len();
    let end = html[start..].find('"')? + start;
    Some(html[start..end].to_string())
}

/// Value of the hidden input `name`.
pub fn hidden_value(html: &str, name: &str) -> Option<String> {
    let marker = format!("name=\"{name}\" value=\"");
    attr_after(html, &marker)
}

pub fn location(r: &Resp) -> String {
    r.header("location").unwrap_or_default()
}

pub fn set_cookies(h: &HeaderMap) -> Vec<String> {
    h.get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(str::to_string)
        .collect()
}

/// CSP hygiene: no `style` attributes, no `<style>` blocks, no inline
/// scripts, no inline event handlers.
#[track_caller]
pub fn assert_clean_html(html: &str) {
    let lower = html.to_ascii_lowercase();
    assert!(!lower.contains("<style"), "inline <style> block");
    for (i, tag) in lower.split('<').enumerate().skip(1) {
        let tag = tag.split('>').next().unwrap_or("");
        if tag.starts_with('/') || tag.starts_with('!') {
            continue;
        }
        if tag.starts_with("script") {
            assert!(tag.contains(" src="), "inline <script> (tag #{i}): <{tag}>");
        }
        for token in tag.split_whitespace().skip(1) {
            let name = token.split('=').next().unwrap_or("");
            assert!(name != "style", "style attribute in <{tag}>");
            assert!(
                !(name.len() > 2
                    && name.starts_with("on")
                    && name[2..].bytes().all(|b| b.is_ascii_alphabetic())
                    && token.contains('=')),
                "inline event handler in <{tag}>"
            );
        }
    }
}

/// A full page: HTML shell, CSP header, clean markup.
#[track_caller]
pub fn assert_page(r: &Resp) {
    assert_eq!(r.status, StatusCode::OK, "body: {}", r.text());
    let ct = r.header("content-type").unwrap_or_default();
    assert!(ct.starts_with("text/html"), "content type {ct}");
    assert!(r.header("content-security-policy").is_some(), "CSP header");
    let html = r.text();
    assert!(html.starts_with("<!DOCTYPE html>"), "doctype");
    // `data-theme` follows when the user chose a theme; `lang` is the
    // request's language.
    assert!(html.contains("<html lang=\"en\"") || html.contains("<html lang=\"uk\""));
    assert_clean_html(&html);
}

/// An htmx fragment: no document shell.
#[track_caller]
pub fn assert_fragment(r: &Resp) {
    let html = r.text();
    assert!(!html.contains("<html"), "fragment has an <html> shell: {html}");
    assert!(!html.contains("<!DOCTYPE"), "fragment has a doctype");
    assert_clean_html(&html);
}
