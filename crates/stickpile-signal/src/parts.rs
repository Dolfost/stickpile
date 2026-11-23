//! Sets larger than one pack (docs/signal.md "Sets larger than one pack").
//!
//! A Signal pack holds at most [`MAX_STICKERS`] stickers. A longer list is
//! uploaded as several packs, its parts ([`split_parts`]): part 1 gets the
//! first 200 stickers, part 2 the next 200, and so on, in order. Signal
//! packs cannot change after upload, so nothing has to stay where it was
//! between publishes: every publish uploads every part as a new pack
//! ([`upload_parts`]).

use crate::pack::{prepare, upload_prepared, UploadPack, UploadedPack, MAX_STICKERS, MAX_TITLE_CHARS};
use crate::{Credentials, SignalClient, SignalError};

/// Part `number`'s title: part 1 is `title` itself, part k is
/// `<title> <k>`. Both have at most [`MAX_TITLE_CHARS`] characters: a
/// longer title is cut on characters (not bytes) before ` <k>`, so the
/// number stays. An empty title stays empty (and an upload refuses it).
pub fn part_title(title: &str, number: u32) -> String {
    let name = title.trim();
    if number <= 1 {
        return name
            .chars()
            .take(MAX_TITLE_CHARS)
            .collect::<String>()
            .trim_end()
            .to_string();
    }
    let tag = format!(" {number}");
    let cut: String = name.chars().take(MAX_TITLE_CHARS - tag.len()).collect();
    let cut = cut.trim_end();
    if cut.is_empty() {
        return String::new();
    }
    format!("{cut}{tag}")
}

/// The packs `pack` is uploaded as: at most [`MAX_STICKERS`] stickers
/// each, in order, numbered from 1 (part 1 first). Every part has the
/// pack's author and cover (a cover that is not one of a part's stickers
/// is uploaded with that part as a file of its own, as for one pack; no
/// cover: each part's first sticker) and the title [`part_title`] gives
/// it. A pack of at most [`MAX_STICKERS`] stickers is one part, as it is.
pub fn split_parts(pack: UploadPack) -> Vec<UploadPack> {
    if pack.stickers.len() <= MAX_STICKERS {
        return vec![pack];
    }
    let UploadPack {
        title,
        author,
        stickers,
        cover,
    } = pack;
    let mut parts = Vec::with_capacity(stickers.len().div_ceil(MAX_STICKERS));
    let mut rest = stickers.into_iter().peekable();
    while rest.peek().is_some() {
        let number = parts.len() as u32 + 1;
        parts.push(UploadPack {
            title: part_title(&title, number),
            author: author.clone(),
            stickers: rest.by_ref().take(MAX_STICKERS).collect(),
            cover: cover.clone(),
        });
    }
    parts
}

/// One part [`upload_parts`] uploaded.
#[derive(Debug, Clone)]
pub struct UploadedPart {
    /// 1 for the first part.
    pub number: u32,
    /// The title in the pack's manifest.
    pub title: String,
    pub pack: UploadedPack,
}

/// What [`upload_parts`] did.
#[derive(Debug)]
pub struct PartsUpload {
    /// The parts uploaded, by number from 1.
    pub uploaded: Vec<UploadedPart>,
    /// The part that failed (its number) and why. The parts before it are
    /// uploaded (Signal keeps them; stickpile cannot delete a pack), the
    /// parts after it were not tried.
    pub failed: Option<(u32, SignalError)>,
}

/// Uploads `parts` ([`split_parts`]) as new packs, one after the other
/// ([`crate::upload_pack`] each), and stops at the first failure.
///
/// Every part is checked first: when one is invalid nothing is sent and
/// the error names the part (when there are several). Each part needs an
/// upload form of its own, so a publish of k parts counts k times against
/// the account's rate limit.
pub async fn upload_parts(
    client: &SignalClient,
    credentials: &Credentials,
    parts: &[UploadPack],
) -> Result<PartsUpload, SignalError> {
    let several = parts.len() > 1;
    let mut prepared = Vec::with_capacity(parts.len());
    for (i, part) in parts.iter().enumerate() {
        match prepare(part) {
            Ok(p) => prepared.push(p),
            Err(SignalError::Invalid(why)) if several => {
                return Err(SignalError::Invalid(format!(
                    "part {} of {}: {why}",
                    i + 1,
                    parts.len()
                )))
            }
            Err(e) => return Err(e),
        }
    }
    let mut out = PartsUpload {
        uploaded: Vec::with_capacity(parts.len()),
        failed: None,
    };
    for (i, p) in prepared.iter().enumerate() {
        let number = i as u32 + 1;
        match upload_prepared(client, credentials, p).await {
            Ok(pack) => out.uploaded.push(UploadedPart {
                number,
                title: p.manifest.title.clone().unwrap_or_default(),
                pack,
            }),
            Err(e) => {
                out.failed = Some((number, e));
                break;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::tests::png;
    use crate::UploadSticker;

    fn pack(n: usize) -> UploadPack {
        UploadPack {
            title: "Cats".into(),
            author: "me".into(),
            stickers: (0..n)
                .map(|i| {
                    let mut bytes = png(512, 512, None);
                    bytes.extend((i as u32).to_be_bytes());
                    UploadSticker {
                        bytes,
                        emoji: "😼".into(),
                    }
                })
                .collect(),
            cover: None,
        }
    }

    #[test]
    fn titles_are_numbered_and_keep_the_number_when_cut() {
        assert_eq!(part_title("  Cats ", 1), "Cats");
        assert_eq!(part_title("Cats", 2), "Cats 2");
        assert_eq!(part_title("Cats", 13), "Cats 13");
        let long = "Кіт".repeat(100);
        let one = part_title(&long, 1);
        assert_eq!(one.chars().count(), MAX_TITLE_CHARS);
        let two = part_title(&long, 2);
        assert_eq!(two.chars().count(), MAX_TITLE_CHARS);
        assert!(two.ends_with(" 2"), "{two}");
        assert!(part_title(&format!("{} x", "a".repeat(125)), 10).ends_with("a 10"));
        assert_eq!(part_title("  ", 3), "");
    }

    #[test]
    fn parts_hold_at_most_200_in_order() {
        let counts = |n: usize| -> Vec<usize> { split_parts(pack(n)).iter().map(|p| p.stickers.len()).collect() };
        assert_eq!(counts(0), [0]);
        assert_eq!(counts(1), [1]);
        assert_eq!(counts(200), [200]);
        assert_eq!(counts(201), [200, 1]);
        assert_eq!(counts(450), [200, 200, 50]);
        assert_eq!(counts(600), [200, 200, 200]);

        let whole = pack(450);
        let parts = split_parts(UploadPack {
            cover: Some(b"cover".to_vec()),
            ..whole.clone()
        });
        let titles: Vec<&str> = parts.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, ["Cats", "Cats 2", "Cats 3"]);
        assert!(parts.iter().all(|p| p.author == "me"));
        assert!(parts.iter().all(|p| p.cover.as_deref() == Some(&b"cover"[..])));
        let again: Vec<&UploadSticker> = parts.iter().flat_map(|p| &p.stickers).collect();
        assert_eq!(again.len(), 450);
        for (a, b) in again.iter().zip(&whole.stickers) {
            assert_eq!(a.bytes, b.bytes, "the set's order, every sticker once");
        }
        // One part is the pack as it was.
        let one = split_parts(UploadPack {
            title: "  Cats ".into(),
            ..pack(3)
        });
        assert_eq!(one[0].title, "  Cats ");
    }
}
