-- Where each set is published, one row per set and platform
-- (`service::publish`, docs/web.md "Export and publish"). The platform id
-- is the registry's (`telegram`; Discord and Signal later).
--
-- * `target`, `url`, `state`: what the set is published as, its link and
--   platform-specific JSON (bindings, mappings), for platforms that keep
--   them here. Telegram keeps its binding in `sets.telegram_*` (0003) and
--   leaves these empty.
-- * `published_at`, `last_result`: the last successful publish and its job
--   result; `last_status`, `last_error`, `last_finished_at`, `last_job_id`:
--   the last publish that finished, successful or not. Written when a
--   publish job finishes, so they outlive the job rows (deleted after 30
--   days).
CREATE TABLE set_publications (
    set_id           TEXT NOT NULL REFERENCES sets(id) ON DELETE CASCADE,
    platform         TEXT NOT NULL,
    target           TEXT,
    url              TEXT,
    state            TEXT,
    published_at     INTEGER,
    last_result      TEXT,
    last_status      TEXT,
    last_error       TEXT,
    last_finished_at INTEGER,
    last_job_id      TEXT,
    PRIMARY KEY (set_id, platform)
);

-- Sets published to Telegram before this table existed: the newest
-- finished export job, if it is still there.
INSERT INTO set_publications (set_id, platform, published_at, last_result, last_status, last_finished_at, last_job_id)
SELECT s.id, 'telegram', j.finished_at, j.result, j.status, j.finished_at, j.id
FROM sets s
LEFT JOIN jobs j ON j.id = (
    SELECT j2.id FROM jobs j2
    WHERE j2.kind = 'telegram_export' AND j2.subject = s.id AND j2.status = 'done'
    ORDER BY j2.finished_at DESC, j2.id DESC
    LIMIT 1
)
WHERE s.telegram_pack IS NOT NULL;
