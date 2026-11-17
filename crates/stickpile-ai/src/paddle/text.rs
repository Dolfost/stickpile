//! Repair of Latin/Cyrillic look-alike letters in recognised lines.
//!
//! The Cyrillic recogniser knows both alphabets, and for the letters that
//! look the same in both (`T`/`Т`, `O`/`О`, `K`/`К`, `A`/`А`, ...) it picks
//! either one, so a line can come out as `GLORY ТО UКRАINE` with Cyrillic
//! `Т`, `О`, `К`, `А`. Search would then miss the words. [`fix_lookalikes`]
//! puts every look-alike letter of a word into the script of the word's other
//! letters, else into the script of the line's; when both are undecided (the
//! word and the line hold only look-alikes), into the script most of the
//! word's look-alikes already have. A word whose script stays undecided is
//! left as it is.
//!
//! This is specific to how this reader errs, so it runs inside
//! [`super::PaddleOcr`], not in the reader-independent
//! [`crate::ocr::clean_lines`].

/// Look-alikes, pairwise: `LATIN[i]` looks like `CYRILLIC[i]`.
const LATIN: [char; 24] = [
    'A', 'B', 'C', 'E', 'H', 'I', 'K', 'M', 'O', 'P', 'T', 'X', 'Y', 'a', 'c', 'e', 'i', 'o', 'p', 'x', 'y', 'Ï', 'ï',
    'ë',
];
const CYRILLIC: [char; 24] = [
    'А', 'В', 'С', 'Е', 'Н', 'І', 'К', 'М', 'О', 'Р', 'Т', 'Х', 'У', 'а', 'с', 'е', 'і', 'о', 'р', 'х', 'у', 'Ї', 'ї',
    'ё',
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Script {
    Latin,
    Cyrillic,
}

fn lookalike(c: char) -> Option<(Script, usize)> {
    LATIN
        .iter()
        .position(|&l| l == c)
        .map(|i| (Script::Latin, i))
        .or_else(|| CYRILLIC.iter().position(|&l| l == c).map(|i| (Script::Cyrillic, i)))
}

/// The script of the letters that only exist in one alphabet, by majority.
fn script_of(text: &str) -> Option<Script> {
    let (mut lat, mut cyr) = (0usize, 0usize);
    for c in text.chars().filter(|&c| lookalike(c).is_none()) {
        if c.is_alphabetic() && (c as u32) < 0x250 {
            lat += 1;
        } else if ('\u{400}'..='\u{4FF}').contains(&c) {
            cyr += 1;
        }
    }
    match lat.cmp(&cyr) {
        std::cmp::Ordering::Greater => Some(Script::Latin),
        std::cmp::Ordering::Less => Some(Script::Cyrillic),
        std::cmp::Ordering::Equal => None,
    }
}

/// The script most of the word's look-alikes are written in.
fn lookalike_majority(word: &str) -> Option<Script> {
    let (mut lat, mut cyr) = (0usize, 0usize);
    for (s, _) in word.chars().filter_map(lookalike) {
        match s {
            Script::Latin => lat += 1,
            Script::Cyrillic => cyr += 1,
        }
    }
    match lat.cmp(&cyr) {
        std::cmp::Ordering::Greater => Some(Script::Latin),
        std::cmp::Ordering::Less => Some(Script::Cyrillic),
        std::cmp::Ordering::Equal => None,
    }
}

fn convert(word: &str, to: Script) -> String {
    word.chars()
        .map(|c| match lookalike(c) {
            Some((_, i)) => match to {
                Script::Latin => LATIN[i],
                Script::Cyrillic => CYRILLIC[i],
            },
            None => c,
        })
        .collect()
}

/// Repairs look-alike letters word by word (see the module docs). Words are
/// separated by whitespace, which is kept as it is.
pub fn fix_lookalikes(line: &str) -> String {
    let line_script = script_of(line);
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while !rest.is_empty() {
        let ws = rest.find(|c: char| !c.is_whitespace()).unwrap_or(rest.len());
        out.push_str(&rest[..ws]);
        rest = &rest[ws..];
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let word = &rest[..end];
        rest = &rest[end..];
        match script_of(word).or(line_script).or_else(|| lookalike_majority(word)) {
            Some(s) => out.push_str(&convert(word, s)),
            None => out.push_str(word),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_pair_up() {
        for (l, c) in LATIN.iter().zip(CYRILLIC) {
            assert!((*l as u32) < 0x250, "{l}");
            assert!(('\u{400}'..='\u{4FF}').contains(&c), "{c}");
        }
    }

    #[test]
    fn words_follow_their_own_letters_then_the_line() {
        // Cyrillic Т, О, К, А in an English line.
        assert_eq!(fix_lookalikes("GLORY ТО UКRАINE"), "GLORY TO UKRAINE");
        // Latin I in a Ukrainian word; Latin o, c in a Ukrainian line.
        assert_eq!(fix_lookalikes("КIТ сидить"), "КІТ сидить");
        // A word of look-alikes only (Latin P, A, H, K) in a Ukrainian line.
        assert_eq!(fix_lookalikes("ДОБРОГО PAHKУ"), "ДОБРОГО РАНКУ");
    }

    #[test]
    fn undecided_words_take_their_majority() {
        // Only look-alikes in the word and the line: two of three are Cyrillic.
        assert_eq!(fix_lookalikes("КIТ"), "КІТ");
        assert_eq!(fix_lookalikes("TОP"), "TOP");
        // A tie stays as it is.
        assert_eq!(fix_lookalikes("ОK"), "ОK");
    }

    #[test]
    fn other_text_is_untouched() {
        assert_eq!(fix_lookalikes("09:14  ok?\tДобре"), "09:14  ok?\tДобре");
        assert_eq!(fix_lookalikes("з'їжджу"), "з'їжджу");
        assert_eq!(fix_lookalikes(""), "");
        assert_eq!(fix_lookalikes("  "), "  ");
        // Mixed-script words with a clear majority keep the majority script.
        assert_eq!(fix_lookalikes("MIЙ"), "МІЙ");
        assert_eq!(fix_lookalikes("Сat"), "Cat");
    }
}
