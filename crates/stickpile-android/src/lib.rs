//! stickpile for Android, the Rust side: a library over `stickpile-core`
//! offered to the Kotlin app in `android/` through UniFFI. See
//! `docs/android.md` ("Bridge interface").

uniffi::setup_scaffolding!();

/// The stickpile version this library was built from (the workspace's).
#[uniffi::export]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// The client name the library reports to the server, as the other
/// clients do (`desktop/0.5.2`, `cli/0.5.2`).
#[uniffi::export]
pub fn client_name() -> String {
    format!("android/{}", version())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_is_the_workspace_version() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(client_name(), format!("android/{}", env!("CARGO_PKG_VERSION")));
        // stickpile-core is linked: its default config resolves.
        let _ = stickpile_core::ClientConfig::default();
    }
}
