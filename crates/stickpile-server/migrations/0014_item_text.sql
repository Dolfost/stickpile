-- The words written in a picture (docs/api.md, "Items"; docs/ai.md, "OCR
-- and the text field").
--
-- items.text: one string, line breaks kept, at most 2000 characters.
-- items.text_source: who wrote it. An empty text with source 'ai' means
-- "not set": the computer fills it from the media's OCR result. Any edit by
-- a person, clearing included, sets 'user', and the computer never writes
-- the text again. Existing items have no text yet: '' and 'ai'.
ALTER TABLE items ADD COLUMN text TEXT NOT NULL DEFAULT '';
ALTER TABLE items ADD COLUMN text_source TEXT NOT NULL DEFAULT 'ai' CHECK (text_source IN ('user', 'ai'));

-- The OCR result belongs to the media, so the same file in two sets is read
-- once, like embedding / embedding_model. ocr_text NULL: never read; '':
-- read, no confident text. ocr_model: the reader that read it; another
-- reader's result counts as not read.
ALTER TABLE media ADD COLUMN ocr_text TEXT;
ALTER TABLE media ADD COLUMN ocr_model TEXT;
