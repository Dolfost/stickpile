-- Who may publish to each Telegram pack. One bot serves every user and can
-- edit every pack it ever created, so the server records which stickpile
-- user created (or, as an admin, adopted) each pack and refuses exports and
-- import bindings by anyone else (docs/telegram.md, "Pack ownership").
-- Telegram compares pack names case-insensitively, and so does this table.
CREATE TABLE telegram_packs (
    name        TEXT PRIMARY KEY COLLATE NOCASE,
    user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL
);

-- Existing bindings: the owner of the earliest set bound to a pack keeps it.
INSERT OR IGNORE INTO telegram_packs (name, user_id, created_at)
SELECT telegram_pack, owner_id, created_at FROM sets
WHERE telegram_pack IS NOT NULL
ORDER BY created_at, id;

-- A pack follows exactly one set: later duplicates lose their binding.
UPDATE sets SET telegram_pack = NULL, telegram_mapping = NULL
WHERE telegram_pack IS NOT NULL
  AND id <> (
    SELECT s2.id FROM sets s2
    WHERE s2.telegram_pack = sets.telegram_pack COLLATE NOCASE
    ORDER BY s2.created_at, s2.id
    LIMIT 1
  );

CREATE UNIQUE INDEX sets_telegram_pack ON sets(telegram_pack COLLATE NOCASE) WHERE telegram_pack IS NOT NULL;
