//! Phase 0 clipboard spike (throwaway). See docs/clipboard.md.
//!
//! GUI mode (default) uses GDK's clipboard and drag source, the same
//! mechanism as the desktop client. `--wl-data-control` copies through
//! wl-clipboard-rs instead.

mod fixtures;
mod gui;
mod payload;
mod record;
mod wl;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use crate::fixtures::Fixture;

#[derive(Debug, Parser)]
#[command(
    name = "clipspike",
    about = "Offer a media file on the clipboard in chosen MIME combinations and record what chat apps do with it"
)]
struct Cli {
    /// Media file to offer [default: a generated fixture].
    file: Option<PathBuf>,
    /// Start with this built-in fixture.
    #[arg(long, value_enum, conflicts_with = "file")]
    fixture: Option<Fixture>,
    /// Where fixtures are written [default: $XDG_CACHE_HOME/clipspike].
    #[arg(long, value_name = "DIR")]
    fixtures_dir: Option<PathBuf>,
    /// Markdown file the result recorder appends to.
    #[arg(long, value_name = "FILE", default_value = "clipboard-results.md")]
    results: PathBuf,
    /// Copy via wlr/ext-data-control (wl-clipboard-rs) without a window.
    #[arg(long)]
    wl_data_control: bool,
    /// MIME type to offer, in order; repeatable [default: the provisional default for the file kind].
    #[arg(long = "mime", value_name = "MIME")]
    mimes: Vec<String>,
    /// With --wl-data-control: stop owning the clipboard after SECS (persistence test).
    #[arg(long, value_name = "SECS", requires = "wl_data_control")]
    quit_after: Option<u64>,
    /// Write all fixtures to --fixtures-dir, print their paths and exit.
    #[arg(long)]
    generate_fixtures: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let dir = cli.fixtures_dir.clone().unwrap_or_else(fixtures::default_dir);
    if cli.generate_fixtures {
        let mut ok = true;
        for f in Fixture::ALL {
            match fixtures::generate(f, &dir) {
                Ok(p) => println!("{}", p.display()),
                Err(e) => {
                    eprintln!("clipspike: {}: {e:#}", f.file_name());
                    ok = false;
                }
            }
        }
        return if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE };
    }
    let file = match &cli.file {
        Some(f) => f.clone(),
        None => match fixtures::generate(cli.fixture.unwrap_or(Fixture::Png), &dir) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("clipspike: {e:#}");
                return ExitCode::FAILURE;
            }
        },
    };
    if cli.wl_data_control {
        let mimes = if cli.mimes.is_empty() {
            let kind = std::fs::read(&file)
                .map(|b| payload::sniff(&b))
                .unwrap_or(payload::Kind::Other);
            payload::provisional_default(kind, false)
                .iter()
                .map(|s| s.to_string())
                .collect()
        } else {
            cli.mimes.clone()
        };
        return match wl::copy(&file, &mimes, cli.quit_after) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("clipspike: {e:#}");
                ExitCode::FAILURE
            }
        };
    }
    let code = gui::run(gui::GuiArgs {
        file,
        fixtures_dir: dir,
        results: cli.results,
        mimes: cli.mimes,
    });
    if code == gtk::glib::ExitCode::SUCCESS {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses() {
        let c = Cli::try_parse_from(["clipspike", "--fixture", "gif"]).unwrap();
        assert_eq!(c.fixture, Some(Fixture::Gif));
        assert_eq!(c.results, PathBuf::from("clipboard-results.md"));
        let c = Cli::try_parse_from([
            "clipspike",
            "a.gif",
            "--wl-data-control",
            "--mime",
            "image/gif",
            "--mime",
            "text/uri-list",
            "--quit-after",
            "3",
        ])
        .unwrap();
        assert_eq!(c.mimes, ["image/gif", "text/uri-list"]);
        assert_eq!(c.quit_after, Some(3));
        assert!(
            Cli::try_parse_from(["clipspike", "--quit-after", "3"]).is_err(),
            "needs --wl-data-control"
        );
        assert!(Cli::try_parse_from(["clipspike", "a.png", "--fixture", "png"]).is_err());
    }
}
