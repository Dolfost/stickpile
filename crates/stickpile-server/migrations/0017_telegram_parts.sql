-- A set larger than one Telegram pack (120 stickers) is published as
-- several packs, its parts (docs/telegram.md, "Sets larger than one pack").
-- One row per part of a set; part 1 is the pack a set was always published
-- as.
--
-- * `pack_name`: the part's pack, `<name>_by_<bot>` for part 1 and
--   `<name>_<k>_by_<bot>` for part k. Telegram compares pack names
--   case-insensitively, and so does this table; a pack is a part of at most
--   one set.
-- * `title`: the title the last export gave the pack ('' before one did).
-- * `mapping`, `thumbnail`: what the last export left on the pack, as
--   `sets.telegram_mapping` (JSON item id -> [media hash, file_unique_id])
--   and `sets.telegram_thumbnail` (the key of the thumbnail stickpile set)
--   held for the one pack before. `mapping` NULL: the part's name is
--   claimed for its export but no export stored a state yet; the next
--   export fills the part or deletes its pack, so none is left behind.
-- * `stickers`: how many stickers the mapping has.
CREATE TABLE telegram_parts (
    set_id      TEXT NOT NULL REFERENCES sets(id) ON DELETE CASCADE,
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

-- The name every part's name derives from: part 1's, the name a set was
-- published under (the default from the slug, or a name chosen at an
-- export). It stays when part 1's own pack is deleted (emptied while other
-- parts still hold stickers); `sets.telegram_pack` then names the lowest
-- part that still exists, so older clients keep a working link.
ALTER TABLE sets ADD COLUMN telegram_base TEXT;
CREATE UNIQUE INDEX sets_telegram_base ON sets(telegram_base COLLATE NOCASE) WHERE telegram_base IS NOT NULL;

-- When stickpile deleted the pack on Telegram (a part left without
-- stickers). Telegram does not say whether a deleted pack's name can be
-- created again, so such a name is never given to a part again. The claim
-- stays (0003).
ALTER TABLE telegram_packs ADD COLUMN deleted_at INTEGER;

-- Every existing binding becomes part 1 of its set, unchanged: the same
-- pack, mapping and thumbnail key.
INSERT INTO telegram_parts (set_id, part, pack_name, title, mapping, thumbnail, stickers, created_at, updated_at)
SELECT id, 1, telegram_pack, '', telegram_mapping, telegram_thumbnail,
       (SELECT COUNT(*) FROM json_each(CASE WHEN json_valid(telegram_mapping) THEN telegram_mapping ELSE '{}' END)),
       updated_at, updated_at
FROM sets
WHERE telegram_pack IS NOT NULL;

UPDATE sets SET telegram_base = telegram_pack WHERE telegram_pack IS NOT NULL;

-- The mapping and thumbnail key live with the part now; these two columns
-- are no longer read or written.
UPDATE sets SET telegram_mapping = NULL, telegram_thumbnail = NULL;
