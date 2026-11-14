-- The web UI language a user chose (docs/languages.md): a code such as
-- `uk`, or empty for "automatic" (the browser's Accept-Language). Clients
-- that follow the account read it from GET /api/v1/me.
ALTER TABLE users ADD COLUMN language TEXT NOT NULL DEFAULT '';
