-- Colour theme of the web UI per user: 'system' follows the device
-- (prefers-color-scheme), 'light' and 'dark' are fixed. The server renders
-- <html data-theme> from it, so pages open in the right colours.
ALTER TABLE users ADD COLUMN theme TEXT NOT NULL DEFAULT 'system'
    CHECK (theme IN ('system', 'light', 'dark'));
