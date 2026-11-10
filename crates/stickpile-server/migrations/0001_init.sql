-- stickpile schema v1 (SQLite). Portable-SQL conventions so a PostgreSQL
-- backend can be added post-v1: app-generated TEXT ids, INTEGER unix-second
-- timestamps, INTEGER 0/1 booleans, JSON arrays stored as TEXT.

CREATE TABLE users (
    id               TEXT PRIMARY KEY,
    username         TEXT NOT NULL UNIQUE COLLATE NOCASE,
    display_name     TEXT NOT NULL,
    password_hash    TEXT,                 -- argon2id PHC string; NULL = no password login
    totp_secret      TEXT,                 -- base32; NULL = TOTP disabled
    role             TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('user', 'admin')),
    tailscale_login  TEXT UNIQUE,
    telegram_user_id INTEGER,              -- owner for Telegram pack export
    disabled         INTEGER NOT NULL DEFAULT 0,
    created_at       INTEGER NOT NULL,
    last_login_at    INTEGER
);

CREATE TABLE sessions (
    id           TEXT PRIMARY KEY,         -- sha256 hex of the cookie secret
    user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    csrf_token   TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    ip           TEXT,
    user_agent   TEXT
);
CREATE INDEX sessions_user ON sessions(user_id);

CREATE TABLE api_tokens (
    id           TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    token_hash   TEXT NOT NULL UNIQUE,     -- sha256 hex of the secret
    prefix       TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER,
    expires_at   INTEGER,
    revoked_at   INTEGER
);
CREATE INDEX api_tokens_user ON api_tokens(user_id);

CREATE TABLE invitations (
    id         TEXT PRIMARY KEY,
    code_hash  TEXT NOT NULL UNIQUE,
    role       TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('user', 'admin')),
    created_by TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER,
    used_by    TEXT REFERENCES users(id) ON DELETE SET NULL,
    used_at    INTEGER
);

-- Content-addressed media, shared by every item with the same upload bytes.
CREATE TABLE media (
    id                TEXT PRIMARY KEY,
    sha256            TEXT NOT NULL UNIQUE,
    kind              TEXT NOT NULL CHECK (kind IN ('still', 'animation', 'video')),
    format            TEXT NOT NULL CHECK (format IN ('png', 'jpeg', 'webp', 'gif', 'mp4', 'webm')),
    width             INTEGER NOT NULL,
    height            INTEGER NOT NULL,
    duration_ms       INTEGER,
    frame_count       INTEGER,
    fps               REAL,
    has_alpha         INTEGER NOT NULL DEFAULT 0,
    size_bytes        INTEGER NOT NULL,
    phash             INTEGER NOT NULL,    -- u64 stored as i64 bit pattern
    dhash             INTEGER NOT NULL,
    quality           INTEGER,
    quality_report    TEXT,                -- JSON QualityReport
    embedding         BLOB,                -- f32 LE, L2-normalised
    embedding_model   TEXT,
    ai_keywords       TEXT,                -- JSON [[kw, score]] last zero-shot output
    ai_emojis         TEXT,
    ai_vibe           TEXT,
    variants_done     INTEGER NOT NULL DEFAULT 0,
    created_at        INTEGER NOT NULL
);
CREATE INDEX media_phash ON media(phash);

CREATE TABLE media_variants (
    media_id   TEXT NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    variant    TEXT NOT NULL,
    mime       TEXT NOT NULL,
    width      INTEGER NOT NULL,
    height     INTEGER NOT NULL,
    size_bytes INTEGER NOT NULL,
    path       TEXT NOT NULL,              -- relative to <data_dir>/media
    PRIMARY KEY (media_id, variant)
);

CREATE TABLE sets (
    id                  TEXT PRIMARY KEY,
    owner_id            TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    slug                TEXT NOT NULL,
    name                TEXT NOT NULL,
    description         TEXT NOT NULL DEFAULT '',
    visibility          TEXT NOT NULL DEFAULT 'private' CHECK (visibility IN ('private', 'unlisted', 'public')),
    aspect_w            INTEGER,
    aspect_h            INTEGER,
    recommended_px      INTEGER,
    version             INTEGER NOT NULL DEFAULT 1,
    source              TEXT NOT NULL DEFAULT 'local' CHECK (source IN ('local', 'git', 'telegram')),
    git_url             TEXT,
    git_branch          TEXT,
    git_subdir          TEXT,
    git_mode            TEXT CHECK (git_mode IN ('read_only', 'push')),
    git_webhook_secret  TEXT,              -- needed in clear to verify HMAC signatures
    git_last_commit     TEXT,
    git_last_pull_at    INTEGER,
    git_last_error      TEXT,
    telegram_pack       TEXT,
    telegram_mapping    TEXT,              -- JSON PackMapping
    extra_toml          TEXT,              -- unknown set.toml keys, preserved for export
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    UNIQUE (owner_id, slug)
);

CREATE TABLE items (
    id               TEXT PRIMARY KEY,
    set_id           TEXT NOT NULL REFERENCES sets(id) ON DELETE CASCADE,
    media_id         TEXT NOT NULL REFERENCES media(id),
    position         INTEGER NOT NULL,
    filename         TEXT NOT NULL,        -- export stem, e.g. 001-cat
    name             TEXT NOT NULL DEFAULT '',
    name_source      TEXT NOT NULL DEFAULT 'user' CHECK (name_source IN ('user', 'ai')),
    keywords         TEXT NOT NULL DEFAULT '[]',
    keywords_source  TEXT NOT NULL DEFAULT 'user' CHECK (keywords_source IN ('user', 'ai')),
    emojis           TEXT NOT NULL DEFAULT '[]',
    emojis_source    TEXT NOT NULL DEFAULT 'user' CHECK (emojis_source IN ('user', 'ai')),
    vibe             TEXT,
    vibe_source      TEXT NOT NULL DEFAULT 'user' CHECK (vibe_source IN ('user', 'ai')),
    tags             TEXT NOT NULL DEFAULT '[]',
    source_url       TEXT,
    extra_toml       TEXT,                 -- unknown per-item set.toml keys
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    UNIQUE (set_id, media_id)
);
CREATE INDEX items_set ON items(set_id, position);
CREATE INDEX items_media ON items(media_id);
CREATE INDEX items_updated ON items(updated_at);

-- Tombstones so client caches learn about removals (ItemPage.removed).
CREATE TABLE item_tombstones (
    item_id    TEXT NOT NULL,
    owner_id   TEXT NOT NULL,
    removed_at INTEGER NOT NULL
);
CREATE INDEX item_tombstones_at ON item_tombstones(removed_at);

CREATE TABLE ratings (
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    item_id    TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, item_id)
);
CREATE INDEX ratings_item ON ratings(item_id);

CREATE TABLE usage_events (
    id          TEXT PRIMARY KEY,          -- client-generated, idempotency key
    user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    item_id     TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    set_id      TEXT NOT NULL,
    target      TEXT,
    variant     TEXT,
    client      TEXT,
    copied_at   INTEGER NOT NULL,
    received_at INTEGER NOT NULL
);
CREATE INDEX usage_item_time ON usage_events(item_id, copied_at);
CREATE INDEX usage_user_time ON usage_events(user_id, copied_at);
CREATE INDEX usage_time ON usage_events(copied_at);

CREATE TABLE jobs (
    id          TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,
    payload     TEXT NOT NULL,             -- JSON
    subject     TEXT,
    status      TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'done', 'failed')),
    attempts    INTEGER NOT NULL DEFAULT 0,
    error       TEXT,
    result      TEXT,                      -- JSON
    owner_id    TEXT REFERENCES users(id) ON DELETE CASCADE,
    run_after   INTEGER NOT NULL,
    created_at  INTEGER NOT NULL,
    started_at  INTEGER,
    finished_at INTEGER
);
CREATE INDEX jobs_queue ON jobs(status, run_after);

CREATE TABLE audit_log (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    at       INTEGER NOT NULL,
    actor_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    action   TEXT NOT NULL,
    target   TEXT,
    ip       TEXT,
    details  TEXT                          -- JSON
);
CREATE INDEX audit_at ON audit_log(at);
