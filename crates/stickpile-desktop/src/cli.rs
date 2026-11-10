//! Command line and what each invocation means (display-free).
//!
//! The application is unique on D-Bus. The first invocation becomes the
//! resident primary instance; every later one forwards its arguments to it
//! and exits. So `stickpile-desktop` bound to a key toggles the popup.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(
    name = "stickpile-desktop",
    version,
    about = "stickpile quick-search popup (resident; re-run to toggle)"
)]
pub struct Cli {
    /// Start resident without showing the popup (systemd unit, exec-once).
    #[arg(long)]
    pub hidden: bool,
    /// Client config file [default: $XDG_CONFIG_HOME/stickpile/config.toml].
    #[arg(long, env = "STICKPILE_CONFIG")]
    pub config: Option<PathBuf>,
    /// Serve the files of this directory instead of a server (UI and clipboard testing).
    #[arg(long, value_name = "DIR")]
    pub demo: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum Cmd {
    /// Show the popup, or hide it if it is focused (default).
    Toggle,
    /// Show the popup.
    Show,
    /// Hide the popup.
    Hide,
    /// Stop the resident instance (the clipboard content it owns goes away on Wayland).
    Quit,
}

/// What the primary instance does with one invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// First invocation: become resident, and maybe show the popup.
    Start {
        show: bool,
    },
    Toggle,
    Show,
    Hide,
    Quit,
    /// `--hidden` while already running.
    Nothing,
}

/// `first`: this invocation started the primary instance.
/// `start_hidden`: `ui.start_hidden` from the config.
pub fn action(cli: &Cli, first: bool, start_hidden: bool) -> Action {
    match (cli.command, first) {
        (Some(Cmd::Quit), _) => Action::Quit,
        (_, true) if cli.hidden => Action::Start { show: false },
        (Some(Cmd::Hide), true) => Action::Start { show: false },
        (Some(Cmd::Show | Cmd::Toggle), true) => Action::Start { show: true },
        (None, true) => Action::Start { show: !start_hidden },
        (None, false) if cli.hidden => Action::Nothing,
        (Some(Cmd::Hide), false) => Action::Hide,
        (Some(Cmd::Show), false) => Action::Show,
        (Some(Cmd::Toggle) | None, false) => Action::Toggle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("stickpile-desktop").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn parses_subcommands_and_flags() {
        assert_eq!(parse(&[]).command, None);
        assert_eq!(parse(&["toggle"]).command, Some(Cmd::Toggle));
        assert!(parse(&["--hidden"]).hidden);
        assert_eq!(
            parse(&["--config", "/x.toml", "quit"]).config,
            Some(PathBuf::from("/x.toml"))
        );
        assert_eq!(parse(&["--demo", "/m"]).demo, Some(PathBuf::from("/m")));
        assert!(Cli::try_parse_from(["stickpile-desktop", "bogus"]).is_err());
    }

    #[test]
    fn first_invocation() {
        assert_eq!(action(&parse(&[]), true, true), Action::Start { show: false });
        assert_eq!(action(&parse(&[]), true, false), Action::Start { show: true });
        assert_eq!(
            action(&parse(&["--hidden"]), true, false),
            Action::Start { show: false }
        );
        assert_eq!(action(&parse(&["toggle"]), true, true), Action::Start { show: true });
        assert_eq!(action(&parse(&["show"]), true, true), Action::Start { show: true });
        assert_eq!(action(&parse(&["hide"]), true, false), Action::Start { show: false });
        assert_eq!(action(&parse(&["quit"]), true, false), Action::Quit);
    }

    #[test]
    fn later_invocations() {
        assert_eq!(action(&parse(&[]), false, true), Action::Toggle);
        assert_eq!(action(&parse(&["toggle"]), false, true), Action::Toggle);
        assert_eq!(action(&parse(&["show"]), false, true), Action::Show);
        assert_eq!(action(&parse(&["hide"]), false, true), Action::Hide);
        assert_eq!(action(&parse(&["quit"]), false, true), Action::Quit);
        assert_eq!(action(&parse(&["--hidden"]), false, true), Action::Nothing);
        assert_eq!(action(&parse(&["--hidden", "show"]), false, true), Action::Show);
    }
}
