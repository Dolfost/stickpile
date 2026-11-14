# Backup and restore

`stickpile-server backup <dir>` writes a full backup of a server: every set
of every user as a plain [set directory](set-format.md) plus a snapshot of
the database. It needs no running server and is safe while one runs.
`stickpile-server restore <dir>` turns such a backup into the data directory
of a fresh server.

The implementation is `crates/stickpile-server/src/service/backup.rs`; the
tests are in `crates/stickpile-server/tests/integrations_backup.rs`.

## What a backup contains

```
<dir>/
  backup.json              time, versions, counts, per-set errors
  stickpile.db             snapshot of the database (one SQLite file)
  sets/
    alice/
      cats/                every set as a set directory
        set.toml
        001-grumpy-cat.png
        002-naahh.gif
      memes/
    bob/
      dogs/
  proposals/
    <media id>.png         files of open proposals (they are in no set)
```

- **`stickpile.db`**: everything the server knows. That is users and their
  password hashes, API token hashes, TOTP secrets, sessions, invitations,
  sets, items and all metadata, AI suggestions and embeddings, likes, usage
  events, the audit log, the job queue, git-pack bindings with their webhook
  secrets, Telegram pack bindings, and [contributors and
  proposals](collaboration.md).
- **`sets/<username>/<slug>/`**: each set in the
  [set directory format](set-format.md), exactly what
  `GET /api/v1/sets/{id}/export.tar` or `stickpile export` would write:
  `set.toml` (with the AI source markers and unknown keys) and the stored
  originals, named `NNN-<name>.<ext>` in set order, plus `cover.<ext>` for
  a set with a cover image.
- **`proposals/<media id>.<ext>`**: the stored originals of the stickers
  and cover images of open proposals, which belong to no set yet. A restore
  writes them back, so the proposals can still be accepted.
- **`backup.json`**:

  ```json
  {
    "format": 1,
    "created_at": 1794268800,
    "stickpile_version": "0.3.0",
    "schema_version": 2,
    "users": 3, "sets": 12, "items": 410, "media": 398,
    "bytes": 81234567,
    "errors": []
  }
  ```

  `errors` lists sets that could not be exported (`username/slug: reason`).
  The command then exits non-zero, but the database snapshot is complete.

Not included, because they are derived or are not data: generated variants
and thumbnails (`media/`), git-pack checkouts (`gitpacks/`), `tmp/`, the CLIP
models, and your configuration and secret files. **Back up the configuration
and the secret files (`*_file` options) separately.**

## Consistency while the server runs

1. The database snapshot is taken with SQLite's `VACUUM INTO`. It runs in a
   single read transaction. In WAL mode it neither blocks the server nor
   sees half of a concurrent write: a change is either completely in the
   snapshot or not at all.
2. The set directories are exported **from the snapshot**, not from the live
   database, so the directories and `stickpile.db` always describe the same
   moment. The files come from `<data_dir>/media`. They are
   content-addressed and only removed when no item references them, so a set
   that is deleted while the backup runs is the only case that can produce an
   entry in `errors`.
3. `backup.json` is written last. A directory without it is an unfinished
   backup.

The command opens the database itself and runs no migrations, so it works
whether or not the server is running. Run it as the service user, because it
must read `<data_dir>`.

## Target directory and permissions

The target must be missing, empty, or an earlier backup, meaning it holds
nothing but `stickpile.db`, `sets/` and `backup.json`. An earlier backup is
replaced. Anything else is refused, so a typo cannot wipe an unrelated
directory.

The snapshot contains password and token hashes, TOTP secrets and webhook
secrets. The backup directory is therefore created with mode `0700` and
`stickpile.db` with `0600`. Keep it at least that private when you copy it
elsewhere.

## Running it

```sh
# NixOS (the module puts the config at /etc/stickpile/config.toml)
sudo -u stickpile stickpile-server backup /var/backup/stickpile/$(date +%F)

# Container: the target must be on a writable volume
podman exec stickpile stickpile-server backup /var/lib/stickpile/backups/$(date +%F)
```

Output:

```
Backed up 12 sets (410 items) of 3 users and the database to /var/backup/stickpile/2026-11-10
```

A systemd timer or cron job that runs the command and then ships the
directory with restic or borg works well. Set directories are plain files
and diff well. Unchanged media files are left alone when a backup is
rewritten in place, so incremental backup tools see few changes.

## Restore

### A complete server: `stickpile-server restore`

On a new machine, or after losing the data directory:

1. Install the same or a newer stickpile version and write its
   configuration. `server.data_dir` must not contain a database yet.
2. Restore, then start the server:

   ```sh
   stickpile-server --config /etc/stickpile/config.toml restore /var/backup/stickpile/2026-11-10
   systemctl start stickpile
   ```

`restore` copies the snapshot into `<data_dir>/stickpile.db`, where the
migrations of a newer version run. It writes every stored original back to
`<data_dir>/media` from the set directories, after checking it against the
sha256 recorded in the database. It regenerates the thumbnails and queues a
`Variants` job per media item, which runs as soon as the server is up (cover
images need none: they only have the original and the thumbnail).
Accounts, API tokens (clients stay logged in), likes, usage statistics, AI
suggestions and embeddings, git-pack and Telegram bindings all come back. Git
packs are cloned again on their next pull. Media that no set directory
provides are reported as warnings, and their items stay without files.

`restore` refuses to overwrite an existing database.

### Individual sets: re-import the directories

Every `sets/<user>/<slug>/` directory is a normal set directory. It can be
imported into any stickpile server, or into another account, with
`stickpile import <dir>`, the web UI's folder upload or
`POST /api/v1/sets/import`. Metadata, AI source markers, unknown `set.toml`
keys and the order survive, and exporting the imported set again gives the
same manifest and byte-identical files (a test checks this). What does not
come along is everything outside the set: likes, usage, other users and the
original item ids.

Use this to move a single set, to migrate between servers that should not
share accounts, or to recover a few sets from an older backup into a running
server.

## Relation to backing up the data directory

| | `stickpile-server backup` | Copy of `<data_dir>` |
|---|---|---|
| Consistent while running | yes | only with care (stop the server, or snapshot the database with `sqlite3 stickpile.db ".backup …"` and copy `media/` afterwards) |
| Contents | database + originals as readable set directories | database + originals + every variant, thumbnail and checkout |
| Size | originals once per set they appear in | originals once (content-addressed) plus variants |
| Restore | `restore` (variants are regenerated) or per-set import | put the directory back |
| Useful without stickpile | yes: plain image files + `set.toml` | barely: hashed file names |

Both are valid. The `backup` command is the recommended way, because it is
consistent without stopping anything and its output stays useful even
without a stickpile server. A file-level backup of the data directory
restores faster on large libraries, because nothing has to be re-rendered.
The client-side `stickpile backup` (CLI) is a third, narrower option: it
exports only the caller's own sets as directories, with no database.
