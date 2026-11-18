//! A synthetic library for the speed measurements (docs/search.md,
//! "Limits and performance"): docs with names, keywords, tags, emojis, a
//! vibe, one of 300 sets of 20 owners (a third each private, unlisted and
//! public), and the words in the picture as OCR gives them: English and
//! Ukrainian captions (common words far more often than rare ones),
//! apostrophes in their three forms, uppercase lines, words OCR joined
//! (`КОЛИВ`), letters it confused, numbers and odd words.
//!
//! The same seed gives the same docs, so each crate measures the same
//! library: the search here, the suggestions in stickpile-quick, the
//! server, the desktop backend and the Android bridge.

use stickpile_api::{ItemSummary, MediaFormat, MediaKind, Visibility};

use crate::Doc;

/// Tiny deterministic PRNG (xorshift64*).
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    pub fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len())]
    }
    /// In 0..1.
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }
    /// An index into a list of `n`, the first ones far more often (about
    /// log-uniform, like the words of a language).
    pub fn common(&mut self, n: usize) -> usize {
        (((n as f64).powf(self.unit()) - 1.0) as usize).min(n - 1)
    }
}

/// English caption words, the most used first.
pub const ENGLISH: &[&str] = &[
    "i",
    "you",
    "the",
    "me",
    "a",
    "no",
    "when",
    "my",
    "is",
    "it",
    "to",
    "lol",
    "this",
    "what",
    "not",
    "be",
    "that",
    "so",
    "just",
    "like",
    "yes",
    "why",
    "do",
    "don't",
    "i'm",
    "can't",
    "it's",
    "we",
    "they",
    "are",
    "how",
    "ok",
    "get",
    "go",
    "have",
    "love",
    "omg",
    "bruh",
    "lmao",
    "wtf",
    "good",
    "bad",
    "nice",
    "fine",
    "thoughts",
    "head",
    "empty",
    "monday",
    "friday",
    "weekend",
    "work",
    "code",
    "bug",
    "fix",
    "works",
    "deploy",
    "prod",
    "coffee",
    "sleep",
    "tired",
    "sad",
    "happy",
    "angry",
    "mood",
    "vibe",
    "irl",
    "meme",
    "stonks",
    "sus",
    "based",
    "cringe",
    "literally",
    "actually",
    "everyone",
    "nobody",
    "someone",
    "people",
    "time",
    "day",
    "night",
    "today",
    "tomorrow",
    "never",
    "always",
    "again",
    "more",
    "help",
    "stop",
    "wait",
    "hello",
    "bye",
    "thanks",
    "please",
    "sorry",
    "wow",
    "yeah",
    "nope",
    "maybe",
    "think",
    "know",
    "want",
    "need",
    "see",
    "look",
    "feel",
    "make",
    "take",
    "come",
    "say",
    "tell",
    "fire",
    "dead",
    "big",
    "small",
    "chungus",
    "boss",
    "king",
    "queen",
    "brain",
    "smol",
    "chonk",
    "doge",
    "much",
    "such",
    "cat",
    "dog",
    "lol",
    "hmm",
    "hehe",
    "haha",
    "pls",
    "fr",
    "ngl",
    "tbh",
    "idk",
    "rn",
    "gonna",
    "gotta",
    "really",
    "very",
    "too",
    "all",
    "one",
    "two",
    "three",
    "first",
    "last",
    "best",
    "worst",
    "new",
    "old",
    "real",
    "fake",
    "true",
    "false",
    "home",
    "school",
    "teacher",
    "student",
    "exam",
    "math",
    "money",
    "rich",
    "poor",
];

/// Ukrainian caption words, the most used first (`'` stands for every
/// apostrophe form).
pub const UKRAINIAN: &[&str] = &[
    "я",
    "не",
    "ти",
    "і",
    "що",
    "коли",
    "це",
    "так",
    "ні",
    "мене",
    "ми",
    "а",
    "але",
    "як",
    "в",
    "у",
    "на",
    "все",
    "вже",
    "ще",
    "тут",
    "мій",
    "моя",
    "кіт",
    "котик",
    "пес",
    "кава",
    "робота",
    "понеділок",
    "п'ятниця",
    "субота",
    "неділя",
    "вихідні",
    "сон",
    "спати",
    "їсти",
    "їжа",
    "любов",
    "люблю",
    "кохаю",
    "дякую",
    "будь",
    "ласка",
    "привіт",
    "бувай",
    "добре",
    "погано",
    "сумно",
    "весело",
    "смішно",
    "лол",
    "ахаха",
    "капець",
    "жах",
    "нічого",
    "щось",
    "завжди",
    "ніколи",
    "знову",
    "сьогодні",
    "завтра",
    "вчора",
    "ранок",
    "вечір",
    "ніч",
    "день",
    "тиждень",
    "м'ясо",
    "п'ять",
    "дев'ять",
    "пам'ять",
    "сім'я",
    "ім'я",
    "з'їсти",
    "об'єкт",
    "україна",
    "україні",
    "києві",
    "слава",
    "героям",
    "доброго",
    "вечора",
    "з",
    "до",
    "від",
    "для",
    "без",
    "про",
    "мама",
    "тато",
    "бабуся",
    "дід",
    "друг",
    "друзі",
    "життя",
    "думки",
    "голова",
    "порожня",
    "мішку",
    "кря",
    "мяу",
    "гав",
    "їжак",
    "їхати",
    "їде",
    "йти",
    "йде",
    "там",
    "можна",
    "треба",
    "хочу",
    "можу",
    "знаю",
    "думаю",
    "бачу",
    "кажу",
    "гарно",
    "красиво",
    "він",
    "вона",
    "вони",
    "де",
    "чому",
    "хто",
    "той",
    "тебе",
    "твій",
    "собака",
    "котики",
    "колись",
    "коливання",
    "школа",
    "вчитель",
    "іспит",
    "гроші",
    "дім",
    "новий",
    "старий",
    "перший",
    "останній",
    "найкращий",
    "правда",
    "брехня",
];

/// Letters OCR reads as others: Cyrillic and Latin look-alikes, `й` for `ї`.
const LOOK_ALIKES: &[(char, char)] = &[
    ('о', 'o'),
    ('а', 'a'),
    ('е', 'e'),
    ('і', 'i'),
    ('с', 'c'),
    ('р', 'p'),
    ('к', 'k'),
    ('ї', 'й'),
    ('o', 'о'),
    ('a', 'а'),
    ('e', 'е'),
    ('i', 'і'),
    ('l', '1'),
    ('o', '0'),
    ('m', 'n'),
    ('n', 'h'),
];

const SYLLABLES: [&str; 30] = [
    "ka", "to", "mi", "ne", "ru", "sa", "lo", "pe", "chu", "gus", "ba", "di", "fo", "zi", "wa", "qui", "ko", "ra",
    "te", "ni", "mo", "su", "ve", "li", "da", "ho", "je", "ya", "pu", "xe",
];

const EMOJIS: [&str; 10] = ["😂", "😭", "🐱", "❤️", "👍🏽", "🔥", "😴", "🤔", "💀", "🥺"];

const VIBES: [&str; 8] = [
    "happy",
    "sad",
    "sarcastic",
    "angry",
    "tired",
    "cringe",
    "wholesome",
    "dismissive",
];

/// A word of the caption as a person wrote it: one of the apostrophe
/// forms in place of `'`.
fn written(word: &str, rng: &mut Rng) -> String {
    let apostrophe = *rng.pick(&['\'', '\u{2019}', '\u{02BC}']);
    word.replace('\'', &apostrophe.to_string())
}

/// `word` as OCR may read it: one letter for a look-alike.
fn misread(word: &str, rng: &mut Rng) -> String {
    let chars: Vec<char> = word.chars().collect();
    let spots: Vec<(usize, char)> = chars
        .iter()
        .enumerate()
        .filter_map(|(i, c)| LOOK_ALIKES.iter().find(|(a, _)| a == c).map(|(_, b)| (i, *b)))
        .collect();
    if spots.is_empty() {
        return word.to_string();
    }
    let (at, to) = *rng.pick(&spots);
    chars
        .iter()
        .enumerate()
        .map(|(i, c)| if i == at { to } else { *c })
        .collect()
}

/// The words written in one picture: empty for a quarter of them.
pub fn caption(rng: &mut Rng) -> String {
    if rng.chance(0.25) {
        return String::new();
    }
    let lang = rng.below(10);
    let n = 2 + rng.below(11);
    let mut words: Vec<String> = Vec::with_capacity(n + 1);
    for _ in 0..n {
        let list = match lang {
            0..=4 => ENGLISH,
            5..=8 => UKRAINIAN,
            _ if rng.chance(0.5) => ENGLISH,
            _ => UKRAINIAN,
        };
        let mut w = written(list[rng.common(list.len())], rng);
        if rng.chance(0.03) {
            w = misread(&w, rng);
        }
        words.push(w);
    }
    if rng.chance(0.08) {
        // A number, or a word no dictionary has (a name, slang, OCR noise).
        let odd = match rng.below(3) {
            0 => format!("{}", rng.below(10_000)),
            _ => (0..2 + rng.below(3)).map(|_| *rng.pick(&SYLLABLES)).collect(),
        };
        words.insert(rng.below(words.len() + 1), odd);
    }
    // OCR joins neighbours now and then: `КОЛИ В` read as `КОЛИВ`.
    let mut text = String::new();
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            let sep = match rng.below(100) {
                0..=3 => "",
                4..=12 => "\n",
                _ => " ",
            };
            text.push_str(sep);
        }
        text.push_str(w);
    }
    if rng.chance(0.15) {
        text.push_str(*rng.pick(&["!", "?", "...", "!!!", " 😂", "."]));
    }
    if rng.chance(0.5) {
        text = text.to_uppercase();
    }
    text
}

/// `n` docs; the same `seed` gives the same docs.
pub fn library(n: usize, seed: u64) -> Vec<Doc> {
    let mut rng = Rng(seed);
    let mut words: Vec<String> = (0..40_000)
        .map(|_| (0..2 + rng.below(3)).map(|_| *rng.pick(&SYLLABLES)).collect::<String>())
        .collect();
    words.sort();
    words.dedup();
    (0..n)
        .map(|i| {
            let w = |k: usize, rng: &mut Rng| (0..k).map(|_| rng.pick(&words).clone()).collect::<Vec<_>>();
            let k = 1 + rng.below(3);
            let name = w(k, &mut rng).join(" ");
            let k = 2 + rng.below(5);
            let keywords = w(k, &mut rng);
            let k = rng.below(3);
            let tags = w(k, &mut rng);
            let set = rng.below(300);
            Doc {
                item_id: format!("item{i:06}"),
                set_id: format!("set{set}"),
                set_slug: format!("set-{set}"),
                set_name: format!("Set {}", words[set]),
                owner_id: format!("user{}", set % 20),
                owner_username: format!("User{}", set % 20),
                visibility: [Visibility::Private, Visibility::Unlisted, Visibility::Public][set % 3],
                name,
                keywords,
                emojis: (0..rng.below(3)).map(|_| rng.pick(&EMOJIS).to_string()).collect(),
                vibe: Some(rng.pick(&VIBES).to_string()),
                tags,
                text: caption(&mut rng),
                kind: MediaKind::Animation,
                format: MediaFormat::Gif,
                quality: Some(rng.below(101) as u8),
                likes: rng.below(50) as u64,
                copies: rng.below(500) as u64,
                media_id: format!("media{i}"),
                embedding: None,
                updated_at: rng.below(1_000_000) as i64,
            }
        })
        .collect()
}

/// A doc as a client caches it.
pub fn summary(d: &Doc) -> ItemSummary {
    ItemSummary {
        id: d.item_id.clone(),
        set_id: d.set_id.clone(),
        set_slug: d.set_slug.clone(),
        set_name: d.set_name.clone(),
        owner_username: d.owner_username.clone(),
        name: d.name.clone(),
        keywords: d.keywords.clone(),
        emojis: d.emojis.clone(),
        vibe: d.vibe.clone(),
        tags: d.tags.clone(),
        text: d.text.clone(),
        kind: d.kind,
        format: d.format,
        width: 512,
        height: 512,
        quality: d.quality,
        likes: d.likes,
        copies: d.copies,
        media_id: d.media_id.clone(),
        sha256: String::new(),
        phash: String::new(),
        variants: Vec::new(),
        updated_at: d.updated_at,
    }
}

/// The docs one viewer (`user0`) can search: theirs and the public ones,
/// as a client's cache holds them (about 18,000 of 50,000).
pub fn visible_to_user0(docs: &[Doc]) -> Vec<&Doc> {
    docs.iter()
        .filter(|d| d.owner_id == "user0" || d.visibility == Visibility::Public)
        .collect()
}
