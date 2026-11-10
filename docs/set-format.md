# Set directory format

A stickpile set can be written out as a plain directory and read back in. The
format is used for export (`GET /api/v1/sets/{id}/export.tar`, CLI export),
import (folder or tar upload, CLI import) and git packs
([git-packs.md](git-packs.md)).

It is an **import/export format, not a live mirror**. The database is the
source of truth. Editing in the web UI changes the database, and exporting
writes a fresh directory. Nothing syncs back from a directory on its own.

The implementation is `crates/stickpile-setfmt`. It has no I/O beyond the
directory or archive it is given.

```
my-cats/
  set.toml
  001-cat.png
  002-naahh.gif
  003-sigh.mp4
```

## Media files

- Allowed extensions (case-insensitive): `png jpg jpeg webp gif mp4 webm`.
  The server still sniffs the content and never trusts the extension.
- **Order** is the lexical order of the filenames: a byte-wise comparison of
  the UTF-8 names, as `LC_ALL=C ls` sorts them. The numeric prefix is only a
  convention that makes this order visible. `10-x.png` sorts before
  `9-y.png`, so use zero padding.
- On export the files are named `NNN-<slug>.<ext>`. `NNN` is the 1-based
  position, zero-padded to `max(3, digits(item count))`. `<slug>` is the item
  name reduced to `[a-z0-9-]`; accents are folded (`Café` becomes `cafe`), and
  a name with no ASCII letters or digits becomes `item`. See
  `export_filename`.
- On import, an item without its own `name` is named after the filename
  without the ordering prefix and extension: `001-cat.png` gives `cat`,
  `12_-dog.gif` gives `dog`, `cat.png` gives `cat`. A prefix is a run of
  digits followed by `-`, `_`, `.` or a space, so `2fast.gif` stays `2fast`.
  See `display_stem`.

## set.toml

```toml
version = 3                    # bumped on every change (see below)
name = "My cats"
slug = "my-cats"               # optional; [a-z0-9-]
description = "Cats, mostly staring."
visibility = "unlisted"        # private (default) | unlisted | public
aspect_ratio = "1:1"           # optional, "W:H"
recommended_px = 512           # optional, longest side for new items

[items."001-cat.png"]
name = "cat"
keywords = ["cat", "stare"]
keywords_source = "ai"
emojis = ["😼"]
vibe = "smug"
vibe_source = "ai"
tags = ["reaction"]

[items."002-naahh.gif"]
name = "naahh"
emojis = ["🙅"]
```

| Key | Type | Default | Notes |
|---|---|---|---|
| `version` | integer | `1` | See [Versions](#versions). |
| `name` | string | directory name | The set's display name. |
| `slug` | string | derived from the name | A hint for the URL and export name. The server keeps slugs unique per owner. |
| `description` | string | `""` | |
| `visibility` | `private` / `unlisted` / `public` | `private` | |
| `aspect_ratio` | `"W:H"` | none | Recommended aspect ratio for new items. If it does not parse, import warns and ignores it. |
| `recommended_px` | integer | none | Recommended longest side in pixels. |
| `items."<filename>"` | table | | One table per media file, keyed by the bare filename (no path). |

Item keys:

| Key | Type | Notes |
|---|---|---|
| `name` | string | Defaults to the display stem of the filename. |
| `keywords` | array of strings | |
| `emojis` | array of strings | |
| `vibe` | string | One label from the vibe vocabulary. |
| `tags` | array of strings | Always user-set, never AI. |
| `name_source`, `keywords_source`, `emojis_source`, `vibe_source` | `"user"` / `"ai"` | See below. |

### AI markers

Every field that AI enrichment can fill has a `<field>_source` marker. It is
`"ai"` while the value is an unconfirmed AI suggestion and becomes `"user"`
when someone edits or confirms it. `"user"` is the default, so it is not
written. A marker missing from `set.toml` therefore means user data. Markers
survive every round trip, so exporting a set and importing it on another
server keeps track of which values a human has checked.

### Unknown keys are preserved

Keys that stickpile does not know are kept verbatim, at the top level and
inside every item table. This includes nested tables, arrays of tables and
datetimes. A pack author can add `author = "…"`, `license = "CC-BY-4.0"` or
`[items."001-cat.png".credits]`, and those survive import, export and
re-import.

Comments and formatting are not kept, because they are not data. Exported
files use a canonical layout: known keys in a fixed order, then unknown keys,
then items sorted by filename.

### Round-trip guarantees

These are checked by tests in `crates/stickpile-setfmt/tests`, including a
property-style test over 500 generated manifests:

- `parse(serialise(m)) == m` for every manifest.
- Serialising twice gives byte-identical text, so exports are deterministic
  and diff cleanly in git.
- `read_dir(write_dir(bundle)) == bundle`, and the same holds for
  `write_tar`/`read_tar`.
- Unknown keys and `*_source = "ai"` markers are never dropped.

### Versions

`version` is bumped on every metadata or content change made through
stickpile, and an export writes the current value. For git packs the version
tells clients and the server whether a directory comes from an upstream update
or from a local edit. See [git-packs.md](git-packs.md#version).

## Import rules

The rules are the same for a directory, a tar archive and a multipart folder
upload:

- `set.toml` is optional. Without it, the set is named after the directory
  (or the archive's top-level directory) and every file gets default
  metadata.
- Files are matched to `[items."…"]` tables by exact name first, then by
  Unicode NFC form. A `set.toml` written on Linux therefore still matches
  files copied from macOS, which stores decomposed names.
- Hidden entries (a leading `.`, which includes `.git/` and `.DS_Store`) are
  skipped without a warning.
- Each of these is skipped with a warning in the import report: files with a
  non-media extension, subdirectories, symlinks, special files, names that
  are not UTF-8 or not safe (see below), `set.toml` entries whose file is
  missing, and a malformed `aspect_ratio`.
- A `set.toml` that does not parse is an error. Media files are never
  imported half-described.
- `set.toml` may be at most 4 MiB.

Directory specifics: symlinks are never followed, including a symlinked
`set.toml`, and files are opened with `O_NOFOLLOW`.

Folder upload specifics: each path may have one leading directory
(`cats/001-cat.png`). A `set.toml` part sent separately takes precedence over
one inside the folder.

### Tar archives

`write_tar` produces an uncompressed tar with one top-level directory
`<slug>/`. It contains `set.toml` first, then the media files in set order.
It is reproducible: GNU headers (long names supported), mode 0644 for files
and 0755 for the directory, uid/gid 0, empty owner names, mtime 1.

`read_tar` accepts that layout, a flat archive without a top-level directory,
`./`-prefixed paths and `git archive` output (its pax global header is
skipped). It rejects the whole archive on any of these:

- absolute paths, `..` components, backslashes or NUL in paths;
- symlinks, hard links, device nodes, FIFOs and sparse files (only regular
  files and directories are allowed);
- nesting deeper than one top-level directory, more than one top-level
  directory, or files both inside and outside it;
- duplicate entries, including names that are equal after NFC
  normalisation, and a second `set.toml`;
- more than `max_total_bytes` of extracted data. The count is the bytes
  actually read, not the sizes the headers claim.

## Safe filenames

Every name taken from a manifest, directory, archive or upload must pass
`safe_filename` before it is used:

- exactly one path component: no `/` or `\`;
- not empty, not `.` or `..`, and not hidden (no leading `.`);
- no NUL, no other control characters, no bidirectional-override characters;
- at most 255 bytes, in both the given form and the NFC form.

The checks run on the NFC-normalised form too. Two names that differ only in
normalisation count as the same file.

## Export behaviour

- `write_dir` creates the directory and writes `set.toml` and the media
  files. Each file is written to a temporary file and renamed into place, so
  a symlink planted at a target path is replaced, not followed. Files whose
  content has not changed are left alone.
- Media files in the directory that are not part of the export are deleted,
  so the directory matches the set exactly. Only regular files and symlinks
  with a media extension are deleted. `.git/`, other hidden files,
  subdirectories and non-media files such as `README.md` or `LICENSE` stay.
- The `[items]` tables are rebuilt from the exported files. Metadata of items
  that are no longer in the set is not written.
- A bundle with an unsafe, duplicate or non-media filename is refused before
  anything is written.
