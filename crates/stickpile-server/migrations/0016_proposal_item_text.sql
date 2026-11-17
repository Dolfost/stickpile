-- The text of a proposed sticker (the `text` in the proposal's `meta`, as
-- for uploads), kept until the proposal is accepted, with its source as
-- items.text_source has it: '' and 'ai' is "not set" (the server reads
-- the picture once it is a sticker); a text the proposer gave is 'user'
-- (an empty one too), an unapproved suggestion 'ai'. Proposals made
-- before have none: '' and 'ai'.
ALTER TABLE proposal_items ADD COLUMN text TEXT NOT NULL DEFAULT '';
ALTER TABLE proposal_items ADD COLUMN text_source TEXT NOT NULL DEFAULT 'ai' CHECK (text_source IN ('user', 'ai'));
