# Web UI

The web UI is rendered on the server with [maud](https://maud.lambda.xyz)
templates and enhanced with [htmx](https://htmx.org) 2 (vendored, no
JavaScript toolchain). Every page works without JavaScript: forms post and
redirect. When htmx is loaded, the same forms swap small fragments in place.
Small hand-written scripts add the rest: `assets/app.js` clipboard copy
and the item context menu; `assets/prefs.js` the display options ("View")
and the theme switch; `assets/upload.js` drag-and-drop, pasting and the
upload tray, where files can be renamed, described (with AI suggestions to
approve) and images rotated, flipped and cropped before they are uploaded;
`assets/emoji.js` the emoji picker of the Emojis fields.

Code: `crates/stickpile-server/src/web/` (one module per area), static files
in `crates/stickpile-server/assets/`, UI-only queries in
`src/service/ui.rs`. All business logic and authorization go through the same
`service` functions as the JSON API, so the web UI can never see or change
more than the API allows.

## Page tour

| Page | What it shows |
|---|---|
| `/login` | Username and password. Accounts with TOTP get a second step for the 6-digit code. Anonymous visitors to `/` see the same form. "Forgot your password?" lists the ways back in: the tailnet, an admin, or `stickpile-server reset-password` on the host (there is no email). |
| `/register` | Self-service registration, following `auth.registration`: `closed` explains that and refuses, `invite` asks for the code (`/register?code=…` fills it in), `open` needs nothing. A successful registration signs you in. |
| `/` | Search box, items you copied recently, your sets with covers (with the set lists' "View" control). |
| `/sets` | Tabs **Mine** and **Public**. Unlisted sets are never listed; they are reachable only by their link. Links to "Add a git pack" and "Import a Telegram pack" appear when those integrations are enabled. Each card shows the cover, name, description, item count, visibility and source labels, owner and slug; the "View" control arranges them (see [Display options and themes](#display-options-and-themes)). |
| `/sets/new` | Name, slug (derived from the name when empty), description, visibility, recommended aspect ratio (`W:H`) and size in pixels. |
| `/sets/{id}` | Set header (owner, visibility, version, git binding), actions (settings, export `.tar`, popular in this set, pull now, publish to Telegram), the upload form, "Replace from a set directory", the filter "hide below N likes" with sorting (set order, likes, copies, quality, recent) and the "View" control (style and shown fields), and the item editor grid (see [Set page](#set-page)). |
| `/sets/{id}/settings` | The set form, with the **cover** chosen from the set's thumbnails ("First item" by default), plus "Delete this set…". Read-only git packs only offer deletion. |
| `/items/{id}` | Large preview (`<img>` for stills and animations, `<video controls loop muted>` for videos), copy and download, like, metadata with inline editing, "Re-run AI" (when AI is ready), "Move to" another of your sets, delete, the apps that cannot take the item, the quality report table, near-duplicates in your library, "Find a better version", and every variant with a download link. |
| `/upload` | Upload files to one of your sets; import a set folder or an export `.tar` as a new set. |
| `/search?q=` | The same hybrid search as `GET /api/v1/search`, a collapsible syntax cheat-sheet, and whether semantic search is on. Results update while you type (htmx) and "More results" appends the next page. |
| `/popular` | Most copied, most liked and trending side by side, never blended. Scope: whole server, your items, or one set (`?scope=set&set=…`, linked from the set page). The window (day, week, month, all) applies to "most copied". |
| `/stats` | Your totals, a bar chart of copies per day for 30 days (inline SVG with a hover title per bar and a screen-reader table), your most copied items and the apps you paste into. |
| `/settings` | Username (shown as the owner of your sets and matched by `by:`; renaming keeps you signed in), display name and Telegram user id, the theme (System, Light, Dark), password change (current password required if you have one, except when you are signed in through the tailnet; at least `auth.min_password_length` characters), two-step verification setup (QR code and key, confirm with a code) and disabling, API tokens (create, the secret is shown once, revoke). |
| `/admin` | Admins only. See [Admin panel](#admin-panel). |
| `/jobs/{id}` | Status of a background job (yours, or any job for admins). |

Git packs (`/gitpacks/new`) and Telegram (`/telegram/import`,
`/sets/{id}/telegram`) are described in [Integrations](#integrations).

### Tailnet users

When stickpile is opened through `tailscale serve`, the tailnet identity signs
you in automatically. The top bar shows your name with a `tailnet` badge and
no "Sign out" button (there is no session to end). `/login` redirects to the
start page.

## Set page

The item editor grid shows one card per item: the thumbnail with its kind
and quality badges, then the fields in a dense layout. The name is the
card's title; keywords, vibe and tags have small uppercase labels;
keywords and tags are chips (tags with `#`); emojis and the vibe share a
line when both fit. A pencil after each field edits it in place (see
below), "✓" confirms an AI suggestion. Below the fields one row of small buttons:
like (♡/♥ with the count), Copy, Download (icon), and for editors ↑ and ↓
(set order only) and Delete (icon). Every icon button has a text name for
screen readers and a tooltip. On narrow screens (below 40rem) the cards
become rows: a small thumbnail on the left, fields and buttons on the
right.

**Styles and shown fields.** The "View" button next to the sort opens the
display options: the style **Regular** (every field and the button row),
**Compact** (a denser grid of thumbnails with the name, and the emojis on a
line of their own at the bottom of the card; the other fields and the
button row are hidden, the context menu has the actions) or **Grid**
(thumbnails only, with "⋯" on the thumbnail's corner), and which fields
show: Emojis, Keywords, Vibe, Tags and Labels (the kind, quality and
"Cover" badges). See [Display options and themes](#display-options-and-themes).

**Context menu.** Right-click an item card (either view) for its actions:

| Entry | Shown when | Does |
|---|---|---|
| Copy image / Copy link | always | the card's copy button (same rules as [Copy to clipboard](#copy-to-clipboard)) |
| Like / Unlike | always | the card's like button |
| Open item page | always | link to `/items/{id}` (middle-click opens a tab) |
| Download original | always | download link |
| Rename | you can edit the set | inline edit of the name (also in the compact and grid views, where the form opens inside the small card) |
| Edit all fields… | you can edit the set | `/items/{id}?edit=1` |
| Use as cover / Stop using as cover | you can edit the set | makes the item the set's cover (or goes back to the first item) through `POST /sets/{id}/cover`; the grid comes back in the page's sort with the "Cover" badge on that card |
| Move up, Move down, Move to top, Move to bottom | you can edit the set, set order, no likes filter | the set's move endpoint; the ends are disabled for the first and last item |
| Delete… | you can edit the set | asks for confirmation, then removes the card |

The menu is built by app.js from `data-*` attributes the server puts on the
card only when the action is allowed (`data-edit-url`, `data-delete-url`,
`data-move-url` with `data-first`/`data-last`, `data-cover-url` with
`data-cover` on the current cover), and it uses the same
endpoints, htmx requests and CSRF header as the buttons. It also opens from
the keyboard (the context-menu key or Shift+F10 while focus is in a card)
and from the "⋯" button on each card (for touch screens and keyboards).
Inside the menu: ↑/↓ move between entries (wrapping), Home/End jump,
typing a letter jumps to the entry starting with it, Enter or Space
activates, Escape or Tab closes and returns focus to where it was. A click
outside, scrolling or resizing closes it. After a move from the keyboard,
focus follows the item. Ctrl+ or Shift+right-click (and right-clicking a
text field, e.g. while editing) keeps the browser's own menu. Without
JavaScript there is no custom menu and no "⋯"; the browser's menu and the
card buttons work as before.

<!-- ---- display options and themes (begin) ---- -->

## Display options and themes

**"View" on the set page.** A disclosure button in the filter form. Its
panel has the style (Regular, Compact, Grid) and the "Show" toggles for
Emojis, Keywords, Vibe, Tags and Labels; the name always shows. Toggles a
style has no room for are disabled with a note (Compact shows the name and
the emojis; Grid only thumbnails and labels). Everything applies at once:
`assets/prefs.js` puts a style class (`compact`, `view-grid`) and
`hide-<field>` classes on `#items-section`, outside the swapped grid, so
they survive reordering and uploads, and CSS hides the fields without
leaving gaps. A field being edited stays visible, and Rename from the
context menu opens its form inside the card in every style. The choices
are remembered in `localStorage` (`stickpile.items.view`,
`stickpile.items.hide`; the old `stickpile.setView` of the "Compact view"
checkbox is still read) for every set page. The address bar carries the
style (`?view=compact` or `?view=grid`; none for Regular), so a reload,
"Apply" and htmx requests agree with what is shown; a `view` parameter in
the URL wins over the remembered style and becomes the remembered one.
Escape or a click outside closes the panel.

**"View" on set lists** (`/sets` and the start page's "Your sets"). The
style **Regular cards** or **Compact** (smaller cards, one-line name and
description), and toggles for the parts of a card: Cover, Description,
Item count, Labels (visibility and source), Owner and Slug. Kept apart from
the set page's options (`stickpile.sets.view`, `stickpile.sets.hide`),
applied as classes on the `.set-list` wrapper and shared by every set list.

**Before the first paint.** prefs.js is loaded without `defer`: once in
`<head>`, and once more as the first child of `#items-section` and of each
`.set-list`, where it applies the remembered classes to its parent before
the cards are parsed, so hidden fields never show up and vanish again.

**Without JavaScript** every field and every part of a set card shows. The
set page's style radios are part of the filter form: "Apply" submits them
with the likes filter and the sort, and the server renders the style class
from `?view=`. The show/hide toggles and the set lists' control need
JavaScript and are hidden without it.

**Themes.** Settings → Appearance offers **System** (follow the device's
light or dark setting, the default), **Light** and **Dark**, stored with
the account (`users.theme`, `theme` in `PATCH /api/v1/me`). For Light and
Dark the server renders `<html data-theme="light|dark">` and a matching
`<meta name="color-scheme">`, so pages open in the right colours; System
renders no `data-theme`. In app.css the light palette is on `:root`; the
dark one applies to `[data-theme="dark"]` and, without a `data-theme`,
under `prefers-color-scheme: dark`; `color-scheme` follows, so form
controls and scrollbars match. Colours that differ between the themes are
variables; nothing else depends on `prefers-color-scheme`.

**The switch in the top bar** (a moon in the light theme, a sun in the
dark one) is a toggle button named "Dark theme" whose `aria-pressed` says
whether the page is dark. It flips from the theme in effect (also under
System, where it follows the device and updates when the device changes),
applies at once and saves it with `PATCH /api/v1/me` (session cookie and
`X-CSRF-Token`); a failure is reported in the status region. Without
JavaScript it is a form posting the other theme to `POST /settings/theme`
(with `csrf_token`), which returns to the page it came from. The account's
choice is also remembered in `localStorage` (`stickpile.theme`), and pages
without a session (sign-in, registration) apply it in `<head>`; there the
switch only changes that remembered value. Signing in, the account's theme
wins.

<!-- ---- display options and themes (end) ---- -->

## Keyboard and mouse

- **Skip link**: the first Tab stop jumps to the main content.
- **Search from anywhere**: the search field in the top bar. On the search page
  results refresh 0.4 s after you stop typing; Enter searches immediately.
- **Inline editing** (set page and item page): activate "Edit" next to a field
  (the pencil on the set page).
  The field turns into a small form with the cursor in it. Enter saves,
  Escape (or "Cancel") restores the view. Lists are comma-separated (keywords,
  tags), emojis space-separated, an empty vibe clears it. Invalid input stays
  in the form with the error next to it. Without JavaScript "Edit" opens the
  item page's full form (`?edit=1`), which saves only the fields you changed.
- **Emoji picker**: the Emojis field (inline, in the full form and in the
  upload tray) has a smiley button that opens a panel under it: a search box
  (English names and keywords from Unicode CLDR plus the slang search knows,
  so "cat" finds 🐱 😺 🐈 and "laugh" or "lol" finds 😂 🤣), category tabs,
  your recently used emojis (kept in `localStorage`, `stickpile.recentEmojis`)
  and the emojis. A pick appends the emoji to the field, space-separated,
  unless it is already there; the panel stays open for more. Enter in the
  search box picks the first result, ↓ moves to the results; the tabs, the
  recent row and the grid are one Tab stop each, arrow keys (and Home/End)
  move inside them. Escape closes the panel and returns to the button (a
  second Escape cancels an inline edit), as does a click outside. Skin-tone
  and hair-style variants are not offered; type them if you need them. The
  data (`/static/emoji.json`) is fetched the first time a panel opens or a
  `:` is typed.
- **`:` suggestions**: typing `:` in an Emojis field searches without the
  panel. `:cat` lists up to 8 matches with their names right under the field,
  the best one highlighted (the same search as the panel; `_` joins words, as
  in `:thumbs_up`); a bare `:` lists your recently used emojis (those not in
  the field yet), or common ones if there are none. Space, Enter or Tab
  replaces `:cat` with the highlighted emoji and a space, so the next `:` can
  follow; a click on a suggestion does the same. ↑/↓ move the highlight
  (wrapping around). Escape closes the list and keeps the text (it does not
  cancel an inline edit); so do deleting the `:`, moving the caret away,
  typing a character that is not part of a name, and leaving the field. With
  no matches Space types a space and ends the search, and Enter does nothing:
  while a search is open, Enter never submits (the inline editor saves on the
  next Enter). Only a typed `:` starts a search, not a pasted one. Picks count
  as recently used; an emoji already in the field is not added twice. The
  field is an ARIA combobox (`aria-autocomplete="list"`, the highlighted
  option as `aria-activedescendant`, the result count in a polite live
  region), and keys are ignored while an input method is composing.
- **AI suggestions** are marked with an `AI` badge and a tinted row. "✓ Confirm"
  keeps the value and marks it as yours; editing it does the same with your
  value. AI values you leave alone stay marked (also in exports). In the
  upload tray the same is called "✓ Approve" (see [Upload and
  import](#upload-and-import)).
- **Reorder**: ↑ and ↓ on each card move it by one place; the context menu
  also moves to the top or bottom. They are only offered in set order
  without the likes filter.
- **Delete** asks for confirmation (a browser dialog with JavaScript, a
  confirmation page without).
- **Context menu** on set page cards: right click, the context-menu key,
  Shift+F10 or "⋯". See [Set page](#set-page).
- **Likes**: the ♡/♥ button toggles your like (`aria-pressed` tells screen
  readers the state).
- **Drag and drop and paste**: drop files on the dashed upload area, or
  paste an image anywhere on the page (Ctrl+V), to add them to the upload
  tray; "Choose files" opens the file dialog. Folders go through "Set folder"
  (the browser's directory picker). See [Upload and import](#upload-and-import).

Focus is always visible (a 3 px outline), every input has a label, images
have the item name as alt text, and colours follow the theme (see
[Display options and themes](#display-options-and-themes)).

## Copy to clipboard

Copy buttons need JavaScript and stay hidden without it (the stylesheet
enforces the `hidden` attribute). "Download" always works.

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

- **Upload** takes many files at once (file picker, drag-and-drop or
  paste). The results list every file: added (with a link), already in the
  set, or the error (unsupported type, too large, corrupt). Near-duplicates
  in your own library are reported as "You already have this in set X: name"
  with links to the set and the item. On the set page the item grid
  refreshes after an upload.
- **The upload tray** (with JavaScript): chosen, dropped and pasted files
  are staged first, as a row of thumbnails, and nothing is sent until you
  press "Upload N files". Select a thumbnail to edit that file: its details
  on the left, the image on the right (stacked on narrow screens). × removes
  it from the upload, "Remove all" empties the tray.
  - **Name**: the item name. It starts as the file name without the
    extension and without a leading order number (`003-naahh.gif` is
    "naahh"), the same rule the server applies. Pasted images are called
    "pasted image" (then "pasted image 2", …) because browsers name every
    pasted image `image.png`.
  - **Keywords**, **Emojis**, **Vibe** and **Tags**, as in the item editor
    (keywords and tags separated by commas, emojis by spaces; the vibe field
    suggests the vocabulary; Emojis has the [emoji
    picker](#keyboard-and-mouse)). The hints say what each is for:
    keywords describe what is in the sticker and go to Telegram as its search
    words, the vibe is the mood in one word, and tags are your own labels for
    sorting and filtering (`tag:nsfw`, `-tag:nsfw`) that never go to
    Telegram and that AI never fills. What you type or approve is the item's
    from the start, so AI enrichment fills only the fields left empty (and
    unapproved suggestions, below). "Use for every file" gives
    the other files the selected file's keywords, emojis, vibe and tags (not
    its name). Enter in these fields does not upload. The details are sent
    in a `meta` field, the same JSON as the API's (one `UploadMeta` per
    file).
  - **AI suggestions** (when the server's AI is ready or still loading; the
    form then carries `data-ai-suggest`): as soon as files are staged, the
    tray asks the server for keywords, emojis and a vibe for each one, two
    files at a time, the selected file first
    ([ai.md](ai.md#suggestions-before-an-upload)).
    It sends a small PNG rendered in the browser, not the file. While it
    waits, the selected file shows "Suggesting keywords, emojis and vibe…".
    Suggestions fill only fields that are still empty, never what you typed,
    and are marked like on the item page: the `AI` badge next to the label,
    a tinted field, and an `AI` badge on the thumbnail while the file has
    any left to review. A line above the fields says what AI suggested.
    Suggestions are never made for tags or names.
    - **✓ Approve** next to a field makes the value yours. It is a toggle
      (`aria-pressed`), so pressing it again marks the value AI again.
    - **Editing** a suggested field, including picking an emoji or a vibe
      from the list, makes the value yours.
    - **✓ Approve all** (above the fields) approves the selected file's
      suggestions; **✓ Approve AI for all files** (next to "Remove all",
      with several files) approves every file's.
    - **Use for every file** counts as approval: the copied values are
      yours on every file, the selected file's suggestions included, and
      they replace the other files' own suggestions in those fields.
    - Unapproved suggestions are uploaded too, marked AI (`ai_fields` in
      `meta`), so the item page shows them with "✓ Confirm".
    - Failures are quiet: if the server has no AI, the browser cannot read
      a file, or a request fails, the fields stay as they are. When the
      server asks to wait (rate limit, model still loading) the tray waits
      and tries again a few times.
    - Suggestions look at the file as it was added, before crops and turns.
      Clearing a suggested field leaves it empty, and empty fields are filled
      by enrichment after the upload, as before.
    - Screen readers: each field's description says "Suggested by AI, not
      approved." while it is; the ✓ buttons are named "Approve keywords"
      (and so on), "Approve all suggestions for <file>"; the thumbnail
      says "AI suggestions to approve"; the line above the fields is a
      polite live region.
  - **Image tools** for still images (PNG, JPEG, still WebP): rotate left or
    right, flip horizontally or vertically, and crop. The crop area covers
    the whole image at first; drag its corners to crop and its inside to
    move it, or focus it and use the arrow keys (move) and Shift+arrow keys
    (resize). The image is shown as large as the width and 70% of the window
    height allow (small images up to 3×). "Free", "Square" and the set's recommended aspect ratio (when
    it has one other than 1:1) set the crop's shape. "Reset" goes back to
    the original. The line under the image says what will be uploaded.
  - An edited image is rendered in the browser when you press Upload and
    sent as a lossless PNG of the result. Files without image edits are sent
    as their original bytes, so a JPEG is not re-encoded unless you edit it.
  - GIFs, animated WebP and videos get details but no image tools: the
    browser's canvas keeps just the first frame of an animation.

  Without JavaScript the form posts the chosen files as they are.
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
| Users | Every user with role, status (disabled, TOTP, no password), tailnet login, last sign-in, sets/items and storage. "Manage" per user: rename, set the role, disable or enable, reset the password, reset TOTP (lost device), bind or unbind a tailnet login, delete (with a confirmation page listing what is deleted). Plus "Create a user". The server refuses to demote, disable or delete the last active admin and to delete yourself; the message is shown on the page. |
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

`/static/app.css`, `/static/app.js`, `/static/prefs.js`,
`/static/upload.js`, `/static/emoji.js`, `/static/emoji.json`,
`/static/htmx.min.js` and `/static/favicon.svg` are compiled into the binary and served with
`Cache-Control: public, max-age=31536000, immutable`. Pages link them with a
content hash (`?v=…`), so upgrades are picked up at once. emoji.js gets the
versioned URL of `emoji.json` from its `<script data-emoji-data>` attribute.

**Emoji data.** `assets/emoji.json` (about 110 kB, 35 kB gzipped; 1,902
emojis) is generated by `scripts/gen-emoji-data.py` from Unicode
`emoji-test.txt` 17.0 (order, categories, names; fully-qualified emojis only,
without the Component group and skin-tone and hair-style variants) and the
English CLDR 48.2 annotations (keywords), plus the slang in
`crates/stickpile-search/src/emoji_names.rs`, kept apart from the CLDR
keywords: the picker ranks those curated terms first (so "laugh" and "lol"
find 😂 before 😀), then words of the name, then keywords; ties go to the
order of `emoji_names.rs` (its emojis are also the `common` list, the
suggestions before anything was used). The Unicode data is
© Unicode, Inc., under the [Unicode License
v3](https://www.unicode.org/license.txt); the file carries that notice and
the versions. To regenerate it (for example after updating `flake.lock`, or
after changing `emoji_names.rs`), run `python3 scripts/gen-emoji-data.py`:
it builds `nixpkgs#unicode-emoji` and `nixpkgs#cldr-annotations` from the
flake's nixpkgs (or takes `--emoji-test` and `--cldr` paths) and rewrites
the file, one emoji per line.

## Tests

`crates/stickpile-server/tests/web_auth.rs`, `web_pages.rs` and
`web_flows.rs` (helpers in `tests/web_common/`) cover: every page renders for
its user; anonymous redirects (401 + `HX-Redirect` for htmx); 404 for content
you cannot see; 403 for admin pages and others' sets; CSRF on urlencoded,
htmx and multipart forms, and cross-site posts; the login flow and cookie
flags; logout; the TOTP step; registration per policy; tailnet sign-in;
htmx fragments without the document shell; escaping of `<script>` and quotes;
the set page's "View" control (in the filter form, `?view=compact` and
`?view=grid`, other parameters kept, the JS-only toggles hidden), the set
lists' control and card parts, themes (`<html data-theme>` per account,
none for System, the top bar's switch, Settings → Appearance and
`POST /settings/theme` with its validation and CSRF) and the context-menu data per permission (no edit, delete
or move entries for viewers, no move entries when sorted or filtered, the
named "⋯" button);
the CSP header and the absence of inline styles and scripts; static asset
types; the emoji picker's markers (the emojis inputs, the script order, the
`:` suggestions' combobox, the `emoji.json` notice and contents, its terms
column and `common` list); HTML error pages; and the flows end to end (sets, uploads with
near-duplicate warnings, imports, editing, likes and filters, stats and
popular, settings, the admin panel, git pack and Telegram forms, job polling).

`tests/ai_admin.rs` covers the tray's AI side on the server: the
`data-ai-suggest` attribute (present when AI is ready or loading, absent when
off), the suggestion endpoint with a web session (CSRF header, same origin),
and uploads with `ai_fields`. The tray itself (marking, approving, editing,
"Approve all", what gets uploaded) was checked with Playwright in Chromium,
Firefox and WebKit against a server without models: the test intercepts the
suggestion endpoint in the page and adds the attribute.
