-- Problems a set's owner marked as "not a problem" (docs/web.md, "Problems").
-- problem_id is the problem's id: its kind plus a hash of what makes it a
-- problem (the affected items with their media, emojis, ...). A dismissal
-- therefore survives restarts and stops applying as soon as one of those
-- changes: the changed problem gets a new id and shows up again. Rows that no
-- longer match anything are harmless; each set keeps at most a few hundred
-- (the oldest are dropped when a new one is added).
CREATE TABLE problem_dismissals (
    set_id       TEXT NOT NULL REFERENCES sets(id) ON DELETE CASCADE,
    problem_id   TEXT NOT NULL,
    kind         TEXT NOT NULL,
    dismissed_by TEXT REFERENCES users(id) ON DELETE SET NULL,
    created_at   INTEGER NOT NULL,
    PRIMARY KEY (set_id, problem_id)
);
