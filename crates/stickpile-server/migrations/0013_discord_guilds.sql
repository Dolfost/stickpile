-- Who may publish to each Discord server, per kind (`sticker` or `emoji`).
-- One bot serves every user and can edit and delete every sticker and emoji
-- it ever created there, so the server records which stickpile user first
-- published to a server and kind (or, as an admin, adopted its bot-made
-- expressions on import) and refuses publishes by anyone else
-- (docs/discord.md, "Server ownership"), as `telegram_packs` (0003) does for
-- Telegram packs. The sets' mappings live in `set_publications.state`
-- (0012, platform `discord`).
CREATE TABLE discord_guilds (
    guild_id    TEXT NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('sticker', 'emoji')),
    user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (guild_id, kind)
);
