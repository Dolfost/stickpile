//! Config drift guard: fails loudly when a Nix module and the Rust config it
//! mirrors disagree.
//!
//! The real config modules are compiled in with `#[path]`, so this checks the
//! code that ships, not a copy. They must stay self-contained (std, serde,
//! toml, thiserror, directories, stickpile_api); if one grows another
//! dependency, add it to Cargo.toml here.
//!
//! ```text
//! stickpile-config-drift server|client <defaults.json> <defaults.toml>
//! stickpile-config-drift dump server|client
//! ```
//!
//! `defaults.json` is the module's evaluated `settings` with nothing set
//! (`builtins.toJSON`, nulls kept); `defaults.toml` is what the module renders
//! from it (nulls dropped). The check passes only if
//! 1. the JSON equals `serde_json::to_value(Default::default())` exactly: the
//!    same keys in both directions (None serialises as null, so optional keys
//!    count too) and the same default values, and
//! 2. the rendered TOML parses with the real loader (`deny_unknown_fields`)
//!    back to `Default::default()`.

#[allow(dead_code, unused_imports)]
#[path = "../../../crates/stickpile-server/src/config.rs"]
mod server_config;

#[allow(dead_code, unused_imports)]
#[path = "../../../crates/stickpile-core/src/config.rs"]
mod client_config;

use std::path::Path;
use std::process::ExitCode;

use serde_json::Value;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match argv.as_slice() {
        ["dump", "server"] => dump(&server_config::Config::default()),
        ["dump", "client"] => dump(&client_config::ClientConfig::default()),
        ["server", json, toml] => check(
            "services.stickpile.settings",
            "crates/stickpile-server/src/config.rs (Config)",
            &server_config::Config::default(),
            Path::new(json),
            Path::new(toml),
            |t| server_config::Config::from_toml(t).map_err(|e| e.to_string()),
        ),
        ["client", json, toml] => check(
            "programs.stickpile.settings",
            "crates/stickpile-core/src/config.rs (ClientConfig)",
            &client_config::ClientConfig::default(),
            Path::new(json),
            Path::new(toml),
            |t| client_config::ClientConfig::from_toml(t).map_err(|e| e.to_string()),
        ),
        _ => Err("usage: stickpile-config-drift server|client <defaults.json> <defaults.toml>\n       \
                  stickpile-config-drift dump server|client"
            .into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::FAILURE
        }
    }
}

fn dump<T: serde::Serialize>(value: &T) -> Result<(), String> {
    let v = serde_json::to_value(value).map_err(|e| e.to_string())?;
    println!("{}", serde_json::to_string_pretty(&v).unwrap());
    Ok(())
}

fn check<T, F>(module: &str, rust: &str, default: &T, json: &Path, toml: &Path, parse: F) -> Result<(), String>
where
    T: serde::Serialize + PartialEq + std::fmt::Debug,
    F: Fn(&str) -> Result<T, String>,
{
    let read = |p: &Path| std::fs::read_to_string(p).map_err(|e| format!("reading {}: {e}", p.display()));
    let nix: Value = serde_json::from_str(&read(json)?).map_err(|e| format!("parsing {}: {e}", json.display()))?;
    let rust_value = serde_json::to_value(default).map_err(|e| e.to_string())?;

    let mut problems = Vec::new();
    diff("", &nix, &rust_value, &mut problems);

    match parse(&read(toml)?) {
        Ok(parsed) if &parsed == default => {}
        // Already explained key by key above.
        Ok(_) if !problems.is_empty() => {}
        Ok(parsed) => problems.push(format!(
            "the module's rendered default TOML parses, but not to the Rust default:\n  parsed:  {parsed:?}\n  default: {default:?}"
        )),
        Err(e) => problems.push(format!("the Rust loader rejects the module's rendered default TOML: {e}")),
    }

    if problems.is_empty() {
        println!("{module}: in sync with {rust}");
        return Ok(());
    }
    let mut msg = format!("CONFIG DRIFT between the Nix module ({module}) and {rust}:\n");
    for p in &problems {
        msg.push_str("  - ");
        msg.push_str(p);
        msg.push('\n');
    }
    msg.push_str(
        "Every Rust config field needs a typed option with the same name and default in the module \
         (nix/modules/*.nix), and vice versa. See docs/nix.md.",
    );
    Err(msg)
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

fn diff(path: &str, nix: &Value, rust: &Value, out: &mut Vec<String>) {
    match (nix, rust) {
        (Value::Object(n), Value::Object(r)) => {
            for (k, nv) in n {
                match r.get(k) {
                    Some(rv) => diff(&join(path, k), nv, rv, out),
                    None => out.push(format!("{}: option exists in the Nix module but not in Rust", join(path, k))),
                }
            }
            for (k, rv) in r {
                if !n.contains_key(k) {
                    out.push(format!("{}: field exists in Rust (default {rv}) but has no Nix option", join(path, k)));
                }
            }
        }
        (Value::Array(n), Value::Array(r)) if n.len() == r.len() => {
            for (i, (nv, rv)) in n.iter().zip(r).enumerate() {
                diff(&format!("{path}[{i}]"), nv, rv, out);
            }
        }
        _ if nix == rust => {}
        _ => out.push(format!("{path}: default differs: Nix {nix}, Rust {rust}")),
    }
}
