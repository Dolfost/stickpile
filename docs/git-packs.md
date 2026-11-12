# Git packs

A git pack is a set bound to a remote git repository whose content is a set
directory in the format of [set-format.md](set-format.md). Anyone can publish
a pack as a repository, and others subscribe by URL. Git packs are
stickpile's federation story; there is no server-to-server protocol.

The implementation is `crates/stickpile-gitpack`, which uses libgit2 through
the `git2` crate. The server runs its calls as background jobs (`git_pull`,
`git_push`). All calls are blocking, so the server runs them on blocking
worker threads.

## Binding a repository

`POST /api/v1/sets/gitpack` with a `CreateGitPack` body:

```json
{
  "url": "https://codeberg.org/alice/cat-stickers.git",
  "branch": null,
  "subdir": "packs/cats",
  "mode": "read_only",
  "slug": "cats"
}
```

- `url` must pass the [URL policy](#url-policy).
- `branch`: `null` follows the remote's default branch (its `HEAD`).
- `subdir`: the directory inside the repository that holds `set.toml`. The
  default is the repository root.
- `mode`: `read_only` or `push` (see below).

The answer is `201` with a `GitPackCreated`: the new set, the webhook URL
(`<public_url>/api/v1/hooks/git/<set id>`), the webhook secret and the
`git_pull` job that clones the repository. The secret is shown only here.
The job clones into `<data_dir>/gitpacks/<set id>` and imports the directory
(`GET /api/v1/jobs/{id}` shows its progress and result).

- `slug`: defaults to the last component of `subdir`, else the repository
  name (`…/cat-stickpile.git` becomes `cat-stickpile`). A slug you already
  use is `409`. The slug never changes on later pulls.
- The set is named after the repository until the first import. From then
  on name, description, **visibility**, aspect ratio, recommended size, the
  cover (`cover`, and a cover image named by `cover_image`) and item
  metadata come from upstream's `set.toml`. A pack whose `set.toml` says
  `visibility = "public"` is public on your server too.
- `branch` must be a plain branch name (no options, revision syntax or
  whitespace). `subdir` must be a relative path inside the repository with
  no `..` and no hidden components (so never `.git`). Violations are `400`.
- With `git.enabled = false` every git endpoint answers `404`, and the
  scheduler does nothing.
- Edits made in stickpile before the first pull has finished are replaced by
  the repository content.

The set's `git` field (`GitBinding`) shows the binding: `last_commit` is the
imported or pushed commit, `last_pull_at` is the time of the last pull
attempt, and `last_error` is the error of the last failed pull or push
(cleared on success). Deleting the set unsubscribes; its checkout is removed
by the periodic cleanup.

Job failures: a URL that is not allowed, a repository over the size limit,
an unreadable or missing set directory and an invalid `set.toml` fail at
once. Network and credential errors are retried twice with backoff (10 s,
20 s).

### What a sync does

`stickpile_gitpack::sync(url, branch, dest, auth, max_bytes)`:

1. Clones into `dest` if it does not exist yet, otherwise fetches. A changed
   remote URL is followed.
2. Fetches only the tracked branch. Over https and ssh the fetch is shallow
   (depth 1), so the full history is never downloaded. libgit2's local
   transport (`file://`) cannot fetch shallow and falls back to a full fetch.
3. Checks the size. The sum of the blob sizes in the target tree must not
   exceed `max_bytes`, and this is checked before anything is written to the
   worktree. The download itself is aborted past 4 × `max_bytes`. On
   violation the checkout is deleted and the job fails with "repository
   exceeds size limit".
4. Hard-resets the worktree to the remote tip and removes untracked and
   ignored files, the equivalent of `git reset --hard && git clean -ffdx`.
   Local changes are always discarded, because the database is the source of
   truth and the checkout is only a transport.
5. Returns the new commit and whether it moved. If it did not move there is
   nothing to import.

Safety properties of the checkout:

- `core.symlinks = false`. A symlink committed upstream is checked out as a
  small regular file that contains the link target, so it can never point
  the importer outside the checkout. The importer also refuses symlinks on
  its own.
- libgit2 runs no hooks and no external filter drivers. Submodules and LFS
  objects are not fetched.
- A failed first clone removes its directory again.

## Read-only vs push mode

| | `read_only` | `push` |
|---|---|---|
| Web UI | Set is read-only, with an **Edit upstream** link | Editable like any set |
| Upstream changes | Pulled and imported | Pulled and imported |
| Local edits | Not possible | Exported, committed and pushed |
| Credentials | Only for private repositories | Must have push rights |

In read-only mode the API refuses every edit with `403` ("edit it
upstream"): set `PATCH`, reordering, uploads, `import?into=`, item `PATCH`
and `DELETE`, and moving items in or out. `Set.editable` is `false`. The
owner may still pull and delete (unsubscribe). Likes and usage work as for
any set.

**Edit upstream** links (`GitBinding.web_url`) come from
`web_url(url, branch, subdir)`:

| Host | Link |
|---|---|
| GitHub and unrecognised hosts | `https://host/owner/repo/tree/<branch or HEAD>/<subdir>` |
| GitLab (`gitlab.com`, `gitlab.*`, nested groups) | `https://host/group/sub/repo/-/tree/<branch or HEAD>/<subdir>` |
| Codeberg, `gitea.*`, `forgejo.*` | `https://host/owner/repo/src/branch/<branch>/<subdir>` (`…/src/<subdir>` without a branch) |

ssh and scp-like remotes map to `https://host/…`; the ssh port is dropped.
`file://` remotes get no link.

Every change of a push-mode set (anything that bumps its `version`: item
and set edits, uploads, deletions, reordering, imports into it, AI
suggestions) schedules a `git_push` job 5 seconds later. Further changes
within that time move the same job, so a burst of edits becomes one commit.
The job fetches upstream, exports the set into the checkout with
`write_dir`, and calls `commit_and_push`:

- `write_dir` rewrites `set.toml` and the media files and leaves other files
  such as `README.md` or `LICENSE` alone. Media files are named
  `NNN-<name>.<ext>` as in every export, and a cover image `cover.<ext>`, so
  the first push renames upstream files that follow another naming scheme.
  Removing the cover image in stickpile deletes the file upstream.
- `commit_and_push` stages everything, deletions included, and respects
  `.gitignore`. It commits as `git.author_name` / `git.author_email`, with
  the message `stickpile: update <set name> (version <n>)`, only if the
  tree changed. It then pushes the current branch as a normal fast-forward
  push.
- If someone pushed upstream between the fetch and the push, the push is
  rejected. The job then re-syncs (which drops the local commit),
  re-exports and tries once more. If that fails too, the job fails with the
  error, and it is recorded in the set's `git.last_error`. The edit is not
  lost: the next pull pushes it (see below).

**Conflicts.** The database is the source of truth. The server remembers
which version of the set matches the last imported or pushed commit
(`sets.git_synced_version`). When a pull finds local edits that are not
upstream yet (the set's version differs), it does **not** import. It pushes
the local state on top of the new upstream commit instead, so stickpile wins.
Upstream's concurrent change stays in the repository history, and
non-set files it added (such as a new `NOTES.md`) are kept. When there are
no pending local edits, a pull imports upstream as in read-only mode, and
nothing is pushed back.

Push mode also works with an empty repository. Create an empty repository on
the forge, bind it in push mode, and the first change creates the initial
commit on `main` (or the configured branch).

## Schedule and webhooks

Every git pack is re-pulled every `git.pull_interval_minutes` (default 60,
`0` turns scheduled pulls off). The scheduler runs every minute and queues a
pull for each pack whose last pull attempt is older than the interval and
that has no pull queued or running. `POST /api/v1/sets/{id}/pull` (the owner
only, read-only packs included) pulls immediately and returns the job. If a
pull is already queued, that job is returned instead of a second one. A pull
that finds the commit unchanged imports nothing. A webhook makes updates
near-instant:

```
POST <public_url>/api/v1/hooks/git/<set id>
```

The endpoint is unauthenticated. It accepts a request only if it carries the
set's webhook secret, in one of these forms:

- `X-Hub-Signature-256: sha256=<hex HMAC-SHA256 of the body>` (GitHub,
  Gitea, Forgejo), or the bare hex form that Gitea and Forgejo send in
  `X-Gitea-Signature` and `X-Forgejo-Signature`;
- `X-Gitlab-Token: <secret>` (GitLab's secret token);
- `X-Stickpile-Webhook-Secret: <secret>` (any forge that can send a custom
  header).

All are compared in constant time (`verify_hub_signature`, `verify_token`).
An empty secret never verifies. The request body (at most 2 MiB) is not
interpreted beyond the signature: any valid call enqueues a pull, or reuses
the one already queued.

| Answer | When |
|---|---|
| `202` + `Job` | Verified; pull queued |
| `401` | No secret or signature header |
| `403` | A header is present but does not verify |
| `404` | Unknown set, a set that is not a git pack, or git packs disabled |
| `429` | More than 60 calls per minute from the client IP (checked before the signature) |

**GitHub**: *Settings → Webhooks → Add webhook*. Payload URL = the webhook
URL, content type `application/json`, secret = the webhook secret, event
"Just the push event".

**Gitea / Forgejo / Codeberg**: *Settings → Webhooks → Add webhook → Gitea*
(or Forgejo). Target URL = the webhook URL, method POST, content type
`application/json`, secret = the webhook secret, trigger "Push events".

**GitLab**: *Settings → Webhooks*. URL = the webhook URL, *Secret token* =
the webhook secret (sent as `X-Gitlab-Token`), trigger "Push events".

## Version

`set.toml` carries `version`. Every change made in stickpile bumps it, and
upstream authors are expected to bump it when they publish. The value lets
the server and clients tell an upstream update from a local edit:

- A pull that changes anything sets the version to
  `max(stored + 1, upstream version)`. It always increases, so clients that
  cached the set see the number change and refresh. It equals the upstream
  version whenever the author bumped it past the stored one. (A new pack
  starts as an empty set with version 1, so its first pull gives
  `max(2, upstream version)`.)
- A pull that changes nothing keeps the version.
- In push mode the exported `set.toml` carries the server's version, so the
  upstream history shows each stickpile edit as a version bump.
- A pulled `version` lower than the one stored is still imported (upstream
  content wins, the version still increases). The difference is logged.

## URL policy

Remote URLs are user input. `validate_url(url, allowed_schemes)` enforces
the server's `git.allowed_schemes` (default `["https"]`; choose from
`https`, `http`, `ssh`, `file`):

- `ssh` covers both `ssh://[user@]host[:port]/path` and scp-like
  `[user@]host:path`.
- `file://` is meant for tests. It must not name a host.
- These are always rejected:
  - credentials in the URL (`https://user:token@…`, `https://token@…`, or a
    password in an ssh URL). Configure a token or a key instead;
  - transport helpers (`ext::…`, `fd::…`, any `<transport>::<address>`);
  - local paths without `file://` (`/srv/repo`, `./repo`, `C:\repo`);
  - anything that looks like a command-line option: a leading `-` in the
    URL, host, user or scp path;
  - whitespace, control characters, and URLs longer than 2048 bytes.

Error messages redact any `user:password@` part of a URL.

## Credentials

Set in the `[git]` section of the server config:

| Key | Used for |
|---|---|
| `ssh_key_file` | ssh remotes: an unencrypted private key file, offered as `Cred::ssh_key` for the user in the URL (default `git`). The host must be in `~/.ssh/known_hosts` of the service user, because libgit2 refuses unknown host keys. |
| `https_token_file` | https remotes: the token is sent as the password. |
| `https_username` | Username sent with the token (default `x-access-token`, which GitHub, GitLab, Gitea and Forgejo accept for personal access tokens). |
| `author_name`, `author_email` | Author and committer of push-mode commits. |
| `max_repo_mb` | `max_bytes` for `sync`. |

Credentials are offered at most a few times per connection, so rejected
credentials end the operation with "authentication failed" instead of
looping.

## Build and runtime requirements

- `git2` links **system libgit2 1.9** through pkg-config when it is
  available: the dev shell provides it, and `LIBGIT2_NO_VENDOR=1` makes
  that mandatory. Otherwise the crate builds the vendored copy. libgit2 uses
  OpenSSL for https. The CA bundle comes from the system: on NixOS
  `/etc/ssl/certs`, in the container set `SSL_CERT_FILE` to the `cacert`
  bundle.
- No `git` binary is needed at runtime.
- `stickpile-server serve` sets libgit2's network timeouts at startup
  (`set_network_timeouts`): 30 s to connect, 120 s per read or write.

## Testing

`crates/stickpile-gitpack/tests` covers the library against local `file://`
repositories. `crates/stickpile-server/tests/integrations_git.rs` covers the
server end to end: clone and import, webhooks (secret, GitLab token, HMAC,
wrong or missing credentials, unknown set, rate limit), re-pull, read-only
enforcement on every edit path, push mode against a bare repository
(debounced commits, conflicts, an empty repository, a remote that refuses
the push), the size limit, the URL policy, the scheduler and checkout
cleanup. No test needs the network.
