-- Git packs in push mode: the set version that matches git_last_commit
-- (after a pull-import or a successful push). version != git_synced_version
-- means local edits that are not upstream yet.
ALTER TABLE sets ADD COLUMN git_synced_version INTEGER;
