use super::*;

#[test]
fn parses_codes_and_locales() {
    assert_eq!(Lang::parse("uk"), Some(Lang::Uk));
    assert_eq!(Lang::parse("uk_UA.UTF-8"), Some(Lang::Uk));
    assert_eq!(Lang::parse("UK-ua"), Some(Lang::Uk));
    assert_eq!(Lang::parse("ua"), Some(Lang::Uk));
    assert_eq!(Lang::parse("en-GB"), Some(Lang::En));
    assert_eq!(Lang::parse("de"), None);
    assert_eq!(Lang::parse(""), None);
    for l in Lang::ALL {
        assert_eq!(Lang::parse(l.code()), Some(l));
        assert_eq!(serde_json::to_string(&l).unwrap(), format!("\"{}\"", l.code()));
    }
}

#[test]
fn accept_language_by_quality() {
    assert_eq!(Lang::from_accept_language("uk-UA,uk;q=0.9,en;q=0.8"), Some(Lang::Uk));
    assert_eq!(Lang::from_accept_language("de-DE,en;q=0.5,uk;q=0.7"), Some(Lang::Uk));
    assert_eq!(Lang::from_accept_language("en, uk"), Some(Lang::En), "first on a tie");
    assert_eq!(Lang::from_accept_language("uk;q=0, en;q=0.1"), Some(Lang::En));
    assert_eq!(Lang::from_accept_language("de, fr"), None);
    assert_eq!(Lang::from_accept_language(""), None);
}

#[test]
fn locale_environment() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    };
    assert_eq!(Lang::from_env(env(&[("LANG", "uk_UA.UTF-8")])), Some(Lang::Uk));
    assert_eq!(
        Lang::from_env(env(&[("LANG", "uk_UA.UTF-8"), ("LC_ALL", "en_US.UTF-8")])),
        Some(Lang::En)
    );
    assert_eq!(
        Lang::from_env(env(&[("LANGUAGE", "uk:en"), ("LANG", "en_US.UTF-8")])),
        Some(Lang::Uk)
    );
    assert_eq!(Lang::from_env(env(&[("LANG", "C")])), None);
    assert_eq!(Lang::from_env(env(&[])), None);
}

#[test]
fn translates_with_fallback() {
    assert_eq!(t(Lang::Uk, "Sets"), "Набори");
    assert_eq!(t(Lang::En, "Sets"), "Sets");
    assert_eq!(t(Lang::Uk, "no such text"), "no such text");
    assert_eq!(tf(Lang::Uk, "by {owner}", &[("owner", &"alice")]), "від alice");
    assert_eq!(tf(Lang::En, "by {owner}", &[("owner", &"alice")]), "by alice");
}

#[test]
fn ukrainian_plurals() {
    let forms: Vec<usize> = [0u64, 1, 2, 4, 5, 11, 12, 14, 15, 21, 22, 25, 101, 111, 112]
        .iter()
        .map(|n| plural_form(Lang::Uk, *n))
        .collect();
    assert_eq!(forms, [2, 0, 1, 1, 2, 2, 2, 2, 2, 0, 1, 2, 0, 2, 2]);
    assert_eq!(tp(Lang::Uk, 1, "{n} sticker", "{n} stickers"), "1 стікер");
    assert_eq!(tp(Lang::Uk, 3, "{n} sticker", "{n} stickers"), "3 стікери");
    assert_eq!(tp(Lang::Uk, 11, "{n} sticker", "{n} stickers"), "11 стікерів");
    assert_eq!(tp(Lang::En, 1, "{n} sticker", "{n} stickers"), "1 sticker");
    assert_eq!(tp(Lang::En, 0, "{n} sticker", "{n} stickers"), "0 stickers");
    assert_eq!(tp(Lang::Uk, 2, "{n} thing", "{n} things"), "2 things", "untranslated");
}

#[test]
fn catalogs_are_well_formed() {
    for lang in Lang::ALL {
        let mut seen = std::collections::HashSet::new();
        for (i, file) in catalog_files(lang).iter().enumerate() {
            let (entries, errors) = parse_catalog(file);
            assert!(errors.is_empty(), "{lang} file #{i}: {errors:?}");
            for (en, tr) in entries {
                assert!(seen.insert(en.clone()), "{lang}: {en:?} twice");
                let holes = |s: &str| {
                    let mut v: Vec<String> = s
                        .split('{')
                        .skip(1)
                        .filter_map(|p| p.split_once('}').map(|(n, _)| n.to_string()))
                        .collect();
                    v.sort();
                    v.dedup();
                    v
                };
                assert_eq!(holes(&en), holes(&tr), "{lang}: placeholders of {en:?}");
                if let Some((one, many)) = en.split_once('|') {
                    assert!(!one.is_empty() && !many.is_empty(), "{en:?}");
                    let n = tr.split('|').count();
                    let want = if lang == Lang::Uk { 3 } else { 2 };
                    assert_eq!(n, want, "{lang}: plural forms of {en:?}");
                }
            }
        }
    }
}

#[test]
fn finds_keys_in_source() {
    let src = r#"
        p { (t("Sets")) " " (tf("by {owner}", &[("owner", &o)])) }
        (tp(n, "{n} sticker", "{n} stickers")) (plural(n, "item", "items"))
        Err(AppError::forbidden("you may not \"do\" this \
             at all"))
        let not_this = format!("x");
        set_t("not a call");
    "#;
    assert_eq!(
        source_keys(src),
        [
            "Sets",
            "by {owner}",
            "{n} sticker|{n} stickers",
            "{n} item|{n} items",
            "you may not \"do\" this at all"
        ]
    );
}

/// Texts of the `.rs` files under `crates/<crate>/src` without a translation
/// into `lang` (tests' own strings, after `#[cfg(test)]`, are left out).
fn untranslated(lang: Lang, krate: &str) -> Vec<String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(krate)
        .join("src");
    let mut missing = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            let src = src.split("#[cfg(test)]").next().unwrap_or(&src);
            for key in source_keys(src) {
                if lookup(lang, &key).is_none() {
                    missing.push(format!("{}: {key:?}", path.display()));
                }
            }
        }
    }
    missing.sort();
    missing.dedup();
    missing
}

/// Every text the web UI and the server's messages mark for translation
/// has a Ukrainian one.
#[test]
fn ukrainian_covers_the_server() {
    let missing = untranslated(Lang::Uk, "stickpile-server");
    assert!(
        missing.is_empty(),
        "{} texts without Ukrainian:\n{}",
        missing.len(),
        missing.join("\n")
    );
}

#[test]
fn finds_keys_in_scripts() {
    let src = r#"toast(tr("Link copied.")); x.tr("no"); trf("Copied {name}.", { name: n }); str("no")"#;
    assert_eq!(js_keys(src), ["Link copied.", "Copied {name}."]);
}

/// Every `tr(...)` / `trf(...)` text of the web UI's scripts has a Ukrainian one.
#[test]
fn ukrainian_covers_the_scripts() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../stickpile-server/assets");
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("js") || path.ends_with("htmx.min.js") {
            continue;
        }
        for key in js_keys(&std::fs::read_to_string(&path).unwrap()) {
            if lookup(Lang::Uk, &key).is_none() {
                missing.push(format!("{}: {key:?}", path.display()));
            }
        }
    }
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "{} script texts without Ukrainian:\n{}",
        missing.len(),
        missing.join("\n")
    );
}

/// Every text the desktop client and its popup model mark has a Ukrainian one.
#[test]
fn ukrainian_covers_the_desktop_client() {
    let mut missing = untranslated(Lang::Uk, "stickpile-quick");
    missing.extend(untranslated(Lang::Uk, "stickpile-desktop"));
    assert!(
        missing.is_empty(),
        "{} texts without Ukrainian:\n{}",
        missing.len(),
        missing.join("\n")
    );
}
