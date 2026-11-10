# Web UI

The web UI is rendered on the server with [maud](https://maud.lambda.xyz)
templates and enhanced with [htmx](https://htmx.org) 2 (vendored, no
JavaScript toolchain). Every page works without JavaScript: forms post and
redirect. When htmx is loaded, the same forms swap small fragments in place.
A small hand-written script (`assets/app.js`) adds clipboard copy and
drag-and-drop.

Code: `crates/stickpile-server/src/web/` (one module per area), static files
in `crates/stickpile-server/assets/`, UI-only queries in
`src/service/ui.rs`. All business logic and authorization go through the same
`service` functions as the JSON API, so the web UI can never see or change
more than the API allows.

## Page tour

| Page | What it shows |
|---|---|
| `/login` | Username and password. Accounts with TOTP get a second step for the 6-digit code. Anonymous visitors to `/` see the same form. |
| `/register` | Self-service registration, following `auth.registration`: `closed` explains that and refuses, `invite` asks for the code (`/register?code=…` fills it in), `open` needs nothing. A successful registration signs you in. |
| `/` | Search box, items you copied recently, your sets with covers. |
| `/sets` | Tabs **Mine** and **Public**. Unlisted sets are never listed; they are reachable only by their link. Links to "Add a git pack" and "Import a Telegram pack" appear when those integrations are enabled. |
| `/sets/new` | Name, slug (derived from the name when empty), description, visibility, recommended aspect ratio (`W:H`) and size in pixels. |
| `/sets/{id}` | Set header (owner, visibility, version, git binding), actions (settings, export `.tar`, popular in this set, pull now, publish to Telegram), the upload form, "Replace from a set directory", the filter "hide below N likes" with sorting (set order, likes, copies, quality, recent), and the item editor grid. |
| `/sets/{id}/settings` | The set form plus "Delete this set…". Read-only git packs only offer deletion. |
| `/items/{id}` | Large preview (`<img>` for stills and animations, `<video controls loop muted>` for videos), copy and download, like, metadata with inline editing, "Re-run AI" (when AI is ready), "Move to" another of your sets, delete, the apps that cannot take the item, the quality report table, near-duplicates in your library, "Find a better version", and every variant with a download link. |
| `/upload` | Upload files to one of your sets; import a set folder or an export `.tar` as a new set. |
| `/search?q=` | The same hybrid search as `GET /api/v1/search`, a collapsible syntax cheat-sheet, and whether semantic search is on. Results update while you type (htmx) and "More results" appends the next page. |
| `/popular` | Most copied, most liked and trending side by side, never blended. Scope: whole server, your items, or one set (`?scope=set&set=…`, linked from the set page). The window (day, week, month, all) applies to "most copied". |
| `/stats` | Your totals, a bar chart of copies per day for 30 days (inline SVG with a hover title per bar and a screen-reader table), your most copied items and the apps you paste into. |
| `/settings` | Display name and Telegram user id, password change (current password required if you have one), two-step verification setup (QR code and key, confirm with a code) and disabling, API tokens (create, the secret is shown once, revoke). |
| `/admin` | Admins only. See [Admin panel](#admin-panel). |
| `/jobs/{id}` | Status of a background job (yours, or any job for admins). |

Git packs (`/gitpacks/new`) and Telegram (`/telegram/import`,
`/sets/{id}/telegram`) are described in [Integrations](#integrations).

### Tailnet users

When stickpile is opened through `tailscale serve`, the tailnet identity signs
you in automatically. The top bar shows your name with a `tailnet` badge and
no "Sign out" button (there is no session to end). `/login` redirects to the
start page.

## Keyboard and mouse

- **Skip link**: the first Tab stop jumps to the main content.
- **Search from anywhere**: the search field in the top bar. On the search page
  results refresh 0.4 s after you stop typing; Enter searches immediately.
- **Inline editing** (set page and item page): activate "Edit" next to a field.
  The field turns into a small form with the cursor in it. Enter saves,
  Escape (or "Cancel") restores the view. Lists are comma-separated (keywords,
  tags), emojis space-separated, an empty vibe clears it. Invalid input stays
  in the form with the error next to it. Without JavaScript "Edit" opens the
  item page's full form (`?edit=1`), which saves only the fields you changed.
- **AI suggestions** are marked with an `AI` badge and a tinted row. "✓ Confirm"
  keeps the value and marks it as yours; editing it does the same with your
  value. AI values you leave alone stay marked (also in exports).
- **Reorder**: ↑ and ↓ on each card move it by one place. They are only shown in
  set order without the likes filter.
- **Delete** asks for confirmation (a browser dialog with JavaScript, a
  confirmation page without).
- **Likes**: the ♡/♥ button toggles your like (`aria-pressed` tells screen
  readers the state).
- **Drag and drop**: drop files on the dashed upload area to upload them at
  once; "Choose files" opens the file dialog. Folders go through "Set folder"
  (the browser's directory picker).

Focus is always visible (a 3 px outline), every input has a label, images
have the item name as alt text, and colours follow `prefers-color-scheme`.

## Copy to clipboard

Copy buttons need JavaScript and stay hidden without it. "Download" always
works.

| Media | Button | What happens |
|---|---|---|
| Still (PNG, JPEG, WebP) | **Copy** | The PNG is fetched (the original if it is a PNG, else the `png` variant; if that is not generated yet the original is converted to PNG in the browser) and written as `image/png` with `navigator.clipboard.write`. Chat apps paste it as a picture. |
| Animation (GIF, animated WebP) | **Copy link** + **Download** | The absolute file URL is copied. |
| Video (MP4, WebM) | **Copy link** + **Download** | Same. |

**Why no GIF or video copy?** The async Clipboard API only writes
`image/png`, `text/plain` and `text/html` to the system clipboard. Writing
`image/gif` or a video type is rejected, and converting a GIF to PNG keeps
only its first frame. Chat apps need the actual file (`text/uri-list` or the
GIF bytes), which only a native program can offer. So animations and videos get "Copy link" (the link
opens for people signed in to this server; share links are a post-v1 feature)
and "Download" (then drag the file into the chat). The desktop client puts the
right formats on the clipboard; see [clipboard.md](clipboard.md).

**Secure context.** The Clipboard API only exists on `https://` pages and on
`localhost`. On plain `http://` the buttons explain this and point to
Download and the desktop client. `tailscale serve` provides HTTPS on the
tailnet.

**Usage statistics.** After a successful copy the page reports a usage event
(`POST /api/v1/usage`, `client: "web"`, a random event id, the variant). It
counts for popular items and your stats. Downloads are not counted.

## Upload and import

- **Upload** takes many files at once (file picker or drag-and-drop). The
  results list every file: added (with a link), already in the set, or the
  error (unsupported type, too large, corrupt). Near-duplicates in your own
  library are reported as "You already have this in set X: name" with links
  to the set and the item. On the set page the item grid refreshes after an
  upload.
- **Import a set directory as a new set** (upload page): pick the folder
  holding `set.toml` and the files, or an export `.tar`. The set gets the
  manifest's name, slug, visibility, metadata and AI markers.
- **Replace from a set directory** (set page): makes the set equal to the
  directory. Matching files get the directory's metadata, new files are added,
  items missing from the directory are **removed**, and the order follows the
  file names. The form says so before you submit.

Limits are the server's (`limits.max_upload_mb` per file,
`limits.max_request_mb` per request, `limits.upload_per_minute`).

## Admin panel

Only users with the `admin` role see the "Admin" link; everyone else gets 403
on `/admin*`. Admins get no extra access to other users' content in the rest
of the UI.

| Tab | Contents |
|---|---|
| Overview | Media and database size, media and item counts, waiting jobs, the registration policy (read-only, it comes from the configuration), AI status, storage per user, the job queue by kind and status, and the latest failed jobs with their errors. |
| Users | Every user with role, status (disabled, TOTP, no password), tailnet login, last sign-in, sets/items and storage. "Manage" per user: set the role, disable or enable, reset the password, reset TOTP (lost device), bind or unbind a tailnet login, delete (with a confirmation page listing what is deleted). Plus "Create a user". The server refuses to demote, disable or delete the last active admin and to delete yourself; the message is shown on the page. |
| Invitations | Create an invitation (role, validity). The link with the one-time code is shown **once**. The list shows who created each invitation and whether it is open, used (by whom) or expired; open ones can be revoked. Invitations only work while `auth.registration = "invite"`. |
| Audit log | Newest first, 50 per page ("Older entries"): time, actor, action, target, IP, details. |

Every admin action goes through `service::admin` and is written to the audit
log.

## Integrations

Shown only when enabled on the server.

- **Git packs** (`git.enabled`): "Add a git pack" asks for the repository URL,
  branch, folder, mode (read-only, or push edits back) and slug. After
  creating it the page shows the webhook URL and secret **once**, plus the
  status of the first pull. Git sets show their binding (URL, branch, last
  pull, last error) and "Pull now"; read-only packs show "Edit upstream" and no
  edit controls.
- **Telegram** (`telegram.enabled` and a bot token): "Import a Telegram pack"
  (pack name or `t.me/addstickers/…` link) creates a set in the background;
  "Publish to Telegram" on your sets creates or updates the pack. Packs belong
  to your Telegram user id (Settings) or the server's default owner.

Background work (git pulls, Telegram, "Re-run AI") shows a status box that
polls itself every 2 seconds with htmx (`hx-trigger="every 2s"`) until the job
is done or failed, then links to the result. Without JavaScript the job page
refreshes itself while the job runs.

## Errors

Browser requests get HTML error pages: 404 "Not found" (also for content you
may not see, so ids cannot be probed), 403 "Not allowed" (admin pages, edits of
others' sets), 403 "Request blocked" (CSRF), 429, 5xx. Anonymous visitors are
redirected to `/login?next=…` and come back after signing in. htmx requests
get the message in the page's status region (`#flash`, via `HX-Retarget`) and
`401` with `HX-Redirect: /login` when the session has ended. Form validation
errors re-render the form with your input and the message.

## Security notes

- **CSP** (`security.rs`): `script-src 'self'; style-src 'self'` and no
  exceptions. The pages contain no inline `<script>`, no `style=` attributes
  and no inline event handlers; tests assert this on every page. All styling
  is class-based (`assets/app.css`); the chart and the TOTP QR code are inline
  SVG using only presentation attributes. htmx is configured through
  `<meta name="htmx-config">` with `includeIndicatorStyles: false` (no
  injected `<style>`), `allowEval: false`, `allowScriptTags: false`,
  `selfRequestsOnly: true` and `historyCacheSize: 0` (no page snapshots, which
  contain CSRF tokens, in `localStorage`).
- **CSRF**: cookie sessions need the session's token on every unsafe request
  plus a same-origin `Origin`/`Sec-Fetch-Site`. Forms carry a hidden
  `csrf_token` field (checked by the auth middleware for urlencoded bodies);
  htmx sends `X-CSRF-Token` from `hx-headers` on `<body>`; app.js sends the
  header with its `fetch` calls. Plain multipart forms (upload, import) cannot
  set a header and the middleware does not parse multipart, so their token
  must be the **first** form field; `web::MultipartCaller` checks it (and the
  same-origin rule) before any file is read. Tailnet users have no token and
  pass the same-origin check instead. Login and registration have no session
  yet: they use the same-origin check plus `SameSite=Lax`.
- **Cookies**: `stickpile_session` is `HttpOnly`, `SameSite=Lax`, `Path=/`, and
  `Secure` unless `server.secure_cookies = false`. Signing in replaces any
  session cookie the browser already had; signing out deletes the session on
  the server and clears the cookie. Changing your password signs out your
  other sessions.
- **TOTP step**: after a correct password the server keeps the pending login in
  memory under a random 256-bit one-time token for 5 minutes and 5 attempts.
  The password never goes back to the browser; the code check goes through the
  normal login (rate limits, replay protection, audit).
- **Escaping**: maud escapes every interpolated value (text and attributes);
  only server-generated SVG is inserted unescaped.
- **Redirects**: `next` parameters must be local paths (`/…`, not `//host` or
  `/\host`); anything else falls back to `/`.
- **Uploads** go through the same limits, content sniffing and sanitising as
  the API; the request body is capped at `limits.max_request_mb` before
  parsing.
- **External links** (git upstream, Telegram) use
  `rel="noopener noreferrer"`. No fonts, scripts or styles are loaded from
  other origins.

## Static assets

`/static/app.css`, `/static/app.js`, `/static/htmx.min.js` and
`/static/favicon.svg` are compiled into the binary and served with
`Cache-Control: public, max-age=31536000, immutable`. Pages link them with a
content hash (`?v=…`), so upgrades are picked up at once.

## Tests

`crates/stickpile-server/tests/web_auth.rs`, `web_pages.rs` and
`web_flows.rs` (helpers in `tests/web_common/`) cover: every page renders for
its user; anonymous redirects (401 + `HX-Redirect` for htmx); 404 for content
you cannot see; 403 for admin pages and others' sets; CSRF on urlencoded,
htmx and multipart forms, and cross-site posts; the login flow and cookie
flags; logout; the TOTP step; registration per policy; tailnet sign-in;
htmx fragments without the document shell; escaping of `<script>` and quotes;
the CSP header and the absence of inline styles and scripts; static asset
types; HTML error pages; and the flows end to end (sets, uploads with
near-duplicate warnings, imports, editing, likes and filters, stats and
popular, settings, the admin panel, git pack and Telegram forms, job polling).
