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

The server clones into `<data_dir>/gitpacks/<set id>`, imports the
directory, and returns the set together with a webhook URL and secret. The
secret is shown only once.

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

**Edit upstream** links come from `web_url(url, branch, subdir)`:

| Host | Link |
|---|---|
| GitHub and unrecognised hosts | `https://host/owner/repo/tree/<branch or HEAD>/<subdir>` |
| GitLab (`gitlab.com`, `gitlab.*`, nested groups) | `https://host/group/sub/repo/-/tree/<branch or HEAD>/<subdir>` |
| Codeberg, `gitea.*`, `forgejo.*` | `https://host/owner/repo/src/branch/<branch>/<subdir>` (`…/src/<subdir>` without a branch) |

ssh and scp-like remotes map to `https://host/…`; the ssh port is dropped.
`file://` remotes get no link.

In push mode the server exports the set into the checkout with `write_dir`,
which rewrites `set.toml` and the media files and leaves other files such as
`README.md` or `LICENSE` alone. It then calls `commit_and_push`, which:

- stages everything, deletions included, and respects `.gitignore`;
- commits as the configured author only if the tree changed;
- pushes the current branch as a normal fast-forward push. If someone pushed
  upstream in the meantime, the push is rejected. The job then re-syncs
  (which drops the local commit), re-imports, re-exports and tries again. A
  commit left unpushed by an earlier failure is pushed by the next run.

Push mode also works with an empty repository. Create an empty repository on
the forge, bind it in push mode, and the first export creates the initial
commit on `main` (or the configured branch).

## Schedule and webhooks

Every git pack is re-pulled every `git.pull_interval_minutes` (default 60).
`POST /api/v1/sets/{id}/pull` pulls immediately. A webhook makes updates
near-instant:

```
POST <public_url>/api/v1/hooks/git/<set id>
```

The endpoint is unauthenticated. It accepts a request only if it carries the
set's webhook secret, in one of these forms:

- `X-Hub-Signature-256: sha256=<hex HMAC-SHA256 of the body>` (GitHub,
  Gitea, Forgejo);
- `X-Stickpile-Webhook-Secret: <secret>` (any forge that can send a custom
  header).

Both are compared in constant time (`verify_hub_signature`, `verify_token`).
`verify_hub_signature` also accepts the bare hex form that Gitea and Forgejo
send in `X-Gitea-Signature` and `X-Forgejo-Signature`. An empty secret never
verifies. The request body is not interpreted beyond the signature: any valid
call enqueues a pull.

**GitHub**: *Settings → Webhooks → Add webhook*. Payload URL = the webhook
URL, content type `application/json`, secret = the webhook secret, event
"Just the push event".

**Gitea / Forgejo / Codeberg**: *Settings → Webhooks → Add webhook → Gitea*
(or Forgejo). Target URL = the webhook URL, method POST, content type
`application/json`, secret = the webhook secret, trigger "Push events".

**GitLab**: *Settings → Webhooks*. URL = the webhook URL, add a custom header
`X-Stickpile-Webhook-Secret: <secret>`, trigger "Push events".

## Version

`set.toml` carries `version`. Every change made in stickpile bumps it, and
upstream authors are expected to bump it when they publish. The value lets
the server and clients tell an upstream update from a local edit:

- After a pull, the imported set carries the upstream `version`. Clients that
  cached the set see the number change and refresh.
- In push mode the exported `set.toml` carries the server's version, so the
  upstream history shows each stickpile edit as a version bump.
- A pulled `version` lower than the one stored is still imported (upstream
  wins). The difference is logged.

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
- Network timeouts can be set once at startup with
  `stickpile_gitpack::set_network_timeouts`.
