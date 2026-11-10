# Security

stickpile is self-hosted and usually reached over a Tailscale network, but the
server is built to be safe when exposed to the internet. This page describes
the threat model and how each requirement of the brief (section 9) is met.
Code references are to `crates/stickpile-server/src/`.

## Threat model

| Attacker | Can | Must not be able to |
|---|---|---|
| Anonymous internet client | reach the TCP listener | read or change anything except `healthz`, the OpenAPI document, login and registration (per policy); guess passwords quickly |
| Another user of the same server | use the API with their own account | see others' private sets, list or search others' unlisted sets, edit anything they do not own, learn what others uploaded (no cross-user near-duplicate hints) |
| Malicious upload | be sent as any byte sequence with any name | execute, be served as HTML/script, smuggle metadata or polyglot payloads, exhaust memory with huge images or archives, escape the data directory |
| Website visited by a tailnet or logged-in user | make the browser send cross-site requests | act with the user's session cookie or tailnet identity |
| Other local user on the host | connect to loopback ports | impersonate a tailnet user |

Out of scope: a compromised host or service account, a malicious admin, and
the Tailscale control plane itself. Running the container as a non-root user
with a read-only root filesystem is part of the deployment
([deployment.md](deployment.md)).

## How the requirements are met

### Sessions, tokens and passwords

- **Session cookie** `stickpile_session`: 256 random bits, `HttpOnly`,
  `SameSite=Lax`, `Path=/`, `Secure` unless `server.secure_cookies = false`
  (plain-http development only). The database stores only the sha256 of the
  cookie. Sessions expire after `auth.session_ttl_days`; the expiry slides
  with use (`last_seen_at` and the expiry are refreshed at most every 10
  minutes and the cookie is re-issued). Expired sessions are purged
  periodically. Each session has its own CSRF token (`auth/session.rs`).
- **API tokens** `spk_<52 base32>`: 256 random bits, stored as sha256, shown
  once, optional expiry, revocable by the owner; `last_used_at` is recorded
  (at most once a minute). Revoked or expired tokens and tokens of disabled
  users are rejected (`auth/tokens.rs`).
- **Passwords**: argon2id with the `argon2` crate defaults (19 MiB, t = 2,
  p = 1), hashed and verified on the blocking pool. Unknown users and users
  without a password are verified against a dummy hash so timing does not
  reveal whether an account exists. Unknown user, wrong password and disabled
  account give the same error (`auth/password.rs`, `auth/login.rs`).
- **TOTP** (optional): RFC 6238, SHA-1, 6 digits, 30 s, plus or minus one
  step, compared in constant time. The last accepted step is stored, so a code
  cannot be replayed. Enrolment is pending until confirmed with a valid code;
  an admin can reset TOTP for a lost device (`auth/totp.rs`).
- Changing your own password revokes your other web sessions but keeps your
  API tokens, so the CLI and desktop clients stay logged in (revoke tokens
  explicitly if a device may be compromised). An admin password reset
  assumes the old credentials are compromised and revokes all web sessions
  **and all API tokens** of the user (audit detail `tokens_revoked`).
  Disabling an account revokes its sessions; disabled users cannot
  authenticate by any method.

### Authorization

All rules live in `authz.rs` and implement the visibility table of
[plan.md](plan.md#visibility-rules):

| Caller | own set | other's private | other's unlisted | other's public |
|---|---|---|---|---|
| read set / item / file by id | yes | no (404) | yes | yes |
| listed in browse | yes | no | no | yes |
| search, client cache | yes | no | no | yes |
| edit | yes, unless read-only git pack | no (404) | no (403) | no (403) |
| like, report usage | yes | no (404) | yes | yes |

- Every service function that takes a caller loads the object through
  `authz::readable_*` / `editable_*` / `owned_set`; content the caller cannot
  read is a 404, never a 403, so IDs cannot be probed.
- Search results and the client-cache feed are filtered by the same rule in
  SQL and checked again before they are returned.
- Near-duplicate hints and "already in your library" only consider the
  caller's own items, so shared media storage is never observable.
- Admin routes need the `admin` role (403 otherwise). Admins get no implicit
  access to other users' content. The last active admin cannot be demoted,
  disabled or deleted.
- IDs in URLs are 128 random bits; malformed IDs are rejected before any query.

### Uploads

- Size limits: per file `limits.max_upload_mb` (checked while streaming to a
  temp file), per request `limits.max_request_mb`, decoded pixels
  `limits.max_pixels`, animation frames and video length (`limits.*`). Tar
  archives are bounded by the request limit.
- The format comes from magic bytes, never from the filename or the client's
  content type. Only PNG, JPEG, WebP, GIF, MP4 and WebM are accepted.
- Every image is fully decoded under the limits. PNG and GIF are re-encoded
  losslessly; JPEG and WebP are rebuilt from whitelisted segments/chunks;
  videos are remuxed. Metadata (EXIF, comments, XMP), trailing data and
  polyglot payloads do not survive.
- ffmpeg/ffprobe run only for video and animated variants, with fixed
  arguments on server-generated paths, `-protocol_whitelist file` and a
  timeout. No user input reaches a command line. Details: [media.md](media.md).
- Files are served with the stored MIME type, `X-Content-Type-Options:
  nosniff`, `Content-Disposition: inline` with a sanitised filename, and the
  global CSP, so an upload can never run as a page.

### No directory traversal

- Media files live at `media/<sha[0..2]>/<sha>/<variant>.<ext>`: every path
  component is a server-computed hash or a fixed variant name. Stored relative
  paths are re-checked (`files::resolve`) before use.
- Set slugs are `[a-z0-9-]`; set names, item names and upload filenames are
  data, never paths. Export filenames are generated (`001-<slug>.<ext>`).
- Imported directories and tars go through `stickpile_setfmt`, which rejects
  absolute paths, `..`, links and device entries (`safe_filename`).
- Writes are atomic (temp file in the same directory + rename).

### CSRF, CSP and headers

- Cookie-authenticated unsafe requests need the session's CSRF token
  (`X-CSRF-Token` header or `csrf_token` form field) **and** pass the
  same-origin check: `Sec-Fetch-Site` must not be `cross-site`/`same-site`, and
  an `Origin` must match `server.public_url` or the request's `Host`.
  `SameSite=Lax` already keeps the cookie off cross-site POSTs; the token and
  origin checks are defence in depth.
- Tailnet-authenticated unsafe requests get the same-origin check (see below).
  Bearer requests are exempt: nothing ambient is attached.
- The login and registration forms of the web UI have no session yet; they use
  `security::check_same_origin` plus SameSite.
- Every response carries:
  - `Content-Security-Policy: default-src 'self'; script-src 'self';
    style-src 'self'; img-src 'self' data: blob:; media-src 'self' blob:;
    object-src 'none'; base-uri 'none'; frame-ancestors 'none';
    form-action 'self'`
  - `X-Content-Type-Options: nosniff`
  - `Referrer-Policy: same-origin`
  - `X-Frame-Options: DENY`
  - `Permissions-Policy: camera=(), microphone=(), geolocation=(), payment=(), usb=()`
  - `Cross-Origin-Opener-Policy: same-origin`
  - `Strict-Transport-Security: max-age=31536000` when `server.public_url`
    starts with `https://`

### Rate limiting

In-memory GCRA limiters (`governor`, `security.rs`):

| What | Key | Limit |
|---|---|---|
| Password logins (`/auth/token`, web login), registrations, password changes, TOTP confirm/disable | client IP **and** lowercased username, both must pass | `limits.login_per_minute` |
| Uploads and imports (also Telegram imports) | user | `limits.upload_per_minute` requests |
| Git webhook (`/api/v1/hooks/git/{id}`, unauthenticated) | client IP, checked before the signature | 60 per minute |

Every attempt counts, successful or not. Over the limit: 429 `rate_limited`
with `Retry-After`. The per-username limit means a flood of bad guesses can
delay the real user's login by up to a minute; that is the intended trade-off.
When the client IP is unknown (a trusted-socket request without
`X-Forwarded-For`), the per-username limit applies plus one bucket shared by
all such callers with 5 × `limits.login_per_minute`. The shared bucket caps
password spraying across many usernames through the socket. It is five times
larger than a per-IP bucket, so a single tailnet caller cannot easily starve
the others, and it can never lock out a particular account beyond that
account's own username bucket. Webhook calls with an unknown IP likewise
share one bucket.

### Audit log

Written to the `audit_log` table and readable by admins
(`GET /api/v1/admin/audit`). Each entry has time, actor, action, target, client
IP and JSON details. Writing an entry never fails the action it records.

| Action | When |
|---|---|
| `login` | Successful password login (details: `api_token` or `web_session`) |
| `login_failed` | Failed login (details: username, reason `unknown_user`, `password`, `disabled` or `totp`) |
| `register` | Self-service registration |
| `token.create`, `token.revoke` | API token created (also by password login) or revoked |
| `user.update_self` | Own display name or Telegram user id changed (`PATCH /api/v1/me`) |
| `gitpack.create` | A set was bound to a git repository (URL redacted) |
| `password.change`, `totp.enable`, `totp.disable` | Account security changes |
| `user.create`, `user.update`, `user.delete` | Admin actions on users (`user.create` also from the `create-user` CLI) |
| `user.autocreate`, `user.promote` | Tailnet auto-create; promotion via `auth.tailscale_admins` |
| `user.bootstrap` | Initial admin created from the configuration |
| `invitation.create`, `invitation.delete` | Admin invitation management |
| `set.delete` | A set was deleted |

### No telemetry

The server makes no outbound connections of its own. The only outbound traffic
is what a user configures: git remotes and the Telegram Bot API
(integrations). AI runs in-process.

## Tailscale trust model

`tailscale serve` authenticates tailnet users and adds `Tailscale-User-Login`
and `Tailscale-User-Name` headers. stickpile trusts those headers only when the
request provably came from that proxy (`security::from_trusted_proxy`):

1. **Unix socket (preferred)**: `trusted_proxy.socket`, created with
   `trusted_proxy.socket_mode` (default `0660`), so only root and the service
   user/group can connect. `tailscale serve ... unix:<socket>` points there.
   The socket is never reachable with wider permissions, not even for a
   moment. It is bound inside a fresh `0700` directory next to the target,
   gets its mode there, and is then renamed into place (`app::bind_unix`).
   No process-wide umask change is involved.
2. **Loopback TCP with a shared secret**: for a reverse proxy that cannot use
   a socket. The request must come from a loopback address **and** carry
   `trusted_proxy.secret_header` with the contents of
   `trusted_proxy.secret_file`, compared in constant time. Loopback alone is
   not enough: with rootless containers every local user can reach it.

On every other request the identity headers are ignored. `X-Forwarded-For` is
used for the client IP (rate limiting, audit) only on trusted-proxy requests.

A trusted login maps to the user whose `tailscale_login` matches. If none
exists and `auth.tailscale_autocreate` is on, a user is created (username from
the login's local part, made unique: `alice@github` -> `alice`, `alice-2`, ...;
no password). Logins listed in `auth.tailscale_admins` get the admin role when
created and are promoted on later requests if needed (never demoted).

**Deviation from the original plan**: tailnet requests were meant to be exempt
from CSRF checks. But the proxy attaches the identity to every request from the
device, including a form that a malicious website makes the browser submit, so
it is an ambient credential like a cookie. Tailnet-authenticated unsafe
requests therefore must pass the same-origin check. Non-browser clients (CLI,
scripts) send neither `Origin` nor `Sec-Fetch-Site` and are unaffected.
