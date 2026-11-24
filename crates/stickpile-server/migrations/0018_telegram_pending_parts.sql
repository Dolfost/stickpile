-- An export of a set under a new pack name (a rename, or publishing under
-- the name of another of the user's sets, which takes that set's packs
-- over) takes effect only once it has finished (docs/telegram.md, "Sets
-- larger than one pack"). Until then the set keeps its binding as it was
-- (`telegram_parts`, `sets.telegram_base`, `sets.telegram_pack`), and the
-- parts of the export under way are kept here, one row per part, as
-- `telegram_parts` keeps the bound ones: their names are claimed, and what
-- the export left on each pack is stored as soon as it is done with it, so
-- that a retried export continues under the same names without a second
-- pack for a part and without leaving one behind. When the export
-- finishes, the set's rows here replace its `telegram_parts` rows (and,
-- for a takeover, the other set's), and `base` becomes its
-- `sets.telegram_base`.
--
-- * `base`: the name the parts' names derive from (part 1's), the same for
--   every row of a set.
-- * `part`, `pack_name`, `title`, `mapping`, `thumbnail`, `stickers`: as in
--   `telegram_parts`; `mapping` NULL: registered for the export, no state
--   stored yet.
CREATE TABLE telegram_pending_parts (
    set_id      TEXT NOT NULL REFERENCES sets(id) ON DELETE CASCADE,
    base        TEXT NOT NULL COLLATE NOCASE,
    part        INTEGER NOT NULL CHECK (part >= 1),
    pack_name   TEXT NOT NULL UNIQUE COLLATE NOCASE,
    title       TEXT NOT NULL DEFAULT '',
    mapping     TEXT,
    thumbnail   TEXT,
    stickers    INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (set_id, part)
);

CREATE INDEX telegram_pending_parts_base ON telegram_pending_parts(base COLLATE NOCASE);
