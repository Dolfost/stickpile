//! The item text field: the words written in the picture (docs/api.md,
//! "Items"). One string, line breaks kept. Shared by the server (what a
//! person types), the OCR cleaning in `stickpile-ai` and the clients.

/// Longest item text, in characters, after [`clean_text`].
pub const MAX_TEXT_CHARS: usize = 2000;

/// The normal form of an item text: `\r\n` and a lone `\r` become `\n`, tabs
/// become spaces, every other control character is removed, and the
/// result is trimmed. The length is not limited here: a person's text over
/// [`MAX_TEXT_CHARS`] is refused, a machine's is cut with [`truncate_text`].
pub fn clean_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\n' => out.push('\n'),
            '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.trim().to_string()
}

/// `text` cut to at most [`MAX_TEXT_CHARS`] characters (at a character
/// boundary), trimmed again.
pub fn truncate_text(text: &str) -> String {
    match text.char_indices().nth(MAX_TEXT_CHARS) {
        Some((at, _)) => text[..at].trim_end().to_string(),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_line_breaks_tabs_and_controls() {
        assert_eq!(
            clean_text("  no\r\nthoughts\rjust\tvibes \u{7}\u{0}"),
            "no\nthoughts\njust vibes"
        );
        assert_eq!(clean_text("\n\n  \t"), "");
        assert_eq!(
            clean_text("Кіт\u{200b} 😼"),
            "Кіт\u{200b} 😼",
            "only control characters go"
        );
        assert_eq!(clean_text("a\u{85}b\u{9f}c"), "abc", "C1 controls too");
        assert_eq!(clean_text("one\n\ntwo"), "one\n\ntwo", "inner blank lines stay");
    }

    #[test]
    fn truncates_at_a_character_boundary() {
        let long: String = "ї".repeat(MAX_TEXT_CHARS + 5);
        let cut = truncate_text(&long);
        assert_eq!(cut.chars().count(), MAX_TEXT_CHARS);
        let spaced = format!("{} tail", "a".repeat(MAX_TEXT_CHARS - 1));
        assert_eq!(
            truncate_text(&spaced),
            "a".repeat(MAX_TEXT_CHARS - 1),
            "trailing space trimmed"
        );
        assert_eq!(truncate_text("short"), "short");
    }
}
