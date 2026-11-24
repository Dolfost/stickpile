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
| `/` | Search box, items you copied recently (the set page's cards, read-only, each with its set, and a "View" control of their own), your sets with covers (with the set lists' "View" control). |
| `/sets` | Tabs **Mine** and **Public**. Unlisted sets are never listed; they are reachable only by their link. Links to "Add a git pack" and "Import a Telegram pack" appear when those integrations are enabled. Each card shows the cover, name, description, item count, visibility and source labels, owner and slug; the "View" control arranges them (see [Display options and themes](#display-options-and-themes)). |
| `/sets/new` | Name, slug (derived from the name when empty), description, visibility, recommended aspect ratio (`W:H`) and size in pixels. |
| `/sets/{id}` | Set header (the cover image when the set has one, owner, visibility, version, git binding), actions (settings, "Edit pack" for contributors who may, "Propose…" on others' packs, Export (the [export page](#export-and-publish)), popular in this set, "Recheck this set" for the owner ([Rechecks](#rechecks)), pull now, publish to Telegram), the upload form (for the owner and contributors who may add), "Replace from a set directory", the filter "hide below N likes" with sorting (set order, likes, copies, quality, recent) and the "View" control (style and shown fields), and the item editor grid (see [Set page](#set-page)). |
| `/collaboration` | Proposals waiting for you (with Accept and Decline and a note), the ones you sent (Withdraw), the packs you contribute to, and the decided proposals. The nav link shows how many wait for you. See [collaboration.md](collaboration.md). |
| `/proposals/{id}` | One proposal: the stickers with thumbnails, or a table of the current and the proposed values, with the same buttons. |
| `/sets/{id}/propose` | On someone else's pack: propose new stickers, a new name, description or cover, or a cover image. |
| `/items/{id}/suggest` | On someone else's sticker: suggest a new name, keywords, emojis, tags, vibe or text (the words in the picture). |
| `/sets/{id}/export` | Downloads (ZIP, TAR, TAR.GZ), the WhatsApp sticker packs, and for the owner publishing to one or several platforms at once. See [Export and publish](#export-and-publish). |
| `/sets/{id}/settings` | The set form, with the **cover** chosen from the set's thumbnails ("First item" by default) or its own cover image, then the **Cover image** form (upload, crop, replace, remove; see [Set covers](#set-covers)), plus "Delete this set…". Read-only git packs only offer deletion. A contributor who may edit the pack gets an "Edit pack" page with the name, description and cover. The set page itself lists the **Contributors** for the owner (permissions to tick, Remove, and "Add a contributor") and for contributors (with Leave). |
| `/items/{id}` | Large preview (`<img>` for stills and animations, `<video controls loop muted>` for videos), copy and download, like, metadata with inline editing (the text, the words in the picture, shown with its line breaks), "Re-run AI" (when AI is ready), "Move to" another of your sets, delete, the apps that cannot take the item, the quality report table, near-duplicates in your library, "Find a better version", and every variant with a download link. |
| `/upload` | Upload files to one of your sets; import a set folder or an export `.tar` as a new set. |
| `/search?q=` | The same hybrid search as `GET /api/v1/search`, a collapsible syntax cheat-sheet (`text:"no thoughts"` or `x:` finds the words in the picture, which plain words match too; typos and words inside longer words count, and the last may be just its start: `x:l` finds "lol"), and whether semantic search is on. Results update while you type (htmx) and "More results" appends the next page. |
| `/popular` | Most copied, most liked and trending side by side, never blended. Scope: whole server, your items, or one set (`?scope=set&set=…`, linked from the set page). The window (day, week, month, all) applies to "most copied". |
| `/stats` | Your totals, a bar chart of copies per day for 30 days (inline SVG with a hover title per bar and a screen-reader table), your most copied items and the apps you paste into. |
| `/problems` | Problems in all your sets, grouped by set, with filters by severity and kind; admins also get "Whole server". The set page has the same list for one set at its bottom (`/sets/{id}/problems` without JavaScript). See [Problems](#problems). |
| `/settings` | Username (shown as the owner of your sets and matched by `by:`; renaming keeps you signed in), display name and Telegram user id, the theme (System, Light, Dark), password change (current password required if you have one, except when you are signed in through the tailnet; at least `auth.min_password_length` characters), two-step verification setup (QR code and key, confirm with a code) and disabling, **Recheck my stickers** ([Rechecks](#rechecks)), **Add a device** (a one-time pairing code for five minutes, shown as a QR code for the phone app and as text with the server's address for typing in or `stickpile login --code`; see [api.md](api.md#pairing)), API tokens (create, the secret is shown once, revoke). |
| `/rechecks/{id}` | One recheck: what it covers, its progress (updating itself), Cancel while it runs, the result and the pictures that failed. Its owner, or an admin. See [Rechecks](#rechecks). |
| `/admin` | Admins only. See [Admin panel](#admin-panel). |
| `/jobs/{id}` | Status of a background job (yours, or any job for admins). |

Git packs (`/gitpacks/new`) and Telegram (`/telegram/import`,
`/sets/{id}/telegram`) are described in [Integrations](#integrations), the
export page in [Export and publish](#export-and-publish).

### Tailnet users

When stickpile is opened through `tailscale serve`, the tailnet identity signs
you in automatically. The top bar shows your name with a `tailnet` badge and
no "Sign out" button (there is no session to end). `/login` redirects to the
start page.

## Set page

The item editor grid shows one card per item: the thumbnail with its kind
and quality badges, then the fields in a dense layout. The name is the
card's title; keywords, vibe, tags and text have small uppercase labels;
keywords and tags are chips (tags with `#`); emojis and the vibe share a
line when both fit; the text (the words in the picture) keeps its line
breaks and shows at most three lines on the card (all of it on the item
page and in its edit form). A pencil after each field edits it in place
(see below), "✓" confirms an AI suggestion. Below the fields one row of small buttons:
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
(thumbnails only, without "⋯": right-click a thumbnail, or focus it and
press the context-menu key or Shift+F10, for the menu), and which fields
show: Emojis, Keywords, Vibe, Tags, Text (the words in the picture) and
Labels (the kind, quality and "Cover" badges). See [Display options and
themes](#display-options-and-themes).

**Context menu.** Right-click an item card (either view) for its actions:

| Entry | Shown when | Does |
|---|---|---|
| Copy image (stills) / Copy still frame (animations, videos), with "Ctrl+C" | always | copies the item, as Ctrl+C on the card does ([Copy to clipboard](#copy-to-clipboard)) |
| Copy link | animations and videos | copies the file's address |
| Like / Unlike | always | the card's like button |
| Open item page | always | link to `/items/{id}` (middle-click opens a tab) |
| Download original | always | download link |
| Rename | you can edit the set | inline edit of the name (also in the compact and grid views, where the form opens inside the small card) |
| Edit all fields… | you can edit the set | `/items/{id}?edit=1` |
| Use as cover / Stop using as cover | you can edit the set | makes the item the set's cover (or goes back to the first item) through `POST /sets/{id}/cover`; the grid comes back in the page's sort with the "Cover" badge on that card. When the set has a cover image, it asks first: the item replaces the image |
| Move up, Move down, Move to top, Move to bottom | you can edit the set, set order, no likes filter | the set's move endpoint; the ends are disabled for the first and last item |
| Delete… | you can edit the set | asks for confirmation, then removes the card |

The read-only cards elsewhere (search results, the start page's "Recently
copied", "Better versions") have the same menu with the copy entries, Open
item page and Download original, and the same "⋯" button at the end of the
name line.

The menu is built by app.js from `data-*` attributes the server puts on the
card only when the action is allowed (`data-edit-url`, `data-delete-url`,
`data-move-url` with `data-first`/`data-last`, `data-cover-url` with
`data-cover` on the current cover, `data-cover-image` on the grid when the
set has a cover image), and it uses the same
endpoints, htmx requests and CSRF header as the buttons. It also opens from
the keyboard (the context-menu key or Shift+F10 while focus is in a card)
and from the "⋯" button on each card (for touch screens and keyboards;
not in the Grid style, where only the thumbnail shows: there a long press
opens it on touch screens that send a context-menu event, such as Android,
and the thumbnail link is the keyboard's way in).
Inside the menu: ↑/↓ move between entries (wrapping), Home/End jump,
typing a letter jumps to the entry starting with it, Enter or Space
activates, Escape or Tab closes and returns focus to where it was. A click
outside, scrolling or resizing closes it. After a move from the keyboard,
focus follows the item. Ctrl+ or Shift+right-click (and right-clicking a
text field, e.g. while editing) keeps the browser's own menu. Without
JavaScript there is no custom menu and no "⋯"; the browser's menu and the
card buttons work as before.

## Set covers

The cover shown for a set (set lists, the start page, the set page's
header, the Telegram pack's thumbnail) is, in this order: the set's own
**cover image**, the item chosen as the cover, the first item. The cover is
what you chose last: choosing an item (in the settings or with "Use as
cover") removes the cover image, and uploading an image keeps the chosen
item, which is the cover again once the image is removed.

**Set settings.** The set form's "Cover" chooser has a tile per item and
"First item"; with a cover image, a "Cover image" tile comes first and is
selected, and the hint says that saving another choice removes the image.
Without one, a link leads to the **Cover image** section below the form:

- the current image (thumbnail, size and format) with **Remove image** and a
  note on what the cover becomes then (the chosen item, else the first);
- the upload form, **Upload image** (or **Replace image**): a still PNG,
  JPEG or WebP. GIFs, animations and videos are refused with "A cover image
  must be a still PNG, JPEG or WebP image, not …", shown in the section.

Both are plain forms (`POST /sets/{id}/cover/image`, multipart with the
CSRF token first, and `POST /sets/{id}/cover/image/delete`) that come back
to `/sets/{id}/settings#cover-image` with a message. With JavaScript,
upload.js gives the upload form its tray in a cover mode
(`data-tray="cover"`): one image at a time (a new one, chosen, dropped or
pasted, replaces it), animations, animated WebP and videos are skipped with
a message, no details to fill in, and the crop starts as the largest centred
square, with rotate, flip, "Free" and "Reset" as for uploads. The cropped
image is sent as a PNG. Without JavaScript the file is sent as it is, and
an image that is not square is fitted into the Telegram thumbnail's square
with transparent bands.

**Elsewhere.** Set cards show the cover image instead of an item; the set
page shows it before the set's name (`#set-cover`), and no item card has
the "Cover" badge then. "Use as cover" from an item's menu answers htmx
with the grid and an out-of-band `#set-cover`, so the header loses the
image at once. The image is not an item: it never appears in the item grid,
search or counts.

<!-- ---- display options and themes (begin) ---- -->

## Display options and themes

**"View" on the set page.** A disclosure button in the filter form. Its
panel has the style (Regular, Compact, Grid) and the "Show" toggles for
Emojis, Keywords, Vibe, Tags, Text and Labels; the name always shows (the
toggles start on; Text is a field like the others, below the thumbnail). Each
option is one tile: a press anywhere on it (the dot or box, the name, the
hint, the padding) selects it, and the panel stays open for more changes,
as with arrow keys and Space. The focus then sits on the chosen option.
Tiles are at least 31 px tall, 44 px on touch screens (`pointer: coarse`).
Toggles a style has no room for are disabled with a note (Compact shows the name and
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

**"View" on the start page** (Recently copied). The set page's styles
(Regular, Compact, Grid) and toggles, plus Set (the item's set, a line of
the regular card); the cards are the set page's, read-only. Remembered
apart from the set pages (`stickpile.home.view`, `stickpile.home.hide`),
as classes on `#recent-section`. It needs JavaScript (no URL parameter),
so it is hidden without.

**Before the first paint.** prefs.js is loaded without `defer`: once in
`<head>`, and once more as the first child of `#items-section`,
`#recent-section` and each `.set-list`, where it applies the remembered classes to its parent before
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
dark one) sits left of the account's name, a divider away from "Sign
out" (on narrow screens it moves up to the brand's line). It is a toggle
button named "Dark theme" whose `aria-pressed` says
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
- **Search suggestions**: while a filter is typed in a search field (the
  search page's and the top bar's), a list under it offers its values, as
  the desktop popups do ([desktop.md](desktop.md#search-suggestions)): the
  vibes, tags, sets and owners of the items you can search, most used
  first, the words of `type:`, `is:` and `sort:`, and for `e:` emojis with
  their English names (`e:` alone: the emojis of your own items, most
  used first; `e:sk`: 💀 skull, ☠️ skull and crossbones, ..., those your
  items use first, then others), and for `x:` (or `text:`, `-x:`) the
  words written in the pictures of the items you can search (`x:` alone:
  those on the most items; `x:l`: lol, love, ...; then the words a value
  is inside and those a typo away), shown as the index keeps them: lower
  case, without accents or apostrophes (`x:п'ят` offers `пятниця`). A
  word only someone else's private sticker has is never offered. The
  server matches
  and ranks them (`GET /api/v1/search/suggest`, the popups' Rust code);
  suggest.js only asks, 120 ms after typing pauses, and draws the answer
  in the `:` suggestions' list. The highlight starts on the value a
  half-typed filter stands for (`vibe:sar`: sarcastic), none for an emoji
  name, which the search matches itself, and for `x:` only on a word typed
  whole: the search takes a `text:` value as typed (`x:lo` finds lol and
  love). Up/Down move it; Tab, or Enter on
  a highlighted value, puts it in place of what is typed (an emoji in
  place of its name) with a space, and the results follow; a click does
  the same. Enter without a highlight, or on the value already typed,
  searches as before. Escape, a space, moving the caret to another word
  and leaving the field close the list. The field is an ARIA combobox as
  the Emojis field is. Without JavaScript the search fields work as
  before.
- **Inline editing** (set page and item page): activate "Edit" next to a field
  (the pencil on the set page).
  The field turns into a small form with the cursor in it. Enter saves,
  Escape (or "Cancel") restores the view. Lists are comma-separated (keywords,
  tags), emojis space-separated, an empty vibe clears it. The text is a
  multi-line field (Enter is a line break there, "Save" saves; at most
  2000 characters): saved, it is yours, and saved empty it stays empty,
  because the server does not read a text a person cleared again.
  Invalid input stays
  in the form with the error next to it. Without JavaScript "Edit" opens the
  item page's full form (`?edit=1`), which saves only the fields you changed
  (an AI text sent back unchanged, line endings aside, stays the AI's).
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
- **Context menu** on item cards: right click, the context-menu key,
  Shift+F10 or "⋯". See [Set page](#set-page).
- **Copy**: Ctrl+C (⌘C) on a focused card, or on an item page when nothing
  in particular has focus, copies the sticker; the quick copy icon appears
  on the thumbnail under the pointer. See [Copy to clipboard](#copy-to-clipboard).
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

Every item card (the set page in all three styles, search results, the
start page's "Recently copied", "Better versions") and the item page can
put the sticker on the clipboard, ready to paste into a chat:

| Where | How |
|---|---|
| Card | **Copy** (**Copy frame** for animations and videos) in the card's buttons (Regular style). |
| Card, fast | The **quick copy** icon on the thumbnail's bottom-left corner, shown only while the pointer is over the card or focus is in it, so the Grid style's thumbnails stay clean ("⋯" has the other corner there). It is a mouse shortcut: out of the tab order and hidden from screen readers, which have the Copy button and the menu. Touch screens (no hover, a coarse pointer) do without it. |
| Card, keyboard | **Ctrl+C** (**⌘C** on a Mac) while focus is anywhere in the card (its thumbnail, name, buttons), unless text is selected (then it copies the text as usual) or focus is in a text field. Holding the keys copies once. |
| Card menu | **Copy image** / **Copy still frame** (with the shortcut shown), and **Copy link** for animations and videos: right click, the context-menu key, Shift+F10 or "⋯" (see [Set page](#set-page)). |
| Item page | **Copy image** / **Copy still frame**, **Copy link** for animations and videos, **Download**; Ctrl+C when nothing in particular has focus (e.g. after clicking the picture). |

**What is copied.** Browsers write only `image/png` (besides text) to the
system clipboard, so every copy is one PNG, the same as the desktop
client's `image/png` offer for "Other / browser" ([clipboard.md](clipboard.md)):

| Media | Copied | From |
|---|---|---|
| Still (PNG, JPEG, WebP) | the picture as a PNG | The original if it is a PNG. Otherwise the `png` variant, the server's lossless rendering of the original, used as it is; until that variant is generated, the original, converted to PNG on a canvas in the browser. |
| Animation (GIF, animated WebP) | a **still frame** as a PNG: the first frame, the one the thumbnail shows, at full size | Decoded from the original in the browser. |
| Video (MP4, WebM) | a **still frame** as a PNG: the frame about 10 % in, the one the thumbnail shows, at full size | Decoded from the original with a `<video>` element. |

If the browser cannot decode the original (a video codec it lacks), the
frame comes from the thumbnail (the same frame, at most 256 px), and the
message says so.

**Why only a still for GIFs and videos?** The Clipboard API rejects
`image/gif` and video types, and drawing an animation on a canvas keeps one
frame. Chat apps need the file itself (`text/uri-list` or the GIF bytes) to
keep the motion, which only a native program can offer. So the label says
"frame", the confirmation says "still frame … Download keeps the
animation", and the item page explains it: to send the animation, download
it and drag the file into the chat, or use the desktop client, which
pastes GIFs and videos directly. **Copy link** copies the file's address
(it opens for people signed in to this server; share links are a post-v1
feature).

**How.** app.js creates the `ClipboardItem` synchronously inside the click
or key press, with a promise of the PNG, then fetches and converts: Safari
requires that, and the permission to write cannot expire while the file
loads. Engines that refuse a promise get the finished blob instead.

**Feedback.** The button shows "Copied ✓" (the quick copy icon a check
mark) for a moment, and a message such as "Copied “cat”. Paste it into your
chat." appears at the bottom in a polite live region (`#toast`,
`role="status"`). Failures go to an alert region (`#toast-alert`,
`role="alert"`) with the reason, e.g. "Could not copy “cat”: the download
failed (HTTP 500). Try again, or use Download." Both regions exist from the
page load, so screen readers announce the first message too.

**Secure context.** The Clipboard API exists only on `https://` pages and
on `localhost`. On plain `http://` (for example a tailnet name without
`tailscale serve`) a copy opens a dialog, "Copying images needs HTTPS",
that says why and offers **Open image** (in a new tab, where the browser's
own "Copy Image" works) and **Download**; the item page shows the same
note next to its buttons up front. A browser without `ClipboardItem` or
without `image/png` support gets the same dialog, titled "This browser
cannot copy images". Copy link falls back to `document.execCommand("copy")`
there. `tailscale serve` provides HTTPS on the tailnet.

**Usage statistics.** After a successful image copy the page reports one
usage event, like the desktop client: `POST /api/v1/usage` with the
session cookie and `X-CSRF-Token` (tailnet users: the same-origin check),
a random event id, `client: "web"`, `target: "generic"` (the profile whose
PNG it copied, as the CLI reports without `--target`) and the `variant`
the PNG was made from (`original`, `png`, or `thumb` for the fallback
frame). It counts for "Recently copied", popular items and your stats
("Where you paste" lists it as "Other apps"). A failed or refused copy,
Copy link and downloads are not counted.

**Without JavaScript** the copy buttons stay hidden (the stylesheet
enforces the `hidden` attribute); "Download" always works.

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
  - **Keywords**, **Emojis**, **Vibe**, **Tags** and **Text**, as in the item editor
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
    its name and its text, which belong to one picture). Enter in these
    fields does not upload (in Text it starts a new line). The details are
    sent in a `meta` field, the same JSON as the API's (one `UploadMeta` per
    file).
  - **Text**: the words written in the picture, several lines, at most 2000
    characters. A text you typed or approved is yours (`text` in `meta`);
    left empty and never touched it is not sent, so the server reads the
    picture after the upload (when it has a text reader); a text you
    emptied is sent as `""`, which keeps the item without a text (the
    server does not read it then).
  - **AI suggestions** (when the server's AI is ready or still loading; the
    form then carries `data-ai-suggest`): as soon as files are staged, the
    tray asks the server for keywords, emojis and a vibe for each one, two
    files at a time, the selected file first
    ([ai.md](ai.md#suggestions-before-an-upload)).
    It sends a small PNG rendered in the browser, not the file (256 px on
    the longest side). When the server reads the words in pictures (the
    form's `data-ai-text`: AI is ready or loading and the server has a text
    reader with `ai.ocr` on), the answer also has the text, and the PNG is
    larger, 1024 px on the longest side (its own size when it is smaller),
    made smaller by quarters while it is over the endpoint's 2 MiB: 256 px
    is too small to read words, and the reader, measured on pictures
    scaled down first, read about as well at 1024 px as at full size
    (normalised character error rate 4.1 % against 3.8 %; 512 px: 5.7 %,
    256 px: 31.9 %), while the PNG stays quick to encode and send. An empty text in the answer is no
    suggestion. While it
    waits, the selected file shows "Suggesting keywords, emojis and vibe…"
    ("... and reading the text…" with a text reader).
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
      they replace the other files' own suggestions in those fields. It
      leaves the texts alone, suggested or not.
    - Unapproved suggestions are uploaded too, marked AI (`ai_fields` in
      `meta`), so the item page shows them with "✓ Confirm".
    - **What AI is doing** shows on the line above the fields, for the
      selected file, and as a spinner on each thumbnail whose suggestions
      are on their way: "Suggesting…"; waiting while the server loads its
      model (503 `ai_loading`: the tray asks again after `Retry-After`, for
      up to three minutes) or after "too many requests" (429, three tries
      per file); a short note when it failed (the server could not be
      reached or failed, too many requests, the browser cannot read the
      file) with **Try again**; "AI had no confident suggestions"; or, from
      the form's `data-ai` (`ready`, `loading`, `off`, `failed`) or a 503
      `unavailable`, "AI suggestions are off on this server" (or "not
      available: the server could not load its AI model"). The fields stay
      as they are in every one of these cases.
    - A field you typed in is never filled, also when the answer arrives
      after you typed (or after you cleared it again), and "Use for every
      file" protects the copied fields the same way.
    - Suggestions look at the file as it was added, before crops and turns.
      Clearing a suggested field leaves it empty, and empty fields are filled
      by enrichment after the upload, as before.
    - Screen readers: each field's description says "Suggested by AI, not
      approved." while it is; the ✓ buttons are named "Approve keywords"
      (and so on, "Approve text"), "Approve all suggestions for <file>"; the thumbnail
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
- **Upload with nothing chosen** sends nothing: the drop area turns red and
  a line under it says to choose, drop or paste files (in place of the
  browser's bubble on the file input). A request without files (a browser
  that skips the form's check) gets 400 with the message where the results
  appear (htmx), or the upload form again with it above (without
  JavaScript), not an error page.
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
| Rechecks | **Recheck every sticker** on the server (or of one set, by its id) and every server-wide recheck, the server's own included ("the server" started them, after a change of the AI model or for pictures its text reader never read). See [Rechecks](#rechecks). |
| Audit log | Newest first, 50 per page ("Older entries"): time, actor, action, target, IP, details. |

Every admin action goes through `service::admin` and is written to the audit
log; a server-wide recheck through `service::recheck`, which writes
`recheck.start` and `recheck.cancel`.

## Rechecks

A recheck lets the computer look at stickers again in the background
([ai.md](ai.md#recheck), [api.md](api.md#rechecks)).

**Recheck my stickers** (Settings, `#recheck`) covers the stickers of your
own sets, or of one of them. "Recheck this set" on a set page (its owner)
opens the same form with the set chosen (`/settings?recheck_set=<id>`).
The form ticks every part this server can recompute now: keywords, emojis,
vibe and search by meaning with a loaded AI model, the text in the picture
with a text reader, the quality score and the renditions always. A line
says why a part is missing (AI off, the model still loading, no text
reader). The mode is "Only what is missing" (the default) or "Everything
again", each with a line on what it does; neither changes what you wrote
or confirmed. Errors come back in the form with what was sent: nothing
ticked (400), a set of someone else (403), too many rechecks
(`limits.recheck_per_hour`, 429 with `Retry-After`), a part the server
cannot do now (503). Starting one opens its page; asking for one that is
running opens that one. Your five latest rechecks are listed below the
form.

**A recheck's page** (`/rechecks/{id}`) says what it covers, which parts,
how, who started it and when, and shows a progress box: a bar and the
counts of pictures, checked, changed and failed. While it runs the box
asks for itself every 2 s with htmx (a `<noscript>` meta refresh every 3 s
without JavaScript) and offers "Cancel the recheck" (a plain form, or htmx
replacing the box). When it is over the box says how it ended, that new
keywords, emojis, vibes and texts are AI suggestions with the AI mark and
Confirm on each sticker, and lists the failed pictures with their errors:
a link to your sticker for your rechecks, the picture's id for
server-wide ones.

**Recheck every sticker** (admin panel, Rechecks tab) is the same form for
every sticker on the server, with an optional set id; its page links back
to the tab.

## Integrations

Shown only when enabled on the server.

- **Git packs** (`git.enabled`): "Add a git pack" asks for the repository URL,
  branch, folder, mode (read-only, or push edits back) and slug. After
  creating it the page shows the webhook URL and secret **once**, plus the
  status of the first pull. Git sets show their binding (URL, branch, last
  pull, last error) and "Pull now"; read-only packs show "Edit upstream" and no
  edit controls.
- **Telegram** (`telegram.enabled` and a bot token): "Import a Telegram pack"
  (pack name or `t.me/addstickers/...` link) creates a set in the background;
  "Publish to Telegram" (or "Update Telegram pack") on your sets leads to the
  publish section of the [export page](#export-and-publish), which creates or
  updates the pack. Packs belong to your Telegram user id (Settings) or the
  server's default owner. The older one-platform form at
  `/sets/{id}/telegram` still works.
- **Discord** (`discord.enabled` and a bot token): "Import from Discord"
  (`/discord/import`: the server id, stickers and/or emoji, slug,
  visibility) creates a set in the background; publishing a set to a
  server is a row of the [export page](#export-and-publish). See
  [discord.md](discord.md#in-the-server).
- **Signal** (`signal.enabled`): "Import a Signal pack" (`/signal/import`:
  the pack's link, slug, visibility) creates a set in the background; the
  link is not kept and a refused one is not shown back. Publishing needs
  `signal.credentials_file` too and is a row of the export page. See
  [signal.md](signal.md#in-the-server).

The import pages are linked from the sets list and from the upload page's
"More sources". On a set page, "Publish to Telegram" / "Update Telegram
pack" leads to the export page's Publish section; without Telegram but
with Discord or Signal set up, the button is "Publish".

Background work (git pulls, Telegram, "Re-run AI") shows a status box that
polls itself every 2 seconds with htmx (`hx-trigger="every 2s"`) until the job
is done or failed, then links to the result. Without JavaScript the job page
refreshes itself while the job runs.

## Export and publish

`/sets/{id}/export` (code: `src/web/export.rs`), linked as "Export" from
every set page. Three sections:

- **Download**, for anyone who can read the set: the set's files and its
  `set.toml` as **ZIP**, **TAR** or **TAR.GZ** (the API's `export.zip`,
  `export.tar` and `export.tar.gz`; every format holds the same files, see
  [set-format.md](set-format.md)). **RAR is not offered**: writing RAR
  archives needs the proprietary `rar` program, which a free server cannot
  ship, and everything that opens RAR opens ZIP too. Below them, the
  **Signal-ready zip** (`signal.zip`): every item as a 512x512 Signal
  sticker with `emoji.txt` and a `manifest.json` for signal-cli, to make a
  pack by hand in Signal Desktop (File, Create/upload sticker pack). It is
  offered whether or not the server publishes to Signal; when it does not,
  the text says this is the way to make a Signal pack. It is made when it
  is downloaded (a large animated set takes a while) and items that cannot
  become Signal stickers are listed in its `left-out.txt`; a set of more
  than 200 stickers gets a folder per pack
  ([signal.md](signal.md#signal-ready-zip)).
- **WhatsApp sticker packs**, for anyone who can read the set too: "Build
  WhatsApp packs" queues a job (htmx shows its status box; without
  JavaScript the page refreshes itself while it runs). The last build is
  listed: each pack's `.wastickers` file with its title, kind (still or
  animated), sticker count and size, "Download all packs" (one zip), the
  stickers that were changed to fit (an animation sent as a still, a long
  video cut to 10 s) and the items that were left out, each with the
  reason. When the set changed since, a note says so and the button builds
  them again. See [whatsapp.md](whatsapp.md).
- **Publish**, for the owner only: a table with one row per platform the
  server is configured for: Telegram, Discord, Signal. Each row shows where
  the set is published (the Telegram pack, the Discord server, the newest
  Signal pack, each with its link), when it was last published with a
  summary of that result (Telegram and Discord: stickers or emoji, added,
  removed, left out; Signal: stickers in the new pack, left out) and, folded,
  the items that were left out or changed to fit with the reason, the last
  error if the last publish failed, a running publish's status box, the
  platform's options and a note. Options: Telegram's pack short name and
  title (both optional); Discord's server id (required the first time) and
  "Publish as" stickers or emoji, prefilled with what the set was published
  with last; Signal has none. Notes: Telegram when no Telegram user id and
  no default owner are set; Discord that the bot must be in the server with
  the Create Expressions permission and only what stickpile created there
  is changed; Signal that packs cannot be changed, so every publish
  uploads new packs with new links, and that a set of more than 200
  stickers becomes several packs.
  A set published as several packs (more stickers than one pack holds;
  `Publication.parts`, for any platform) lists every pack in its row
  instead, each with its link and sticker count, and the note "Published
  as N packs."; Telegram adds how the packs are kept (120 stickers each, a
  published sticker stays in its pack, new ones go to the first pack with
  room, a pack left empty is deleted) and **Repack the parts**. That link
  opens a confirmation (with htmx in a box under the table, without
  JavaScript as a page of its own, `/sets/{id}/export/telegram/repack`)
  that says what a repack does, how many published stickers move to
  another pack, how many stickers each pack would hold and which packs
  would be deleted, from `GET /api/v1/sets/{id}/telegram/plan?repack=true`;
  nothing changes until "Repack now", which queues the Telegram publish
  with `repack` (never with a pack name, so it repacks the packs the plan
  showed; the publish form has no repack field) and comes back with its
  status box. The set page's
  "Telegram:" line links every pack, and its "Publish to Telegram" /
  "Update Telegram pack" button is unchanged.
  Tick several rows and press "Publish to the selected platforms" to queue
  one job per platform, or press a row's own "Publish to X only". The page
  comes back with the queued jobs' status boxes. Errors (no platform
  ticked, bad options) are shown above the table with what was typed kept;
  nothing is queued then. A platform is ticked by default when the set is
  already published there, or when it is the only one.

The publish state comes from `GET /api/v1/sets/{id}/publish` data
(`service::publish`, the platform registry; see [api.md](api.md#publishing)).

<!-- ---- problems (begin) ---- -->

## Problems

What is wrong, or worth a look, in a set: duplicates, things that keep it
from working on Telegram, broken media and loose metadata. The same report
is in the API ([api.md](api.md#problems)); code in
`src/service/problems.rs` (loading and permissions),
`src/service/problems/detect.rs` (the checks) and `src/web/problems.rs`.

**On the set page.** Owners find a "Problems" section at the bottom of
their sets (others never see it). The page itself does not wait for it: the
list is fetched with htmx right after the page has loaded, and again a
moment after anything in the item grid or the upload form changed, so
fixing an emoji in place or deleting a copy updates it. First a count
("2 warnings · 5 info", or "No problems found"), then one card per problem,
errors first. Without JavaScript the section links to
`/sets/{id}/problems`, the same list as a page.

**The Problems tab** (`/problems`, "Problems" in the top bar) lists the
problems of every set you own, grouped by set, sets with errors first.
Filters: "Show" (everything, errors and warnings, errors only), the kind,
and "Show dismissed"; "Apply" submits them as a plain GET form. Admins get
two tabs, "My sets" and "Whole server" (`?scope=server`): every set on the
server with its owner. For other users' private sets the set name is not a
link and items show as "Private item" without names or thumbnails (admins
still cannot open those sets or files); only owners can dismiss or fix
anything. The server refuses `scope=server` to everyone else with 403.

**Cards.** Each card has a severity badge (Error, Warning, Info), a title,
a sentence about what is wrong and the items concerned:

- Duplicate and look-alike **pairs** show both thumbnails side by side with
  their names, size and quality score (to pick which to keep) and, for
  duplicates in a set you can edit, a "Delete" button under each: it asks
  for confirmation, deletes that item, removes its card from the grid and
  refreshes the list. Without JavaScript it is the item's delete
  confirmation page.
- **Lists** of items (no emoji, very small, ...) show a small thumbnail,
  the name, what is wrong with it (the entries that are not emoji, "64×64
  px", the reason Telegram skips it) and a link to fix it ("Edit emojis"
  opens the item's edit form, "Review", "Find a better version").
- Set-level fixes are buttons: "Publish to Telegram", "Pull again", "Add
  items".
- **"Not a problem"** dismisses the card (an intentional near-duplicate, a
  sticker meant to be small). Dismissed problems are hidden and counted
  ("Show 1 dismissed"); shown, they are greyed out with "Show again". A
  dismissal is stored on the server (`problem_dismissals`) under the
  problem's id, a hash of what makes it a problem: when those items change
  (another file, other emojis, one more item without emoji) it is a new
  problem and shows up again. The forms post to
  `/sets/{id}/problems/{problem}/dismiss` and `/restore` (with
  `csrf_token`); htmx gets the list back, plain forms are redirected to
  where they came from.

**What is checked** (severity in brackets):

| Problem | When |
|---|---|
| Exact duplicates (warning) | Two or more items store the same file: different uploads that are identical after sanitising (e.g. the same PNG saved with other settings). The same upload twice is refused by the upload itself. |
| Near-duplicates (warning) | The same picture resized, recompressed, blurred or cropped by a few percent. Candidates come from the perceptual hashes (pHash ≤ 20 or dHash ≤ 12 of 64 bits) or from the AI; each is then compared on the grid thumbnails: 48×48 grey, where the worst of 8×8 blocks may differ by at most 16 of 255, also after trimming transparent margins and with 3, 6 and 10 % centre crops, and the colours of a 4×4 grid by at most 12 (so the same drawing in another colour is not a copy). Only items of the same kind (still or moving), about the same shape and, for animations, length. The thresholds come from synthetic stickers: copies scored up to 10.5, the same face with other eyes 27 or more. |
| Look alike (info, an AI hint) | CLIP similarity at least 0.95 between two items that are not near-duplicates; only with AI on. At most 30 pairs per set, the most similar first. Variations of one character often look alike on purpose. |
| Missing files (error) | An item has no original or no thumbnail. |
| Processing failed (warning) | Generating the variants failed (with the job's error) or never ran; still queued is fine. |
| Git sync failed (error) | The git pack's last pull or push failed. |
| Publishing to Telegram failed (error) | The set's newest Telegram export job failed (also before the first successful publish). |
| Cannot become Telegram stickers (warning) | In a published set: videos longer than 10 s, animations and videos without ffmpeg, and items whose sticker rendition could not be made. Publishing leaves them out. |
| Telegram pack out of date (warning or info) | In a published set: items not in the pack yet, items with a new file, stickers of deleted items (warning), or only metadata or order changed since the last successful publish (info). A set larger than one pack (120 stickers) is published as several packs, which are compared together; that it is larger is no problem. |
| No emoji (warning) | In a published set: items without an emoji; Telegram gets 🙂 for them. |
| Emoji entries that are not emoji (warning when published, else info) | Words typed into the emoji field; Telegram drops them. |
| Cut short on Telegram (info) | In a published set: animations and videos longer than 3 s; the pack gets the first 3 s. |
| Very small pictures (info, warning when published) | The longest side is under 128 px. |
| Also in other sets (info) | The same file is in another of your sets that got it first (shown once, in the later set). Other users' copies are not your problem and never show. |
| AI suggestions to review (info) | Items with AI-filled keywords, emojis, vibe, name or text nobody confirmed. One card per set. |
| Empty set (info) | No items yet. |

"Published" means bound to a Telegram pack while the Telegram integration is
on; without the integration nothing about Telegram is checked.

**Speed.** Nothing is cached but the thumbnails' signatures (by media id,
which never changes). Within a set every pair is compared on the stored
hashes and embeddings, and only candidates are checked pixel by pixel;
across sets, the same file is found by grouping on its hash in SQL. In a
debug build, a set of 200 items of one cartoon character (where nearly every
pair is a candidate) takes 3 to 4 s the first time (computing 200 thumbnail
signatures) and about 0.2 s afterwards; the Problems tab over 6 sets and
220 items about 0.3 s. Release builds are faster.

<!-- ---- problems (end) ---- -->

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
`/static/upload.js`, `/static/emoji.js`, `/static/suggest.js`, `/static/emoji.json`,
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
two files: `emoji.json`, and the names table of `e:skull` that the server
and every client embed, `crates/stickpile-search/src/emoji_names.tsv`
(1,914 emojis, one per line: the emoji, its English name and keywords,
the slang included; the same Unicode notice in its header). The TSV stays
in git as the source; `stickpile-search`'s `build.rs` deflates it into
the binaries (about 33 kB instead of 90 kB) and the table is inflated on
first use.

## Tests

`crates/stickpile-server/tests/web_auth.rs`, `web_pages.rs` and
`web_flows.rs` (helpers in `tests/web_common/`), the rechecks' tests in
`web_recheck.rs` (the settings form and its errors keeping what was sent,
the page and the fragment it polls, cancel with and without htmx, the
failed pictures linking to the items, "Recheck this set", the admin tab:
403 for users, the server's own rechecks listed, the audit entry), the export page's tests
in `export.rs` (downloads for readers, publishing only for the owner, form
errors keeping the input, the publish and WhatsApp jobs, Telegram, Discord
and Signal published together with Discord's options prefilled), and the
import pages' tests in `integrations_discord.rs` and `integrations_signal.rs`
(form errors, a refused Signal link not shown back, the pages and links
gone when the platform is off, the Signal-ready zip offered) cover: every page renders for
its user; anonymous redirects (401 + `HX-Redirect` for htmx); 404 for content
you cannot see; 403 for admin pages and others' sets; CSRF on urlencoded,
htmx and multipart forms, and cross-site posts; the login flow and cookie
flags; logout; the TOTP step; registration per policy; tailnet sign-in;
htmx fragments without the document shell; escaping of `<script>` and quotes;
the set page's "View" control (in the filter form, `?view=compact` and
`?view=grid`, other parameters kept, the JS-only toggles hidden), the set
lists' control and card parts, the start page's recently copied cards and
their own control (`data-view-prefs="home"`, the Set field, read-only
menu data, no "⋯" in the grid style's CSS), themes (`<html data-theme>` per
account, none for System, the top bar's switch left of the account group,
Settings → Appearance and
`POST /settings/theme` with its validation and CSRF) and the context-menu data per permission (no edit, delete
or move entries for viewers, no move entries when sorted or filtered, the
named "⋯" button); copying (`web_copy_is_recorded_and_shows_in_recently_copied`:
the copy controls' data, the usage event app.js posts accepted once with the
session cookie and CSRF header and refused without them or cross-site, and
the item first in "Recently copied" with its copy controls, quick copy and
menu);
the CSP header and the absence of inline styles and scripts; static asset
types; the emoji picker's markers (the emojis inputs, the script order, the
`:` suggestions' combobox, the `emoji.json` notice and contents, its terms
column and `common` list); HTML error pages; and the flows end to end (sets, uploads with
near-duplicate warnings, uploads without files (the form again, or the
inline message for htmx), imports, editing, likes and filters, stats and
popular, settings, the admin panel, git pack and Telegram forms, job polling,
the cover image in the set settings: upload, errors, replace by an item,
remove).

`tests/ai_admin.rs` covers the tray's AI side on the server: the
`data-ai` and `data-ai-suggest` attributes (the URL when AI is ready or
loading, absent when off or failed), the 503 codes (`unavailable` without
`Retry-After`, `ai_loading` with it, also for re-running enrichment), the suggestion endpoint with a web session (CSRF header, same origin),
and uploads with `ai_fields`. The tray itself (marking, approving, editing,
"Approve all", what gets uploaded) was checked with Playwright in Chromium,
Firefox and WebKit against a server without models: the test intercepts the
suggestion endpoint in the page and adds the attribute. The status line
(suggesting, loading with retries, busy, failed with Try again, off,
signed out) and typing during a request were checked the same way, against
a server with AI on and the endpoint answered by the test, and the timing
in [ai.md](ai.md#suggestions-before-an-upload) against the real models.

Copying in the browser was checked with Playwright against a test server
with a still PNG, JPEG and WebP, a GIF, an animated WebP, an MP4 and a WebM:
every route (button, quick copy in the Grid style, Ctrl+C on a card and on
the item page, the menu on the start page and in search results, Copy
link), the PNG handed to `navigator.clipboard.write` and its size, the
usage events, the error region, and the dialog on plain `http://` (a tailnet
name mapped to 127.0.0.1). Headless Chromium reads the PNG back from its
clipboard; headless Firefox's clipboard keeps only text, and headless WebKit
refuses to read without a paste prompt, so there the check stops at the
write resolving with the right PNG. Headless WebKit in the nix sandbox
crashes on any `<video>`, so video frames were checked in Chromium (MP4 and
WebM at full size) and Firefox (WebM at full size, MP4 through the
thumbnail fallback).

`tests/problems.rs` covers the problems report through the API and the web
UI: duplicates and filters, dismissals that come back when the items
change, the same file in two sets, Telegram blockers, the set page's section
(owner only, loaded lazily), dismiss / restore / delete with htmx and
without, the Problems tab, and the permissions of owners, other users and
admins on public and private sets. The checks themselves are unit-tested in
`src/service/problems/tests.rs`, with real images through the media
pipeline for the near-duplicate thresholds. The section and the tab
(loading, dismissing, restoring, the refresh after editing an item in the
grid, deleting one of a pair) were checked with Playwright in Chromium,
Firefox and WebKit, in both themes at desktop and phone widths.
