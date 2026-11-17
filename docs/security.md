# Security

stickpile is self-hosted and usually reached over a Tailscale network, but the
server is built to be safe when exposed to the internet. This page describes
the threat model and how each security requirement is met.
Code references are to `crates/stickpile-server/src/`.

## Threat model

| Attacker | Can | Must not be able to |
|---|---|---|
| Anonymous internet client | reach the TCP listener | read or change anything except `healthz`, the OpenAPI document, login, pairing-code exchange and registration (per policy); guess passwords or pairing codes quickly |
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
- **Pairing codes**: 60 random bits, stored as sha256, single use, five
  minutes; exchanged anonymously for an API token. See [Pairing](#pairing).
- **Passwords**: argon2id with the `argon2` crate defaults (19 MiB, t = 2,
  p = 1), hashed and verified on the blocking pool. Unknown users and users
  without a password are verified against a dummy hash so timing does not
  reveal whether an account exists. Unknown user, wrong password and disabled
  account give the same error (`auth/password.rs`, `auth/login.rs`). The
  minimum length is `auth.min_password_length` (default 8, never below 4);
  below 8 the login rate limits are what stands between a guesser and the
  account, so keep such a server off the public internet.
- **TOTP** (optional): RFC 6238, SHA-1, 6 digits, 30 s, plus or minus one
  step, compared in constant time. The last accepted step is stored, so a code
  cannot be replayed. Enrolment is pending until confirmed with a valid code;
  an admin can reset TOTP for a lost device (`auth/totp.rs`).
- Changing your own password needs the current one, except on a request
  signed in through the tailnet: `tailscale serve` has just vouched for the
  person, which is what the current password would prove. It revokes your
  other web sessions but keeps your API tokens, so the CLI and desktop
  clients stay logged in (revoke tokens explicitly if a device may be
  compromised). A forgotten password is reset by an admin or on the host with
  `stickpile-server reset-password` (sessions revoked; tokens only with
  `--revoke-tokens`); there is no email-based reset. An admin password reset
  assumes the old credentials are compromised and revokes all web sessions
  **and all API tokens** of the user (audit detail `tokens_revoked`).
  Disabling an account revokes its sessions; disabled users cannot
  authenticate by any method.

### Pairing

Pairing lets a new device (the phone app, or `stickpile login --code`) get
an API token without a password typed on it ([api.md](api.md#pairing)). A
signed-in user makes a one-time code, `POST /api/v1/pairings` or Settings,
"Add a device" (a form with the session's CSRF token, like every other
form); the device sends it to the anonymous `POST /api/v1/auth/pair`
(`auth/pairing.rs`).

- **Format and entropy.** 12 characters of Crockford's base32 (no `I`, `L`,
  `O`, `U`), so 60 bits from the OS CSPRNG, shown as `7KQ2-M9XA-4FTR`.
  Input is normalised before it is hashed (case, spaces and dashes ignored,
  `O` read as `0`, `I` and `L` as `1`), so the forgiving reading costs no
  entropy: it only maps typos onto the same code.
- **Storage.** Only the sha256 of the canonical code is stored
  (`pairing_codes`, migration `0011`), with the user, creation time, expiry
  and when it was used. The code itself is only in the one response (and
  the QR code of the web page, which is sent with `Cache-Control:
  no-store`), never in the database or the audit log. A plain sha256 is
  enough: the code is random, not chosen by a person, and a slow hash on an
  anonymous endpoint would only spend CPU on every guess.
- **Life and single use.** A code works for five minutes and once. The
  exchange claims it with one statement, `UPDATE ... SET used_at = now
  WHERE code_hash = ? AND used_at IS NULL AND expires_at > now RETURNING`,
  so of two concurrent exchanges exactly one wins. The token name is
  checked before, so a bad name does not spend the code. A code of an
  account that was disabled after it was made is spent and refused. Used
  and expired codes are deleted an hour after they expire (lazily when a
  code is made, and by the periodic cleanup); until then a late attempt is
  audited with the right reason.
- **No oracle.** A wrong, expired or used code, a code of a disabled
  account and input that cannot be a code (also long or empty input) all
  get the same `401` with the same body (`invalid or expired pairing
  code`). Every input takes the same path: it is hashed (malformed input as
  it is, which matches no stored hash) and looked up with the one `UPDATE`;
  a failure then reads the row by the same hash for the audit reason. The
  lookup is by sha256, so its timing does not depend on how close a guess
  is to a real code. What timing could at most tell is whether a row with
  that hash exists, which is only the case for codes that are already used
  or expired: dead codes, worthless to an attacker. A success takes longer
  (a token is made), but a success is visible anyway.
- **Rate limit.** Every exchange, successful or not, counts against the
  per-client-IP login bucket (`limits.login_per_minute`), shared with
  password logins and registrations; callers with an unknown IP share the
  larger bucket, as for logins. A code belongs to no username, so there is
  no per-username bucket: a server-wide bucket in its place would let
  anyone block every pairing on the server, and the code's entropy makes it
  unnecessary (below). Over the limit: 429 `rate_limited` with
  `Retry-After`, before the code is looked at.
- **Why 60 bits is enough.** With the default 10 attempts per minute and
  address, one address gets 50 guesses during a code's five minutes. Even a
  botnet of a million addresses, each at the full rate, makes 5 * 10^7
  guesses in that time; with `k` codes alive at once it hits one with
  probability about 5 * 10^7 * k / 2^60 = 4 * 10^-11 * k. Kept up for a
  year with ten codes always alive (a self-hosted server has far fewer:
  a code lives only while someone adds a device), that is about 5 * 10^-5,
  with some five trillion `login_failed` entries in the audit log along the
  way (and 170 000 requests a second, more than the server would answer). 40 bits would not be: the same botnet would have a 1 in 20 000
  chance per code. The bits also cover a leaked database: finding a live
  code from its sha256 takes about 2^59 hashes on average, on the order of
  a GPU-year at 2 * 10^10 sha256 per second, against five minutes of life.
- **What the token is.** The exchange makes an ordinary API token of the
  account that made the code (no expiry, the `token_name` the device sent),
  listed in Settings and revocable there. It skips TOTP: the code was made
  by a session that already passed the account's sign-in. Anyone who sees
  the code (or the QR code) before it is used or expires can sign in as
  the user, which the page says.
- **Audit.** Creation is `pairing.create` (actor, the pairing id, its
  expiry). An exchange is recorded like a password login: `login` with
  `method: pairing` and `token.create` with `via: pairing`; a failure is
  `login_failed` with `method: pairing` and the reason (`unknown_code`,
  `expired_code`, `used_code`, `disabled`) and, when the code was known, its
  user as the actor.

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
- Telegram packs: the shared bot can edit every pack it created, so the
  server records which user created or adopted each pack and refuses exports
  and import bindings by anyone else (`service/telegram.rs`,
  [telegram.md](telegram.md#pack-ownership)).
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

### Reading the words in pictures (OCR)

- The input is the stored, already validated and re-encoded media
  (`stickpile_media::text_frames` of the sanitised original), never the
  raw upload; at most three frames per file (first, middle, last). The
  upload tray's `POST /api/v1/ai/suggest` reads its posted image under the
  same limits as its other suggestions (2 MiB, its pixel bound, the
  per-user rate limit).
- Pixels are bounded before any model runs: frames above 2048 x 2048 px
  are scaled down; the reader works on at most 2000 px on the long side
  (thin strips are scaled up and padded, and not read when that would
  exceed 16 MP), the detector input is at most 2000 px, and at most 1000
  contours and 300 boxes per frame are processed. All of this is pure
  Rust; the two ONNX models run in the same ONNX Runtime as CLIP.
- It runs in-process on the blocking pool, without network access. The
  model files are fetched ahead of time with pinned sha256 hashes
  (`scripts/fetch-models.sh`, `nix/models.nix`); the server never
  downloads anything. Missing or broken models turn OCR off with a log
  line; they never stop the server. There is no timeout (as for CLIP),
  because the work is bounded as above.
- The output is untrusted text: it is cleaned like any text a user types
  (control characters removed, at most 2000 characters), stored as data,
  escaped by maud in the web UI like every other field, and written to
  `set.toml` as a TOML string. Clients must show it as plain text, never
  as markup.

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
| Pairing-code exchanges (`/auth/pair`) | client IP, the same buckets as password logins (no username: see [Pairing](#pairing)) | `limits.login_per_minute` |
| Uploads and imports (also Telegram imports) | user | `limits.upload_per_minute` requests |
| AI suggestions (`/api/v1/ai/suggest`, the upload tray) | user | `limits.ai_suggest_per_minute` requests |
| Rechecks of one's own items (`POST /api/v1/rechecks`) | user | `limits.recheck_per_hour` new rechecks per hour (asking while the same one runs is not counted) |
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
| `login` | Successful password login (details: `api_token` or `web_session`) or pairing-code exchange (method `pairing`, the pairing id) |
| `login_failed` | Failed login (details: username, reason `unknown_user`, `password`, `disabled` or `totp`); failed pairing-code exchange (method `pairing`, reason `unknown_code`, `expired_code`, `used_code` or `disabled`) |
| `register` | Self-service registration |
| `token.create`, `token.revoke` | API token created (also by password login, or by a pairing-code exchange: `via: pairing`) or revoked |
| `pairing.create` | A pairing code was made (target: its id; details: `expires_at`; never the code) |
| `user.update_self` | Own username (`{from, to}`), display name or Telegram user id changed (`PATCH /api/v1/me`) |
| `gitpack.create` | A set was bound to a git repository (URL redacted) |
| `password.change`, `totp.enable`, `totp.disable` | Account security changes |
| `user.create`, `user.update`, `user.delete` | Admin actions on users (`user.create` also from the `create-user` CLI) |
| `user.autocreate`, `user.promote` | Tailnet auto-create; promotion via `auth.tailscale_admins` |
| `user.bootstrap` | Initial admin created from the configuration |
| `user.password_reset` | Password reset with `stickpile-server reset-password` (details: `tokens_revoked`, `reset_totp`) |
| `invitation.create`, `invitation.delete` | Admin invitation management |
| `set.delete` | A set was deleted |
| `recheck.start`, `recheck.cancel` | An admin started a server-wide recheck (target: its id; details: `scope`, `parts`, `mode`, `set_id`, `total`), or cancelled one (details: `dropped`, the queued pictures deleted) |

### What the clients store on the device

The CLI, the desktop popups and the Android app keep an offline cache
(`crates/stickpile-core/src/cache.rs`; `$XDG_CACHE_HOME/stickpile` by
default, `cache_dir` in the client config; on Android the app's
`noBackupFilesDir/cache`, which the system neither backs up nor empties):

- `cache.sqlite3`: the metadata of the items the account can list (own and
  public), the cached account (`/me`), the local copy history, usage
  events not yet uploaded, and what is kept offline (the chosen stickers,
  packs or library, failed downloads with the server's error text,
  download claims).
- Files: thumbnails, the renditions that were copied (evicted least
  recently used beyond `cache_max_mb`), and the files kept offline
  (thumbnails, the renditions a copy to the configured chat apps uses, pack
  covers), which stay until they are released, removed or the account goes,
  bounded by `offline.max_mb` ([offline.md](offline.md)).

Nothing in the cache is a secret: the API token is a separate file (mode
`0600`, `token_file`), never in the cache. The files are the pictures the
account can see, so anyone who can read the user's cache directory can see
them; the cache relies on the user's file permissions and, on Android, on
the app's private storage.

How they are removed:

- "Clear cache" deletes the cached files that are not kept and says how
  many kept files stay.
- "Remove kept files" (confirmed) deletes every kept file and forgets what
  to keep.
- Signing out (`stickpile logout`, the popups, the app), signing in as
  another account and pointing the client at another server drop the
  account's data from the cache: items, history, queued usage, and the kept
  files with the list of what to keep. A client up to 0.8.0 does not know
  the kept files; when it signed out, a newer client sharing the cache
  deletes them the next time it brings its offline files in line (after a
  sync, a keep or a download run).
- Deleting the cache directory removes everything; the clients rebuild it.

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
