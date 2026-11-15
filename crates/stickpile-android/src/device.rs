//! The name of the API tokens this library creates, as the account's token
//! list on the server shows them: `Android (<device model>)`, the model from
//! the `ro.product.model` system property (`Android (Pixel 8)`), or
//! `Android (phone)` where there is none (on the host, in tests).

/// The longest model kept; the server takes token names of 1 to 100
/// characters.
const MODEL_MAX_CHARS: usize = 60;

/// `Android (<model>)`.
pub(crate) fn token_name() -> String {
    token_name_for(device_model().as_deref())
}

fn token_name_for(model: Option<&str>) -> String {
    let model = model
        .map(|m| m.chars().filter(|c| !c.is_control()).collect::<String>())
        .map(|m| m.trim().chars().take(MODEL_MAX_CHARS).collect::<String>())
        .filter(|m| !m.trim().is_empty());
    format!("Android ({})", model.as_deref().map_or("phone", str::trim))
}

/// `ro.product.model` (for example `Pixel 8`).
#[cfg(target_os = "android")]
fn device_model() -> Option<String> {
    let name = c"ro.product.model";
    let mut value = [0 as libc::c_char; libc::PROP_VALUE_MAX as usize];
    // SAFETY: `name` is NUL-terminated and `value` has the PROP_VALUE_MAX
    // bytes the call may write (the value and its NUL).
    let len = unsafe { libc::__system_property_get(name.as_ptr(), value.as_mut_ptr()) };
    if len <= 0 {
        return None;
    }
    // SAFETY: the call NUL-terminated `value` within its length.
    let model = unsafe { std::ffi::CStr::from_ptr(value.as_ptr()) };
    Some(model.to_string_lossy().into_owned())
}

#[cfg(not(target_os = "android"))]
fn device_model() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_names() {
        assert_eq!(token_name(), "Android (phone)", "no model off Android");
        assert_eq!(token_name_for(Some("Pixel 8")), "Android (Pixel 8)");
        assert_eq!(token_name_for(Some("  \n")), "Android (phone)");
        assert_eq!(token_name_for(None), "Android (phone)");
        assert_eq!(token_name_for(Some("a\u{0}b")), "Android (ab)");
        let long = token_name_for(Some(&"x".repeat(200)));
        assert_eq!(long.chars().count(), "Android ()".len() + MODEL_MAX_CHARS);
    }
}
