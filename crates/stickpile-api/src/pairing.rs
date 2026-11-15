//! Pairing codes and pairing links: a signed-in user creates a short-lived
//! one-time code ([`crate::routes::PAIRINGS`]) and a new device exchanges it
//! for an API token without a password ([`crate::routes::AUTH_PAIR`]). See
//! docs/api.md and docs/security.md.
//!
//! A code is [`CODE_LEN`] characters of Crockford's base32 alphabet
//! ([`ALPHABET`]: digits and upper-case letters without I, L, O and U), so it
//! carries [`CODE_BITS`] random bits. It is shown in groups of [`GROUP`]
//! (`7KQ2-M9XA-4FTR`) and read back forgivingly by [`normalize`]: any case,
//! spaces and dashes anywhere, `O` for `0` and `I` or `L` for `1`.
//!
//! The QR code shows a pairing link, [`link`]:
//! `stickpile://pair?url=<server URL>&code=<code>`, read by [`parse_link`].
//! The server URL in it is checked and normalised by [`server_url`], so that
//! the URL a client shows is the one it contacts.

/// Crockford's base32 alphabet: no I, L, O or U, which are easily confused
/// with 1, 1, 0 and V.
pub const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Characters in a code (without separators).
pub const CODE_LEN: usize = 12;
/// Random bits in a code: five per character.
pub const CODE_BITS: u32 = 5 * CODE_LEN as u32;
// docs/security.md#pairing argues from at least 40 bits; the codes come from
// one random u64.
const _: () = assert!(CODE_BITS >= 40 && CODE_BITS <= 64);
/// Characters per group in the display form.
pub const GROUP: usize = 4;
/// Separator between groups in the display form.
pub const SEPARATOR: char = '-';
/// Scheme of a pairing link.
pub const LINK_SCHEME: &str = "stickpile";
/// Host part of a pairing link (`stickpile://pair?...`).
pub const LINK_HOST: &str = "pair";

/// The code made of the low [`CODE_BITS`] bits of `bits`, in canonical form
/// (upper case, no separators). The server feeds it random bits.
pub fn code_from_bits(bits: u64) -> String {
    (0..CODE_LEN)
        .rev()
        .map(|i| ALPHABET[((bits >> (5 * i)) & 31) as usize] as char)
        .collect()
}

/// The canonical form of a code typed or scanned by a person, or `None` if it
/// cannot be one. Case does not matter; whitespace and dashes (also the
/// typographic ones a phone keyboard may put in) are ignored; `O` reads as
/// `0`, `I` and `L` as `1`.
pub fn normalize(input: &str) -> Option<String> {
    let mut out = String::with_capacity(CODE_LEN);
    for c in input.chars() {
        if c.is_whitespace() || is_dash(c) {
            continue;
        }
        let c = match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        if !c.is_ascii() || !ALPHABET.contains(&(c as u8)) || out.len() == CODE_LEN {
            return None;
        }
        out.push(c);
    }
    (out.len() == CODE_LEN).then_some(out)
}

fn is_dash(c: char) -> bool {
    // hyphen-minus, the Unicode hyphens and dashes, the minus sign
    matches!(c, '-' | '\u{2010}'..='\u{2015}' | '\u{2212}')
}

/// The display form of a code: canonical, in groups of [`GROUP`] joined by
/// [`SEPARATOR`] (`7KQ2-M9XA-4FTR`). Input that is not a valid code is
/// returned unchanged.
pub fn display(code: &str) -> String {
    let Some(canonical) = normalize(code) else {
        return code.to_string();
    };
    let chars: Vec<char> = canonical.chars().collect();
    chars
        .chunks(GROUP)
        .map(|g| g.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(&SEPARATOR.to_string())
}

/// The pairing link a QR code shows: `stickpile://pair?url=<server>&code=<code>`,
/// both values percent-encoded. A trailing `/` of the server URL is dropped.
pub fn link(server_url: &str, code: &str) -> String {
    format!(
        "{LINK_SCHEME}://{LINK_HOST}?url={}&code={}",
        percent_encode(server_url.trim_end_matches('/')),
        percent_encode(code)
    )
}

/// What a pairing link says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingLink {
    /// The server's URL, normalised by [`server_url`].
    pub server_url: String,
    /// The code in canonical form.
    pub code: String,
}

/// Why [`server_url`] refuses a server URL.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServerUrlError {
    #[error("not a URL ({0})")]
    Parse(String),
    #[error("not an http or https URL")]
    Scheme,
    #[error("no host")]
    NoHost,
    #[error("a user name or password")]
    Userinfo,
    #[error("a query or fragment")]
    QueryOrFragment,
}

/// The normalised form of a server URL typed or read from a pairing link:
/// `scheme://host[:port][/path]`, as the `url` crate parses and serialises
/// it, without a trailing `/`. The scheme and host are lower case, an
/// internationalised host is in its ASCII (`xn--`) form, and a default port
/// (80 for http, 443 for https) is dropped, so the result names the host
/// that is contacted. Refused: anything but `http` and `https`, no host, a
/// user name or password (`https://my.server@evil.example` goes to
/// `evil.example`), a query or a fragment. Surrounding spaces are ignored.
pub fn server_url(input: &str) -> Result<String, ServerUrlError> {
    let parsed = url::Url::parse(input.trim()).map_err(|e| ServerUrlError::Parse(e.to_string()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ServerUrlError::Scheme);
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(ServerUrlError::NoHost);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ServerUrlError::Userinfo);
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(ServerUrlError::QueryOrFragment);
    }
    // Without a user, a query and a fragment the serialisation is
    // `scheme://host[:port]/path`.
    Ok(parsed.as_str().trim_end_matches('/').to_string())
}

/// Read a pairing link made by [`link`]. `None` if it is not one: another
/// scheme or host, a server URL that [`server_url`] refuses, a missing or
/// invalid code. The server URL comes back normalised by [`server_url`].
/// Unknown parameters are ignored.
pub fn parse_link(text: &str) -> Option<PairingLink> {
    let text = text.trim();
    let (scheme, rest) = text.split_once("://")?;
    if !scheme.eq_ignore_ascii_case(LINK_SCHEME) {
        return None;
    }
    let (host, query) = rest.split_once('?')?;
    if !host.trim_end_matches('/').eq_ignore_ascii_case(LINK_HOST) {
        return None;
    }
    let query = query.split('#').next().unwrap_or_default();
    let (mut url, mut code) = (None, None);
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k {
            "url" => url = Some(percent_decode(v)?),
            "code" => code = Some(percent_decode(v)?),
            _ => {}
        }
    }
    Some(PairingLink {
        server_url: server_url(&url?).ok()?,
        code: normalize(&code?)?,
    })
}

/// Percent-encode everything but the unreserved characters of RFC 3986.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Decode `%XX` escapes and `+` (a space in query strings). `None` for a
/// broken escape or a result that is not UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = s.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alphabet_is_crockford() {
        assert_eq!(ALPHABET.len(), 32);
        let mut sorted = ALPHABET.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 32, "no duplicates");
        for c in [b'I', b'L', b'O', b'U'] {
            assert!(!ALPHABET.contains(&c), "{}", c as char);
        }
        assert!(ALPHABET.iter().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()));
    }

    #[test]
    fn codes_carry_sixty_bits() {
        assert_eq!(CODE_LEN, 12);
        assert_eq!(CODE_BITS, 60);
        assert_eq!(code_from_bits(0), "000000000000");
        assert_eq!(code_from_bits((1 << 60) - 1), "ZZZZZZZZZZZZ");
        // Five bits per character, most significant first.
        assert_eq!(code_from_bits(1), "000000000001");
        assert_eq!(code_from_bits(31 << 55), "Z00000000000");
        // Bits above the 60th are ignored.
        assert_eq!(code_from_bits(u64::MAX), "ZZZZZZZZZZZZ");
        // Every value maps to a different code: the encoding is a bijection
        // on 60 bits (spot check over a spread of values).
        let mut seen = std::collections::HashSet::new();
        for i in 0..5000u64 {
            let bits = i.wrapping_mul(0x9E37_79B9_7F4A_7C15) & ((1 << 60) - 1);
            let code = code_from_bits(bits);
            assert_eq!(code.len(), CODE_LEN);
            assert!(code.bytes().all(|b| ALPHABET.contains(&b)), "{code}");
            assert!(seen.insert(code));
        }
    }

    #[test]
    fn display_groups_by_four() {
        assert_eq!(display("7KQ2M9XA4FTR"), "7KQ2-M9XA-4FTR");
        assert_eq!(display("7kq2 m9xa 4ftr"), "7KQ2-M9XA-4FTR");
        assert_eq!(display("not a code"), "not a code");
        assert_eq!(normalize(&display("7KQ2M9XA4FTR")).as_deref(), Some("7KQ2M9XA4FTR"));
    }

    #[test]
    fn normalize_is_forgiving() {
        let want = Some("7KQ2M9XA4FTR".to_string());
        for input in [
            "7KQ2M9XA4FTR",
            "7KQ2-M9XA-4FTR",
            "7kq2-m9xa-4ftr",
            " 7KQ2 M9XA 4FTR ",
            "7KQ2\tM9XA\n4FTR",
            "7-K-Q-2-M-9-X-A-4-F-T-R",
            "7KQ2\u{2013}M9XA\u{2014}4FTR",
            "7KQ2\u{2212}M9XA\u{2010}4FTR",
            "7KQ2--M9XA  4FTR",
        ] {
            assert_eq!(normalize(input), want, "{input:?}");
        }
        // O reads as 0, I and L as 1, in either case.
        assert_eq!(normalize("OoOo-IiLl-0000").as_deref(), Some("000011110000"));
        assert_eq!(normalize("A0B1-C1D1-E0F0"), normalize("AOB1-CID1-EOFO"));
        assert_eq!(normalize("A0B1-C1D1-E0F0"), normalize("aob1-cld1-eofo"));
    }

    #[test]
    fn normalize_rejects_what_cannot_be_a_code() {
        for input in [
            "",
            "   ",
            "7KQ2M9XA4FT",    // too short
            "7KQ2M9XA4FTRR",  // too long
            "7KQ2-M9XA-4FTU", // U is not in the alphabet
            "7KQ2_M9XA_4FTR", // underscores are not separators
            "7KQ2.M9XA.4FTR",
            "7KQ2M9XA4FT\u{00C9}",
            "7KQ2M9XA4FT\u{0405}", // Cyrillic DZE looks like S
        ] {
            assert_eq!(normalize(input), None, "{input:?}");
        }
    }

    #[test]
    fn links_round_trip() {
        let l = link("https://stickers.example.ts.net/", "7KQ2-M9XA-4FTR");
        assert_eq!(
            l,
            "stickpile://pair?url=https%3A%2F%2Fstickers.example.ts.net&code=7KQ2-M9XA-4FTR"
        );
        let parsed = parse_link(&l).unwrap();
        assert_eq!(parsed.server_url, "https://stickers.example.ts.net");
        assert_eq!(parsed.code, "7KQ2M9XA4FTR");

        // A port and a path prefix survive, the path as the URL is sent.
        let parsed = parse_link(&link("http://192.0.2.7:8080/stick pile", "7kq2m9xa4ftr")).unwrap();
        assert_eq!(parsed.server_url, "http://192.0.2.7:8080/stick%20pile");
        assert_eq!(parsed.code, "7KQ2M9XA4FTR");
        // A server URL with a query is no server URL.
        assert_eq!(parse_link(&link("http://192.0.2.7:8080/s?x=1", "7kq2m9xa4ftr")), None);
    }

    #[test]
    fn server_urls_are_normalised() {
        for (input, want) in [
            ("https://s.example", "https://s.example"),
            (" https://s.example/ ", "https://s.example"),
            ("https://s.example//", "https://s.example"),
            // Scheme and host in lower case.
            ("HTTPS://Stickers.EXAMPLE.org", "https://stickers.example.org"),
            // Default ports dropped, others kept.
            ("https://s.example:443/", "https://s.example"),
            ("http://s.example:80", "http://s.example"),
            ("http://s.example:443", "http://s.example:443"),
            ("http://10.0.0.2:8080/", "http://10.0.0.2:8080"),
            ("http://[::1]:8080", "http://[::1]:8080"),
            // A path prefix kept, without the trailing `/`, escaped as sent.
            ("https://example.org/stickers/", "https://example.org/stickers"),
            ("https://example.org/a/b", "https://example.org/a/b"),
            ("http://h/stick pile", "http://h/stick%20pile"),
            // An internationalised host in the form that is looked up.
            ("https://b\u{fc}cher.example", "https://xn--bcher-kva.example"),
            (
                "https://\u{43f}\u{440}\u{438}\u{43c}\u{435}\u{440}.\u{443}\u{43a}\u{440}",
                "https://xn--e1afmkfd.xn--j1amh",
            ),
        ] {
            assert_eq!(server_url(input).as_deref(), Ok(want), "{input:?}");
        }
    }

    #[test]
    fn server_urls_refuse_what_would_mislead() {
        use ServerUrlError::*;
        for (input, why) in [
            ("https://my.server@evil.example", Userinfo),
            ("https://my.server:x@evil.example", Userinfo),
            ("https://:pw@s.example", Userinfo),
            ("https://s.example?a=1", QueryOrFragment),
            ("https://s.example/?", QueryOrFragment),
            ("https://s.example#top", QueryOrFragment),
            ("ftp://s.example", Scheme),
            ("javascript:alert(1)", Scheme),
            ("file:///etc/passwd", Scheme),
        ] {
            assert_eq!(server_url(input), Err(why), "{input:?}");
        }
        for input in ["", "s.example", "https://", "http://:8080"] {
            assert!(matches!(server_url(input), Err(Parse(_))), "{input:?}");
        }
    }

    #[test]
    fn links_carry_the_normalised_url_and_refuse_userinfo() {
        // A link whose URL hides another host behind a user name is refused,
        // not shown as `https://my.server@evil.example`.
        assert_eq!(
            parse_link("stickpile://pair?url=https%3A%2F%2Fmy.server%40evil.example&code=7KQ2M9XA4FTR"),
            None
        );
        let parsed =
            parse_link("stickpile://pair?url=HTTPS%3A%2F%2Fb%C3%BCcher.Example%3A443%2Fs%2F&code=7KQ2M9XA4FTR")
                .unwrap();
        assert_eq!(parsed.server_url, "https://xn--bcher-kva.example/s");
    }

    #[test]
    fn parse_link_is_lenient_about_form_and_strict_about_content() {
        let ok = parse_link("STICKPILE://Pair/?code=7kq2+m9xa+4ftr&url=http%3a%2f%2fhost%3a8080%2f&v=2#x").unwrap();
        assert_eq!(ok.server_url, "http://host:8080");
        assert_eq!(ok.code, "7KQ2M9XA4FTR");
        for bad in [
            "",
            "7KQ2-M9XA-4FTR",
            "https://pair?url=http%3A%2F%2Fh&code=7KQ2M9XA4FTR",
            "stickpile://login?url=http%3A%2F%2Fh&code=7KQ2M9XA4FTR",
            "stickpile://pair?url=http%3A%2F%2Fh",
            "stickpile://pair?code=7KQ2M9XA4FTR",
            "stickpile://pair?url=ftp%3A%2F%2Fh&code=7KQ2M9XA4FTR",
            "stickpile://pair?url=javascript%3Aalert(1)&code=7KQ2M9XA4FTR",
            "stickpile://pair?url=http%3A%2F%2F&code=7KQ2M9XA4FTR",
            "stickpile://pair?url=http%3A%2F%2Fh&code=7KQ2M9XA4FTU",
            "stickpile://pair?url=http%3A%2F%2Fh&code=7KQ2M9XA4FTR%",
            "stickpile://pair?url=http%3A%2F%2Fh%FF&code=7KQ2M9XA4FTR",
        ] {
            assert_eq!(parse_link(bad), None, "{bad:?}");
        }
    }
}
