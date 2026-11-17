-- Rechecks (docs/ai.md, "Recheck"; docs/api.md, "Rechecks"): batches that
-- recompute metadata of many pictures in the background, one `recheck` job
-- per picture (payload {recheck_id, media_id}).
--
-- scope 'user': the items of the sets owner_id owns; 'server': every item.
-- requested_by: who started it; NULL: the server itself (after a model
-- change, or for pictures its text reader never read).
-- parts: JSON list of RecheckPart names, sorted; mode 'missing' or 'refresh'.
-- total, done, failed, changed count pictures; each job adds to done or
-- failed (and changed) when it ends, and the batch is over when
-- done + failed = total.
CREATE TABLE rechecks (
    id           TEXT PRIMARY KEY,
    scope        TEXT NOT NULL CHECK (scope IN ('user', 'server')),
    requested_by TEXT REFERENCES users(id) ON DELETE CASCADE,
    owner_id     TEXT REFERENCES users(id) ON DELETE CASCADE,
    set_id       TEXT REFERENCES sets(id) ON DELETE CASCADE,
    parts        TEXT NOT NULL,
    mode         TEXT NOT NULL CHECK (mode IN ('missing', 'refresh')),
    state        TEXT NOT NULL DEFAULT 'running' CHECK (state IN ('running', 'done', 'cancelled', 'failed')),
    total        INTEGER NOT NULL DEFAULT 0,
    done         INTEGER NOT NULL DEFAULT 0,
    failed       INTEGER NOT NULL DEFAULT 0,
    changed      INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL,
    finished_at  INTEGER,
    CHECK ((scope = 'user') = (owner_id IS NOT NULL))
);
CREATE INDEX rechecks_owner ON rechecks(owner_id, created_at);
CREATE INDEX rechecks_state ON rechecks(state, scope);

-- The pictures whose recheck failed. media_id is kept after the media is
-- gone; item_id is one of the owner's items showing it (user scope).
CREATE TABLE recheck_failures (
    recheck_id TEXT NOT NULL REFERENCES rechecks(id) ON DELETE CASCADE,
    media_id   TEXT NOT NULL,
    item_id    TEXT,
    error      TEXT NOT NULL,
    at         INTEGER NOT NULL
);
CREATE INDEX recheck_failures_batch ON recheck_failures(recheck_id);

-- Queue order: higher priority first, then by run_after and age. Uploads,
-- imports and integrations keep 0; recheck jobs are negative, so
-- interactive work is never queued behind a large recheck.
ALTER TABLE jobs ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;
CREATE INDEX jobs_claim ON jobs(status, priority DESC, run_after, created_at, id);
