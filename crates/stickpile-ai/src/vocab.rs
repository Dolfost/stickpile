//! Zero-shot vocabularies: keywords, emojis (with descriptions used as
//! prompts) and the fixed vibe set. Built-in defaults, replaceable by a TOML
//! file (`[ai] vocabulary_file`).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::AiError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmojiEntry {
    pub emoji: String,
    /// Prompt text, e.g. "laughing so hard I cry".
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

impl Vocabulary {
    pub fn builtin() -> Vocabulary {
        todo!("ai agent")
    }
    pub fn load(path: &Path) -> Result<Vocabulary, AiError> {
        let _ = path;
        todo!("ai agent")
    }
    /// Vibe labels whose synonyms contain `word` (after squashing repeated
    /// letters: "naahh" -> "nah"). Used by search without any model.
    pub fn vibes_for_word(&self, word: &str) -> Vec<&str> {
        let _ = word;
        todo!("ai agent")
    }
}
