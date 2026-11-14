# Collaboration

Users share work on packs in two ways:

- **Contributors.** A pack's owner adds other users to it and chooses what
  each may do. Contributors work in the pack directly.
- **Proposals.** Anyone who can see a pack may propose new stickers or
  changes to it. The owner, or a contributor allowed to review, accepts or
  declines each proposal.

The web UI's **Collaboration** tab lists both sides: the proposals waiting
for you (incoming), the ones you sent (your proposals), the packs you
contribute to, and the decided proposals. The nav shows how many proposals
wait for you.

The code is `crates/stickpile-server/src/authz.rs` (who may do what),
`service/members.rs` and `service/proposals.rs`. The tests are in
`tests/collaboration.rs` (API) and `tests/web_collab.rs` (web forms).

## Contributors

The owner adds a contributor by username on the pack page, under
**Contributors**, and ticks what they may do:

| Permission | Lets them |
|---|---|
| Add stickers | upload stickers to the pack, or move one in from another pack |
| Edit their stickers | edit and remove the stickers they added |
| Edit all stickers | edit everyone's stickers: name, keywords, emojis, tags, vibe |
| Remove any sticker | remove everyone's stickers |
| Edit the pack | the pack's name, description, cover and sticker order |
| Review proposals | accept or decline proposals for the pack |

New contributors get "Add stickers" and "Edit their stickers". With nothing
ticked, a contributor can only view the pack. That is a way to share a
private pack read-only.

Whatever the permissions, some things stay with the owner: the pack's
settings (slug, visibility, recommended format), the git binding and the
Telegram export, its contributors, replacing it from a set directory, and
deleting it. Nobody changes a read-only git pack, contributors included.

A contributor sees the pack like its owner does, also when it is private:
on its page, in browse (`GET /api/v1/sets?scope=all`, and
`scope=contributing` lists only these), in search, and in their client's
offline cache. The desktop popup's Packs tab lists contributed packs with
the other users' packs. When someone is added, the pack's stickers are
synced to their cache. When they are removed, or leave, the stickers drop
out of it again.

Each sticker records who added it (`Item.added_by_username`). Stickers that
existed before collaboration count as the owner's.

The pack page shows a contributor their permissions and a **Leave** button.
Packs whose settings a contributor may edit have an **Edit pack** page: name,
description and cover.

## Proposals

On someone else's pack, **Propose…** offers three forms:

- **New stickers**: one or more files and a message.
- **Name, description, cover**: new values, or one of the pack's stickers
  as its cover.
- **A cover image**: a still PNG, JPEG or WebP.

On someone else's sticker, **Suggest changes…** proposes a new name,
keywords, emojis, tags or vibe.

Fields with the current value are left out. A proposal that changes
nothing is refused. A proposal shows what it would change: the stickers
with thumbnails, or a table of the current and the proposed values.

Who may propose: anyone who can read the pack (public, unlisted, or one
they contribute to) and is not its owner. Contributors propose what their
permissions do not let them do themselves. Read-only git packs take no
proposals. A user may have at most 20 open proposals per pack, a proposal
at most 50 stickers, and a message or review note at most 1000 characters.

What happens next:

- **Accept** (the owner, or a contributor who reviews): the change is made
  at once, with the review permission alone. A reviewer need not be allowed
  to make the change themself. Accepted stickers count as added by their
  proposer, so "Edit their stickers" covers them. All the stickers of a
  proposal are added, or none.
- **Decline** (the same people), with an optional note for the proposer.

Nobody decides their own proposal: a reviewer's proposals go to the owner
or another reviewer, so the review permission never lets a contributor make
changes their other permissions do not allow.
- **Withdraw** (the proposer, while it is open).

A decision is final. A change that cannot be made any more, for example
because the sticker was removed meanwhile, is refused and the proposal
stays open, so it can be declined. A sticker that was removed or moved to
another pack is gone for a proposal about it.

The proposer keeps seeing their proposals, also if the pack becomes
private later. Deleting a pack deletes its proposals.

### Files of proposals

Proposed stickers and cover images are stored when they are proposed, so
reviewers see thumbnails, but they belong to no pack until accepted. The
cleanup of unused files keeps them while the proposal is open and for a
day after the decision. After that, a declined or withdrawn proposal keeps
its text and the names of its stickers, without the files.

Backups keep the files of open proposals in `proposals/`, next to `sets/`,
and a restore puts them back (see [backup.md](backup.md)).

## API

The endpoints are listed in [api.md](api.md#collaboration):

- `GET`, `PUT` and `DELETE /api/v1/sets/{id}/members[/{username}]` for
  contributors.
- `POST /api/v1/sets/{id}/proposals`, `.../proposals/items` and
  `.../proposals/cover`, and `POST /api/v1/items/{id}/proposals`, to
  propose.
- `GET /api/v1/proposals?box=incoming|outgoing` to list proposals, and
  `POST /api/v1/proposals/{id}/accept|decline|withdraw` to decide them.

`Set.role` (`owner`, `contributor`, `viewer`) and `Set.permissions` tell a
client what the caller may do in a set; `Set.editable` still means "may
change its settings" (the owner).
