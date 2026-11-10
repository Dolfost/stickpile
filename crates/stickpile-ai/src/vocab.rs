//! Zero-shot vocabularies: keywords, emojis (with descriptions used as
//! prompts) and the fixed vibe set. Built-in defaults, replaceable by a TOML
//! file (`[ai] vocabulary_file`).
//!
//! The file format is the [`Vocabulary`] struct itself; the built-in list
//! (`src/vocab_default.toml`) is the reference example:
//!
//! ```toml
//! keywords = ["cat", "thumbs up"]
//!
//! [[emojis]]
//! emoji = "😂"
//! description = "face with tears of joy, laughing so hard it cries"
//!
//! [[vibes]]
//! label = "dismissive"
//! synonyms = ["nah", "nope", "ні"]
//! ```
//!
//! Every top-level key is optional when loading a file: a missing key keeps
//! the built-in list, an explicit empty list disables that kind of suggestion.

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::AiError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmojiEntry {
    pub emoji: String,
    /// Prompt text, e.g. "laughing so hard I cry".
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VibeEntry {
    /// One of the fixed labels: happy, sad, sarcastic, angry, tired, cringe,
    /// wholesome, confused, smug, scared, love, shocked, dismissive, ...
    pub label: String,
    /// Prompts and colloquial synonyms ("nah", "nope" for dismissive) — also
    /// used by search to expand queries.
    pub synonyms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vocabulary {
    pub keywords: Vec<String>,
    pub emojis: Vec<EmojiEntry>,
    pub vibes: Vec<VibeEntry>,
}

/// The fixed vibe labels of the built-in vocabulary (and of `vibe:` search).
pub const VIBE_LABELS: [&str; 16] = [
    "happy",
    "sad",
    "sarcastic",
    "angry",
    "tired",
    "cringe",
    "wholesome",
    "confused",
    "smug",
    "scared",
    "love",
    "shocked",
    "dismissive",
    "excited",
    "bored",
    "awkward",
];

const BUILTIN_TOML: &str = include_str!("vocab_default.toml");
const MAX_TERM_LEN: usize = 100;

/// File form: every section optional (missing = built-in).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VocabularyFile {
    keywords: Option<Vec<String>>,
    emojis: Option<Vec<EmojiEntry>>,
    vibes: Option<Vec<VibeEntry>>,
}

impl Vocabulary {
    pub fn builtin() -> Vocabulary {
        static BUILTIN: OnceLock<Vocabulary> = OnceLock::new();
        BUILTIN
            .get_or_init(|| {
                let f: VocabularyFile = toml::from_str(BUILTIN_TOML).expect("built-in vocabulary parses");
                let v = Vocabulary {
                    keywords: f.keywords.unwrap_or_default(),
                    emojis: f.emojis.unwrap_or_default(),
                    vibes: f.vibes.unwrap_or_default(),
                }
                .normalized();
                v.validate().expect("built-in vocabulary is valid");
                v
            })
            .clone()
    }

    pub fn load(path: &Path) -> Result<Vocabulary, AiError> {
        let text =
            std::fs::read_to_string(path).map_err(|e| AiError::Vocabulary(format!("{}: {e}", path.display())))?;
        Self::from_toml_str(&text).map_err(|e| match e {
            AiError::Vocabulary(m) => AiError::Vocabulary(format!("{}: {m}", path.display())),
            other => other,
        })
    }

    /// Parses the file format (sections missing from `text` come from [`Vocabulary::builtin`]),
    /// trims whitespace and validates.
    pub fn from_toml_str(text: &str) -> Result<Vocabulary, AiError> {
        let f: VocabularyFile = toml::from_str(text).map_err(|e| AiError::Vocabulary(e.to_string()))?;
        let mut v = match (&f.keywords, &f.emojis, &f.vibes) {
            (Some(_), Some(_), Some(_)) => Vocabulary {
                keywords: Vec::new(),
                emojis: Vec::new(),
                vibes: Vec::new(),
            },
            _ => Self::builtin(),
        };
        if let Some(k) = f.keywords {
            v.keywords = k;
        }
        if let Some(e) = f.emojis {
            v.emojis = e;
        }
        if let Some(vb) = f.vibes {
            v.vibes = vb;
        }
        let v = v.normalized();
        v.validate()?;
        Ok(v)
    }

    /// Serialises in the file format (round-trips through [`Vocabulary::from_toml_str`]).
    pub fn to_toml_string(&self) -> String {
        toml::to_string(self).expect("vocabulary serialises")
    }

    fn normalized(mut self) -> Vocabulary {
        let trim = |s: &mut String| *s = s.split_whitespace().collect::<Vec<_>>().join(" ");
        self.keywords.iter_mut().for_each(trim);
        for e in &mut self.emojis {
            e.emoji = e.emoji.trim().to_owned();
            trim(&mut e.description);
        }
        for v in &mut self.vibes {
            v.label = v.label.trim().to_lowercase();
            v.synonyms.iter_mut().for_each(trim);
        }
        self
    }

    /// Rejects empty or overlong terms, duplicates, malformed vibe labels and a
    /// single-vibe set (the vibe classifier needs at least two to choose from).
    pub fn validate(&self) -> Result<(), AiError> {
        let err = |m: String| Err(AiError::Vocabulary(m));
        let check = |kind: &str, s: &str| -> Result<(), AiError> {
            if s.is_empty() {
                return err(format!("empty {kind}"));
            }
            if s.chars().count() > MAX_TERM_LEN {
                return err(format!("{kind} longer than {MAX_TERM_LEN} characters: {s:?}"));
            }
            Ok(())
        };
        let mut seen = HashSet::new();
        for k in &self.keywords {
            check("keyword", k)?;
            if !seen.insert(k.to_lowercase()) {
                return err(format!("duplicate keyword {k:?}"));
            }
        }
        let mut seen = HashSet::new();
        for e in &self.emojis {
            check("emoji", &e.emoji)?;
            if e.emoji.is_ascii() || e.emoji.chars().count() > 16 || e.emoji.chars().any(char::is_whitespace) {
                return err(format!("emoji {:?} is not a single emoji", e.emoji));
            }
            check(&format!("description of {}", e.emoji), &e.description)?;
            if !seen.insert(e.emoji.as_str()) {
                return err(format!("duplicate emoji {}", e.emoji));
            }
        }
        let mut seen = HashSet::new();
        for v in &self.vibes {
            let ok = v.label.len() <= 32
                && v.label.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                && v.label
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
            if !ok {
                return err(format!("vibe label {:?} must match [a-z][a-z0-9_-]{{0,31}}", v.label));
            }
            if !seen.insert(v.label.as_str()) {
                return err(format!("duplicate vibe {:?}", v.label));
            }
            for s in &v.synonyms {
                check(&format!("synonym of vibe {}", v.label), s)?;
            }
        }
        if self.vibes.len() == 1 {
            return err("at least two vibes are needed (or none to disable vibes)".into());
        }
        Ok(())
    }

    /// Vibe labels whose synonyms contain `word` (after squashing repeated
    /// letters: "naahh" -> "nah"). Used by search without any model.
    ///
    /// Matching is case-insensitive and applies [`squash`] to both sides, so
    /// "NAAAH", "nooope", "hahaha" (-> "haha") and "ніііі" (-> "ні") match. The
    /// label itself counts as a synonym. Labels come in vocabulary order.
    pub fn vibes_for_word(&self, word: &str) -> Vec<&str> {
        let q = squash(word);
        if q.is_empty() {
            return Vec::new();
        }
        self.vibes
            .iter()
            .filter(|v| squash(&v.label) == q || v.synonyms.iter().any(|s| squash(s) == q))
            .map(|v| v.label.as_str())
            .collect()
    }

    /// Short stable hash of the vocabulary content; store it next to AI
    /// suggestions to know when they were computed against a different list.
    pub fn fingerprint(&self) -> String {
        let digest = Sha256::digest(self.to_toml_string().as_bytes());
        digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
    }
}

/// Normal form for lexical matching of slang: lowercase, surrounding
/// punctuation stripped (unless nothing else is left, as in "???"), runs of a
/// repeated character collapsed ("naahh" -> "nah", "ніііі" -> "ні") and
/// repeated two-character units collapsed ("hahaha" -> "ha", "lolol" -> "lol").
pub fn squash(word: &str) -> String {
    let lower = word.trim().to_lowercase();
    let core = lower.trim_matches(|c: char| !c.is_alphanumeric());
    let src = if core.is_empty() { lower.as_str() } else { core };
    let mut out: Vec<char> = Vec::with_capacity(src.len());
    for c in src.chars() {
        if out.last() == Some(&c) {
            continue;
        }
        out.push(c);
        let n = out.len();
        if n >= 4 && out[n - 4] == out[n - 2] && out[n - 3] == out[n - 1] {
            out.truncate(n - 2);
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_sizes_and_fixed_vibes() {
        let v = Vocabulary::builtin();
        assert!((180..=260).contains(&v.keywords.len()), "{} keywords", v.keywords.len());
        assert!((90..=140).contains(&v.emojis.len()), "{} emojis", v.emojis.len());
        let labels: Vec<&str> = v.vibes.iter().map(|v| v.label.as_str()).collect();
        assert_eq!(labels, VIBE_LABELS);
        for kw in [
            "facepalm",
            "thumbs up",
            "crying",
            "laughing",
            "shrug",
            "cat",
            "anime girl",
        ] {
            assert!(v.keywords.iter().any(|k| k == kw), "{kw}");
        }
        v.validate().unwrap();
    }

    #[test]
    fn squash_rules() {
        assert_eq!(squash("naahh"), "nah");
        assert_eq!(squash("NAAAAH!!!"), "nah");
        assert_eq!(squash("nooooope"), "nope");
        assert_eq!(squash("hahahaha"), "ha");
        assert_eq!(squash("haha"), "ha");
        assert_eq!(squash("lololol"), "lol");
        assert_eq!(squash("bruuuh"), "bruh");
        assert_eq!(squash("ніііі"), "ні");
        assert_eq!(squash("  Сумно "), "сумно");
        assert_eq!(squash("???"), "?");
        assert_eq!(squash(":("), ":(");
        assert_eq!(squash(""), "");
    }

    #[test]
    fn vibes_for_word_squashes_and_ignores_case() {
        let v = Vocabulary::builtin();
        assert_eq!(v.vibes_for_word("naahh"), vec!["dismissive"]);
        assert_eq!(v.vibes_for_word("NOPE"), vec!["dismissive"]);
        assert_eq!(v.vibes_for_word("ні"), vec!["dismissive"]);
        assert_eq!(v.vibes_for_word("НІІІ"), vec!["dismissive"]);
        assert_eq!(v.vibes_for_word("bruuuh"), vec!["dismissive"]);
        assert_eq!(v.vibes_for_word("сумно"), vec!["sad"]);
        assert_eq!(v.vibes_for_word("ура"), vec!["excited"]);
        assert_eq!(v.vibes_for_word("lmaooo"), vec!["happy"]);
        assert_eq!(v.vibes_for_word("hahahaha"), vec!["happy"]);
        assert_eq!(v.vibes_for_word("yaaay"), vec!["happy", "excited"]);
        assert_eq!(v.vibes_for_word("Tired"), vec!["tired"]);
        assert_eq!(v.vibes_for_word("omg"), vec!["shocked"]);
        let sigh = v.vibes_for_word("sigh");
        assert!(sigh.contains(&"sad") && sigh.contains(&"tired"), "{sigh:?}");
        assert!(v.vibes_for_word("ugh").contains(&"angry"));
        assert!(v.vibes_for_word("pizza").is_empty());
        assert!(v.vibes_for_word("").is_empty());
        assert!(v.vibes_for_word("   ").is_empty());
    }

    #[test]
    fn toml_round_trip_and_partial_files() {
        let v = Vocabulary::builtin();
        let text = v.to_toml_string();
        assert_eq!(Vocabulary::from_toml_str(&text).unwrap(), v);

        // Only keywords: emojis and vibes come from the built-in list.
        let p = Vocabulary::from_toml_str("keywords = [\"  goose \", \"cat\"]").unwrap();
        assert_eq!(p.keywords, vec!["goose", "cat"]);
        assert_eq!(p.emojis, v.emojis);
        assert_eq!(p.vibes, v.vibes);

        // Explicit empty lists disable a section.
        let e = Vocabulary::from_toml_str("keywords = []\nemojis = []\nvibes = []").unwrap();
        assert!(e.keywords.is_empty() && e.emojis.is_empty() && e.vibes.is_empty());

        let custom = r#"
            keywords = ["frog"]
            [[emojis]]
            emoji = "🐸"
            description = "frog"
            [[vibes]]
            label = "Chill"
            synonyms = ["relaxed", "vibing"]
            [[vibes]]
            label = "hype"
            synonyms = ["lets go"]
        "#;
        let c = Vocabulary::from_toml_str(custom).unwrap();
        assert_eq!(c.vibes[0].label, "chill");
        assert_eq!(c.vibes_for_word("vibiiing"), vec!["chill"]);
        assert_ne!(c.fingerprint(), v.fingerprint());
        assert_eq!(v.fingerprint(), Vocabulary::builtin().fingerprint());
        assert_eq!(v.fingerprint().len(), 16);
    }

    #[test]
    fn validation_errors() {
        let bad = [
            "keywords = [\"cat\", \"Cat\"]",
            "keywords = [\"\"]",
            "keywords = \"cat\"",
            "unknown = 1",
            "[[emojis]]\nemoji = \"😂\"\ndescription = \"\"",
            "[[emojis]]\nemoji = \"lol\"\ndescription = \"x\"",
            "[[emojis]]\nemoji = \"😂\"\ndescription = \"a\"\n[[emojis]]\nemoji = \"😂\"\ndescription = \"b\"",
            "[[emojis]]\nemoji = \"😂\"\ndescription = \"a\"\nextra = 1",
            "[[vibes]]\nlabel = \"happy\"\nsynonyms = []",
            "[[vibes]]\nlabel = \"two words\"\nsynonyms = []\n[[vibes]]\nlabel = \"b\"\nsynonyms = []",
            "[[vibes]]\nlabel = \"a\"\nsynonyms = []\n[[vibes]]\nlabel = \"a\"\nsynonyms = []",
            "[[vibes]]\nlabel = \"a\"\nsynonyms = [\" \"]\n[[vibes]]\nlabel = \"b\"\nsynonyms = []",
        ];
        for b in bad {
            assert!(
                matches!(Vocabulary::from_toml_str(b), Err(AiError::Vocabulary(_))),
                "accepted: {b}"
            );
        }
        let long = format!("keywords = [\"{}\"]", "x".repeat(MAX_TERM_LEN + 1));
        assert!(Vocabulary::from_toml_str(&long).is_err());
    }

    #[test]
    fn load_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("vocab.toml");
        std::fs::write(&p, "keywords = [\"capybara\"]\n").unwrap();
        assert_eq!(Vocabulary::load(&p).unwrap().keywords, vec!["capybara"]);
        std::fs::write(&p, "keywords = [\"a\", \"a\"]\n").unwrap();
        let e = Vocabulary::load(&p).unwrap_err().to_string();
        assert!(e.contains("vocab.toml") && e.contains("duplicate"), "{e}");
        assert!(Vocabulary::load(&dir.path().join("missing.toml")).is_err());
    }
}
