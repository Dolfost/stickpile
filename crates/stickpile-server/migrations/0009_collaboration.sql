-- Collaboration: contributors with permissions, and proposals to other
-- users' sets (docs/collaboration.md).

-- Who added an item, for the "edit and remove their own stickers"
-- permission. Items made before this migration count as their set owner's.
ALTER TABLE items ADD COLUMN added_by TEXT REFERENCES users(id) ON DELETE SET NULL;
UPDATE items SET added_by = (SELECT owner_id FROM sets WHERE sets.id = items.set_id);

-- Contributors of a set and what each may do (stickpile_api::Permissions).
CREATE TABLE set_members (
    set_id         TEXT NOT NULL REFERENCES sets(id) ON DELETE CASCADE,
    user_id        TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    can_add        INTEGER NOT NULL DEFAULT 1,
    can_edit_own   INTEGER NOT NULL DEFAULT 1,
    can_edit_all   INTEGER NOT NULL DEFAULT 0,
    can_remove_all INTEGER NOT NULL DEFAULT 0,
    can_edit_pack  INTEGER NOT NULL DEFAULT 0,
    can_review     INTEGER NOT NULL DEFAULT 0,
    added_by       TEXT REFERENCES users(id) ON DELETE SET NULL,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    PRIMARY KEY (set_id, user_id)
);
CREATE INDEX set_members_user ON set_members(user_id);

-- Removals for one user's client cache: items a contributor could see and
-- no longer can (removed from a set they contribute to, or they stopped
-- contributing). The other removals are in item_tombstones.
CREATE TABLE member_tombstones (
    item_id    TEXT NOT NULL,
    user_id    TEXT NOT NULL,
    removed_at INTEGER NOT NULL
);
CREATE INDEX member_tombstones_user ON member_tombstones(user_id, removed_at);
CREATE INDEX member_tombstones_at ON member_tombstones(removed_at);

-- A request to change someone else's set.
CREATE TABLE proposals (
    id             TEXT PRIMARY KEY,
    set_id         TEXT NOT NULL REFERENCES sets(id) ON DELETE CASCADE,
    item_id        TEXT REFERENCES items(id) ON DELETE SET NULL,  -- edit_item
    proposer_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind           TEXT NOT NULL CHECK (kind IN ('add_items', 'edit_item', 'edit_set')),
    status         TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'accepted', 'declined', 'withdrawn')),
    message        TEXT NOT NULL DEFAULT '',
    changes        TEXT NOT NULL DEFAULT '{}',  -- JSON ProposedChanges
    before         TEXT NOT NULL DEFAULT '{}',  -- JSON ProposedChanges: the values at proposal time
    -- edit_set: a proposed cover image. Media of open proposals is kept;
    -- after the decision it may be cleaned up like any unused media.
    cover_media_id TEXT REFERENCES media(id) ON DELETE SET NULL,
    reviewer_id    TEXT REFERENCES users(id) ON DELETE SET NULL,
    review_note    TEXT NOT NULL DEFAULT '',
    created_at     INTEGER NOT NULL,
    decided_at     INTEGER
);
CREATE INDEX proposals_set ON proposals(set_id, status);
CREATE INDEX proposals_proposer ON proposals(proposer_id, status);

-- The stickers of an add_items proposal.
CREATE TABLE proposal_items (
    proposal_id TEXT NOT NULL REFERENCES proposals(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    media_id    TEXT REFERENCES media(id) ON DELETE SET NULL,  -- kept while the proposal is open
    kind        TEXT NOT NULL,
    format      TEXT NOT NULL,
    width       INTEGER NOT NULL,
    height      INTEGER NOT NULL,
    filename    TEXT NOT NULL,
    name        TEXT NOT NULL,
    keywords    TEXT NOT NULL DEFAULT '[]',
    emojis      TEXT NOT NULL DEFAULT '[]',
    vibe        TEXT,
    tags        TEXT NOT NULL DEFAULT '[]',
    item_id     TEXT,                           -- the item it became, once accepted
    PRIMARY KEY (proposal_id, position)
);
CREATE INDEX proposal_items_media ON proposal_items(media_id);
