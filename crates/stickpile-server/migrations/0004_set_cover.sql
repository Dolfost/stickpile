-- The item a set shows as its cover: in set lists, and as the Telegram
-- pack's thumbnail. NULL means the first item. Not a foreign key (SQLite
-- cannot add one to an existing table): readers only use it while the item
-- is still in the set, so a deleted or moved item falls back to the first.
ALTER TABLE sets ADD COLUMN cover_item_id TEXT;

-- Key (cover item id + media sha256) of the thumbnail the last Telegram
-- export set on the bound pack; NULL when stickpile has set none. Lets the
-- export upload a thumbnail only when the cover changed, and drop it when
-- the cover is unset without touching one made outside stickpile.
ALTER TABLE sets ADD COLUMN telegram_thumbnail TEXT;
