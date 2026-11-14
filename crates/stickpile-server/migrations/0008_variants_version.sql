-- Version of the renditions made for a media row (stickpile_media::
-- variants_version, per kind). On start the server queues a Variants job for
-- every row whose renditions are older than the current version, so a
-- change of encoding (version 2: lossless Telegram stickers for stills)
-- reaches stored items without a manual step. Existing rows were made by
-- version 1.
ALTER TABLE media ADD COLUMN variants_version INTEGER NOT NULL DEFAULT 1;
