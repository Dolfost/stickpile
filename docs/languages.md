# Languages

The web UI and the desktop popup are translated into Ukrainian; English is
the default. Each user chooses a language, and the choice travels between
the web UI and the desktop client through their account.

## Choosing a language

**Web UI.** Settings, under Appearance: **Language** is "Automatic (your
browser's language)", English or Українська. The choice is saved with the
account. With "Automatic", and on pages before signing in, the browser's
`Accept-Language` decides; a language that is not translated falls back to
English.

**Desktop popup.** The Settings tab's **Language** row (`ui.language` in
`config.toml`):

- `auto` (the default) uses the account's language when it has one (the
  web UI's setting, fetched when the popup starts and when the Me tab
  loads, and kept in the cache for offline starts), else the system's
  locale (`LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG`), else English;
- `en` or `uk` uses that language, and choosing it in the popup also sets
  the account's language, so the web UI follows.

The popup switches at once, without a restart.

The CLI stays in English.

## API

- `User.language` (`GET /api/v1/me`) is the account's language (`en`,
  `uk`), or `null` for automatic.
- `PATCH /api/v1/me` with `{"language": "uk"}` sets it; `null` goes back to
  automatic. Other codes are a 400.
- Every response has `Content-Language`. Error messages (`ApiError.message`)
  are in the request's language when the catalog has them: the signed-in
  user's choice, else `Accept-Language`, else English.

## How it works

`crates/stickpile-i18n` holds the catalogs and the lookup. The English text
is the key: code writes `t("Sets")`, and a language other than English
looks it up in its catalog, falling back to the English.

- Catalogs: `crates/stickpile-i18n/catalog/uk/*.tsv`, compiled in. One entry
  per line: the English, a tab, the translation; `#` comments; `\t`, `\n`
  and `\\` escapes. Files by area: `common`, `web*`, `server` (error
  messages), `desktop` and `js` (the web UI's scripts).
- Placeholders: `{name}` in both texts, filled by `tf`.
- Plurals: the key is `one|many` (`{n} sticker|{n} stickers`); the
  translation lists the language's forms, three for Ukrainian (one, few,
  many: 1 стікер, 2 стікери, 5 стікерів).
- The server keeps the language per request (a task-local set by a
  middleware after authentication); `crate::i18n::{t, tf, tp, t_dyn}` read
  it. The web UI's `<html lang>` says which, and the scripts get their
  texts as JSON in `<body data-i18n>` (`tr()` / `trf()` in each script).
- The desktop client keeps one language for the process
  (`stickpile_i18n::global`); widgets built once are relabelled when it
  changes.

**Coverage tests** keep the catalogs complete: `cargo test -p
stickpile-i18n` finds every `t(`, `tf(`, `tp(` and `plural(` text and every
literal `AppError` message in the server, the popup model and the desktop
client, and every `tr(` / `trf(` text of the scripts, and fails when
Ukrainian lacks one. `catalogs_are_well_formed` checks the files: no key
twice, the same placeholders, three plural forms. The desktop crate checks
the Settings rows and the key help, which are not literals.

## Adding a language

1. Add the variant to `Lang` (`code`, `native_name`, `parse`,
   `plural_form` for its plural rule) in `crates/stickpile-i18n/src/lib.rs`
   and to the global language's mapping.
2. Add `catalog/<code>/*.tsv` with the same files and list them in
   `named_sources`.
3. Extend `ui.language` (`LANGUAGES` in `crates/stickpile-core/src/config.rs`
   and the home-manager option) and the coverage tests to the new language.
