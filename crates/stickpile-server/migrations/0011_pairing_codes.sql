-- One-time pairing codes (docs/security.md#pairing): a signed-in user
-- creates one (Settings, "Add a device", or POST /api/v1/pairings) and a new
-- device exchanges it for an API token without a password
-- (POST /api/v1/auth/pair). Only the sha256 of the normalised code is kept.
-- A code works once (`used_at`) and until `expires_at`; rows are deleted an
-- hour after they expire.
CREATE TABLE pairing_codes (
    id         TEXT PRIMARY KEY,
    code_hash  TEXT NOT NULL UNIQUE,       -- sha256 hex of the canonical code
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    used_at    INTEGER
);
CREATE INDEX pairing_codes_user ON pairing_codes(user_id);
CREATE INDEX pairing_codes_expires ON pairing_codes(expires_at);
