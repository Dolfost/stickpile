//! The resident `adw::Application`.
//!
//! Unique on D-Bus (`io.github.dolfost.Stickpile`) with
//! `HANDLES_COMMAND_LINE | SEND_ENVIRONMENT`: the first invocation becomes the
//! primary instance and stays alive (it owns the clipboard content on
//! Wayland); later invocations forward their arguments and environment to it
//! and exit. The forwarded environment is used for focus detection, so a
//! long-running instance follows the current Hyprland session.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use clap::Parser;
use gtk::{gio, glib};
use stickpile_core::ClientConfig;
use stickpile_quick::focus::FocusEnv;

use crate::cli::{self, Action, Cli};
use crate::glue::APP_ID;
use crate::ui::shell::{Shell, StartOptions};

pub fn run() -> glib::ExitCode {
    // Parse locally first so `--help`, `--version` and usage errors are
    // printed by the invoking process.
    let _ = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .init();

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE | gio::ApplicationFlags::SEND_ENVIRONMENT)
        .build();

    let shell: Rc<RefCell<Option<Rc<Shell>>>> = Rc::default();
    let hold: Rc<RefCell<Option<gio::ApplicationHoldGuard>>> = Rc::default();
    app.connect_command_line(move |app, cmdline| {
        let cli = match Cli::try_parse_from(cmdline.arguments()) {
            Ok(c) => c,
            Err(e) => {
                // The invoking process already validated its arguments; this
                // only happens with mismatched binaries.
                tracing::warn!("ignoring forwarded command line: {e}");
                return glib::ExitCode::FAILURE;
            }
        };
        let first = shell.borrow().is_none();
        if first {
            if cli.command == Some(cli::Cmd::Quit) {
                return glib::ExitCode::SUCCESS;
            }
            let config_path = cli.config.clone().unwrap_or_else(ClientConfig::default_path);
            let s = Shell::start(
                app,
                StartOptions {
                    config_path,
                    demo: cli.demo.clone(),
                },
            );
            *shell.borrow_mut() = Some(s);
            *hold.borrow_mut() = Some(app.hold());
        }
        let s = shell.borrow().clone().expect("started above");
        let env = FocusEnv::from_vars(|k| cmdline.getenv(k).map(|v| v.to_string()));
        match cli::action(&cli, first, s.start_hidden()) {
            Action::Start { show: true } | Action::Show => s.open(&env),
            Action::Start { show: false } | Action::Nothing => {}
            Action::Toggle => s.toggle(&env),
            Action::Hide => s.hide(),
            Action::Quit => {
                hold.borrow_mut().take();
                app.quit();
            }
        }
        glib::ExitCode::SUCCESS
    });
    app.run_with_args(&std::env::args().collect::<Vec<_>>())
}
