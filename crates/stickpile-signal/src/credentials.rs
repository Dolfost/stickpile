//! Credentials of a registered Signal account or linked device, needed only
//! to upload packs.
//!
//! The service authenticates with HTTP Basic: user `<ACI>.<device id>` (the
//! account's ACI UUID, `.1` for the primary device) and the device's
//! password (Signal-Server `BasicAuthorizationHeader` and
//! `AccountAuthenticator.getIdentifierAndDeviceId`: the identifier is a
//! UUID, the device id follows a `.`, and defaults to the primary device).
//!
//! [`Credentials::from_file`] reads a JSON file in one of these shapes:
//!
//! 1. stickpile's own: `{"aci": "<uuid>", "device_id": 1, "password": "..."}`
//! 2. Signal Desktop's: `{"username": "<uuid>.<device id>", "password":
//!    "..."}` (the values of `uuid_id` and `password` in its storage; the
//!    signalstickers-client README shows how to read them);
//! 3. a signal-cli account file (`~/.local/share/signal-cli/data/<id>`):
//!    `aciAccountData.serviceId`, `deviceId`, `password` (storage version 8
//!    and later), or `uuid`, `deviceId`, `password` (older versions);
//!    `serviceEnvironment` must be `LIVE` and `registered` true.
//!
//! The password never appears in errors or `Debug` output.

use std::path::Path;

use serde_json::Value;

use crate::SignalError;

#[derive(Clone)]
pub struct Credentials {
    aci: String,
    device_id: u32,
    password: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("aci", &self.aci)
            .field("device_id", &self.device_id)
            .field("password", &"<redacted>")
            .finish()
    }
}

fn bad(why: &str) -> SignalError {
    SignalError::Credentials(why.to_string())
}

/// `8-4-4-4-12` hex digits.
fn valid_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts.iter().map(|p| p.len()).eq([8, 4, 4, 4, 12])
        && parts.iter().all(|p| p.bytes().all(|b| b.is_ascii_hexdigit()))
}

impl Credentials {
    pub fn new(aci: &str, device_id: u32, password: &str) -> Result<Credentials, SignalError> {
        let aci = aci.trim().to_ascii_lowercase();
        if !valid_uuid(&aci) {
            return Err(bad("the ACI must be a UUID"));
        }
        if device_id == 0 || device_id > 127 {
            return Err(bad("the device id must be between 1 and 127"));
        }
        if password.is_empty() || password.chars().any(|c| c.is_control()) {
            return Err(bad(
                "the password is empty or contains characters a password cannot have",
            ));
        }
        Ok(Credentials {
            aci,
            device_id,
            password: password.to_string(),
        })
    }

    /// Reads a credentials file (see the module docs for the formats).
    /// Errors name the file, never its content.
    pub fn from_file(path: &Path) -> Result<Credentials, SignalError> {
        let text =
            std::fs::read_to_string(path).map_err(|e| bad(&format!("cannot read {}: {}", path.display(), e.kind())))?;
        Credentials::parse(&text).map_err(|e| match e {
            SignalError::Credentials(why) => bad(&format!("{}: {why}", path.display())),
            other => other,
        })
    }

    /// Parses the content of a credentials file.
    pub fn parse(text: &str) -> Result<Credentials, SignalError> {
        // Parse into a Value first: serde's typed errors quote the offending
        // value, which could be the password.
        let v: Value = serde_json::from_str(text).map_err(|_| bad("not a JSON object"))?;
        if !v.is_object() {
            return Err(bad("not a JSON object"));
        }
        let password = v["password"].as_str().ok_or_else(|| bad("\"password\" is missing"))?;
        let device = |v: &Value| -> Result<u32, SignalError> {
            match v {
                Value::Null => Ok(1),
                Value::Number(n) => n
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| bad("the device id must be a small number")),
                _ => Err(bad("the device id must be a number")),
            }
        };
        if let Some(aci) = v["aci"].as_str() {
            return Credentials::new(aci, device(&v["device_id"])?, password);
        }
        if let Some(username) = v["username"]
            .as_str()
            .filter(|u| valid_uuid(u.split('.').next().unwrap_or("")))
        {
            let (aci, device_id) = match username.split_once('.') {
                Some((aci, d)) => (
                    aci,
                    d.parse::<u32>().map_err(|_| bad("the device id must be a number"))?,
                ),
                None => (username, 1),
            };
            return Credentials::new(aci, device_id, password);
        }
        // signal-cli.
        let aci = v["aciAccountData"]["serviceId"]
            .as_str()
            .or_else(|| v["uuid"].as_str())
            .ok_or_else(|| bad("no ACI: expected \"aci\", \"username\" or a signal-cli account file"))?;
        if let Some(env) = v["serviceEnvironment"].as_str() {
            if env != "LIVE" {
                return Err(bad(&format!(
                    "the signal-cli account is for the {env} environment, not LIVE"
                )));
            }
        }
        if v["registered"] == Value::Bool(false) {
            return Err(bad("the signal-cli account is not registered"));
        }
        Credentials::new(aci, device(&v["deviceId"])?, password)
    }

    pub fn aci(&self) -> &str {
        &self.aci
    }

    pub fn device_id(&self) -> u32 {
        self.device_id
    }

    /// The Basic auth user name: `<aci>.<device id>`.
    pub fn username(&self) -> String {
        format!("{}.{}", self.aci, self.device_id)
    }

    pub(crate) fn password(&self) -> &str {
        &self.password
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACI: &str = "8f3a1c2e-1b2c-4d5e-8f90-123456789abc";
    const SECRET: &str = "SECRET-pass+word/123";

    #[test]
    fn formats() {
        let own = Credentials::parse(&format!(
            r#"{{"aci": "{}", "device_id": 3, "password": "{SECRET}"}}"#,
            ACI.to_uppercase()
        ))
        .unwrap();
        assert_eq!((own.aci(), own.device_id()), (ACI, 3));
        assert_eq!(own.username(), format!("{ACI}.3"));

        let desktop = Credentials::parse(&format!(r#"{{"username": "{ACI}.2", "password": "{SECRET}"}}"#)).unwrap();
        assert_eq!(desktop.username(), format!("{ACI}.2"));
        let primary = Credentials::parse(&format!(r#"{{"username": "{ACI}", "password": "{SECRET}"}}"#)).unwrap();
        assert_eq!(primary.username(), format!("{ACI}.1"));

        let cli = Credentials::parse(&format!(
            r#"{{"version": 11, "serviceEnvironment": "LIVE", "registered": true, "number": "+15550000000",
                "username": null, "deviceId": 2, "password": "{SECRET}",
                "aciAccountData": {{"serviceId": "{ACI}", "registrationId": 1, "identityPrivateKey": "x"}}}}"#
        ))
        .unwrap();
        assert_eq!(cli.username(), format!("{ACI}.2"));
        let legacy = Credentials::parse(&format!(
            r#"{{"username": "+15550000000", "uuid": "{ACI}", "deviceId": 1, "password": "{SECRET}", "registered": true}}"#
        ))
        .unwrap();
        assert_eq!(legacy.username(), format!("{ACI}.1"));
        assert_eq!(legacy.password(), SECRET);
    }

    #[test]
    fn errors_never_show_the_password() {
        for text in [
            "not json".to_string(),
            format!(r#"["{SECRET}"]"#),
            format!(r#"{{"aci": "{ACI}"}}"#),
            format!(r#"{{"aci": "nope", "password": "{SECRET}"}}"#),
            format!(r#"{{"aci": "{ACI}", "device_id": "{SECRET}", "password": "{SECRET}"}}"#),
            format!(r#"{{"aci": "{ACI}", "device_id": 0, "password": "{SECRET}"}}"#),
            format!(r#"{{"username": "{ACI}.x", "password": "{SECRET}"}}"#),
            format!(r#"{{"password": "{SECRET}"}}"#),
            format!(r#"{{"aci": "{ACI}", "password": "a\nb{SECRET}"}}"#),
            format!(
                r#"{{"serviceEnvironment": "STAGING", "deviceId": 1, "password": "{SECRET}", "aciAccountData": {{"serviceId": "{ACI}"}}}}"#
            ),
            format!(
                r#"{{"registered": false, "deviceId": 1, "password": "{SECRET}", "aciAccountData": {{"serviceId": "{ACI}"}}}}"#
            ),
        ] {
            let err = Credentials::parse(&text).expect_err(&text);
            let shown = format!("{err} {err:?}");
            assert!(!shown.contains("SECRET"), "{shown}");
        }
        let ok = Credentials::new(ACI, 1, SECRET).unwrap();
        assert!(!format!("{ok:?}").contains("SECRET"));
    }

    #[test]
    fn files() {
        let dir = std::env::temp_dir().join(format!("stickpile-signal-creds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("creds.json");
        std::fs::write(&path, format!(r#"{{"aci": "{ACI}", "password": "{SECRET}"}}"#)).unwrap();
        assert_eq!(Credentials::from_file(&path).unwrap().device_id(), 1);
        std::fs::write(&path, format!(r#"{{"aci": "{ACI}", "pass": "{SECRET}"}}"#)).unwrap();
        let err = Credentials::from_file(&path).unwrap_err().to_string();
        assert!(err.contains("creds.json") && !err.contains("SECRET"), "{err}");
        let err = Credentials::from_file(&dir.join("missing.json"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing.json"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
