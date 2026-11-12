-- A set's own cover image, uploaded apart from its items: a row of `media`
-- with just the sanitised original and the grid thumbnail (no other
-- variants, never enriched) that no item has to reference. It wins over
-- cover_item_id: lists show it and Telegram gets it as the pack thumbnail.
-- NULL: none. The orphan-media GC keeps media named here, so replacing or
-- removing the image, or deleting the set, leaves the old media to the GC
-- like a deleted item's.
ALTER TABLE sets ADD COLUMN cover_media_id TEXT REFERENCES media(id);
CREATE INDEX sets_cover_media ON sets(cover_media_id) WHERE cover_media_id IS NOT NULL;
