// stickpile upload tray. Plain JavaScript, no build step, loaded with
// `defer` from /static/upload.js (the CSP allows no inline scripts).
//
// Upload forms mark their file area with [data-dropzone]. Without this
// script the file input posts the chosen files as they are. With it, files
// that are chosen, dropped on the form or pasted anywhere on the page
// (Ctrl+V) are staged in a tray first, and nothing is sent until Upload is
// pressed. Each staged file can be selected and removed, and gets its
// details: name, keywords, emojis (with emoji.js's picker), vibe and tags.
// Still images (PNG, JPEG, still WebP) can also be rotated, flipped and
// cropped.
//
// Edits are applied in the browser when the form is submitted and the
// result is sent as a PNG (lossless). Files without image edits are sent as
// their original bytes, renamed if needed. GIFs, animated WebP and videos
// cannot be edited: a canvas keeps just their first frame. The details go
// in a hidden `meta` field, a JSON array with one entry per file in the
// files' order (stickpile_api::UploadMeta, as in the API).
//
// The form's data-aspect ("4:3") adds the set's recommended aspect ratio as
// a crop preset; data-vibes names the datalist of vibe suggestions.
(function () {
  "use strict";

  // MIME type -> kind; "webp" is decided by looking at the file.
  var KINDS = {
    "image/png": "still",
    "image/jpeg": "still",
    "image/webp": "webp",
    "image/gif": "anim",
    "video/mp4": "video",
    "video/webm": "video",
  };
  var BY_EXT = {
    png: "image/png",
    jpg: "image/jpeg",
    jpeg: "image/jpeg",
    webp: "image/webp",
    gif: "image/gif",
    mp4: "video/mp4",
    webm: "video/webm",
  };
  var EXT_OF = {
    "image/png": ".png",
    "image/jpeg": ".jpg",
    "image/webp": ".webp",
    "image/gif": ".gif",
    "video/mp4": ".mp4",
    "video/webm": ".webm",
  };
  var ACCEPTED = "PNG, JPEG, WebP, GIF, MP4 or WebM";
  var MIN_CROP = 4; // smallest crop side, image pixels
  var STAGE_MIN_H = 320; // the editor preview may always be this tall, CSS pixels
  var STAGE_VH = 0.7; // and up to this share of the window height
  var MAX_ZOOM = 3; // small images are shown at most this much larger
  var THUMB = 176; // edited thumbnails, device pixels
  // Orientation as a 2×2 matrix, to notice when edits cancel out.
  var IDENTITY = [1, 0, 0, 1];
  var TURN = {
    left: [0, 1, -1, 0],
    right: [0, -1, 1, 0],
    flipH: [-1, 0, 0, 1],
    flipV: [1, 0, 0, -1],
  };

  var trays = [];
  var nextId = 1;

  // ---- helpers ---------------------------------------------------------

  function el(tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  function button(cls, text, label) {
    var b = el("button", cls, text);
    b.type = "button";
    if (label) b.setAttribute("aria-label", label);
    return b;
  }

  function tool(glyph, text, label) {
    var b = button("small", null, label);
    var g = el("span", "glyph", glyph);
    g.setAttribute("aria-hidden", "true");
    b.appendChild(g);
    b.appendChild(document.createTextNode(" " + text));
    return b;
  }

  function plural(n, one, many) {
    return n + " " + (n === 1 ? one : many);
  }

  function clamp(v, lo, hi) {
    return Math.max(lo, Math.min(hi, v));
  }

  function mul(a, b) {
    return [
      a[0] * b[0] + a[1] * b[2],
      a[0] * b[1] + a[1] * b[3],
      a[2] * b[0] + a[3] * b[2],
      a[2] * b[1] + a[3] * b[3],
    ];
  }

  function isIdentity(m) {
    return m[0] === 1 && m[1] === 0 && m[2] === 0 && m[3] === 1;
  }

  function splitName(name) {
    var base = String(name || "").replace(/^.*[\/\\]/, "");
    var dot = base.lastIndexOf(".");
    return dot > 0 ? { stem: base.slice(0, dot), ext: base.slice(dot) } : { stem: base, ext: "" };
  }

  function typeOf(file) {
    if (KINDS[file.type]) return file.type;
    return BY_EXT[splitName(file.name).ext.slice(1).toLowerCase()] || "";
  }

  // The server drops a leading order number ("003-naahh" is named "naahh");
  // the tray shows the name the item will get.
  function withoutOrder(stem) {
    var m = /^[0-9]+[-_. ]+([^-_. ].*)$/.exec(stem);
    return m ? m[1] : stem;
  }

  // The file name the server turns into the item name.
  function cleanName(name) {
    return String(name || "")
      .replace(/[\/\\\u0000-\u001f\u007f]/g, " ")
      .replace(/\s+/g, " ")
      .trim()
      .slice(0, 200);
  }

  // A VP8X header with the animation flag.
  function isAnimatedWebp(file) {
    return file
      .slice(0, 32)
      .arrayBuffer()
      .then(function (buf) {
        var b = new Uint8Array(buf);
        if (b.length < 21) return false;
        var chunk = String.fromCharCode(b[12], b[13], b[14], b[15]);
        return chunk === "VP8X" && (b[20] & 0x02) !== 0;
      })
      .catch(function () {
        return false;
      });
  }

  function parseAspect(s) {
    var m = /^\s*(\d+)\s*:\s*(\d+)\s*$/.exec(s || "");
    if (!m || +m[1] === 0 || +m[2] === 0) return null;
    return { label: m[1] + ":" + m[2], ratio: +m[1] / +m[2] };
  }

  function canvas(w, h) {
    var c = document.createElement("canvas");
    c.width = w;
    c.height = h;
    return c;
  }

  // A copy of `src` turned or mirrored.
  function transformed(src, op) {
    var w = src.width;
    var h = src.height;
    var turn = op === "left" || op === "right";
    var c = canvas(turn ? h : w, turn ? w : h);
    var g = c.getContext("2d");
    if (op === "right") {
      g.translate(h, 0);
      g.rotate(Math.PI / 2);
    } else if (op === "left") {
      g.translate(0, w);
      g.rotate(-Math.PI / 2);
    } else if (op === "flipH") {
      g.translate(w, 0);
      g.scale(-1, 1);
    } else {
      g.translate(0, h);
      g.scale(1, -1);
    }
    g.drawImage(src, 0, 0);
    return c;
  }

  // The same region after the image is turned or mirrored (W×H before).
  function transformedCrop(c, op, W, H) {
    if (op === "right") return { x: H - c.y - c.h, y: c.x, w: c.h, h: c.w };
    if (op === "left") return { x: c.y, y: W - c.x - c.w, w: c.h, h: c.w };
    if (op === "flipH") return { x: W - c.x - c.w, y: c.y, w: c.w, h: c.h };
    return { x: c.x, y: H - c.y - c.h, w: c.w, h: c.h };
  }

  // The largest centred region of the given ratio (width / height).
  function centred(W, H, ratio) {
    var w = W;
    var h = H;
    if (W / H > ratio) w = H * ratio;
    else h = W / ratio;
    return { x: (W - w) / 2, y: (H - h) / 2, w: w, h: h };
  }

  function moved(c, dx, dy, W, H) {
    return { x: clamp(c.x + dx, 0, W - c.w), y: clamp(c.y + dy, 0, H - c.h), w: c.w, h: c.h };
  }

  // Drag one corner ("nw", "ne", "sw", "se"); the opposite corner stays.
  function resized(c, corner, dx, dy, W, H, ratio) {
    var west = corner.indexOf("w") >= 0;
    var north = corner.indexOf("n") >= 0;
    var ax = west ? c.x + c.w : c.x;
    var ay = north ? c.y + c.h : c.y;
    var w = west ? c.w - dx : c.w + dx;
    var h = north ? c.h - dy : c.h + dy;
    var maxW = west ? ax : W - ax;
    var maxH = north ? ay : H - ay;
    if (ratio) {
      // Follow the axis that moved more, then fit the room there is.
      if (Math.abs(dx) < Math.abs(dy) * ratio) w = h * ratio;
      var most = Math.min(maxW, maxH * ratio);
      w = clamp(w, Math.min(MIN_CROP * Math.max(1, ratio), most), most);
      h = w / ratio;
    } else {
      w = clamp(w, Math.min(MIN_CROP, maxW), maxW);
      h = clamp(h, Math.min(MIN_CROP, maxH), maxH);
    }
    return { x: west ? ax - w : ax, y: north ? ay - h : ay, w: w, h: h };
  }

  // Whole pixels inside the image.
  function pixels(c, W, H) {
    var x = clamp(Math.round(c.x), 0, W - 1);
    var y = clamp(Math.round(c.y), 0, H - 1);
    var w = clamp(Math.round(c.x + c.w) - x, 1, W - x);
    var h = clamp(Math.round(c.y + c.h) - y, 1, H - y);
    return { x: x, y: y, w: w, h: h };
  }

  // ---- one tray per upload form -----------------------------------------

  function setupTray(zone) {
    if (zone.getAttribute("data-ready")) return;
    var input = zone.querySelector("input[type=file]");
    var form = zone.closest("form");
    if (!input || !form) return;
    zone.setAttribute("data-ready", "1");
    zone.classList.add("js");
    setupDrop(zone);
    // Old browsers keep the plain input (and drag-and-drop above).
    if (typeof DataTransfer === "undefined" || !window.createImageBitmap) {
      input.addEventListener("change", function () {
        var n = input.files ? input.files.length : 0;
        zone.setAttribute("data-count", n ? plural(n, "file", "files") + " selected" : "");
      });
      return;
    }

    var hint = zone.querySelector(".paste-hint");
    if (hint) {
      if (/Mac|iPhone|iPad/.test(navigator.platform || "")) {
        hint.textContent = hint.textContent.replace("Ctrl+V", "⌘V");
      }
      hint.hidden = false;
    }

    var submit = form.querySelector("button[type=submit]");
    var submitLabel = submit ? submit.textContent : "";
    var aspect = parseAspect(form.getAttribute("data-aspect"));
    var entries = [];
    var current = null;
    var drag = null;
    var busy = false;
    var commitTimer = null;
    var api;

    // -- the tray markup

    var tray = el("div", "tray");
    tray.hidden = true;
    var head = el("div", "tray-head");
    var title = el("p", "tray-title");
    var clearAll = button("small", "Remove all");
    head.appendChild(title);
    head.appendChild(clearAll);
    var list = el("ul", "tray-list");
    list.setAttribute("aria-label", "Files to upload");

    var editor = el("div", "tray-editor");
    editor.setAttribute("role", "group");

    // The selected file's details (sent as `meta`), then its image.
    var details = el("div", "tray-details");
    var nameInput = detailField("Name", "", 200);
    var keywordsInput = detailField("Keywords", "What is in it, e.g. cat, stare. Separate with commas. Sent to Telegram as the sticker's search words.", 0);
    var emojisInput = detailField("Emojis", "Separate with spaces.", 0);
    // The emoji picker (emoji.js, loaded first; else it finds the attribute).
    emojisInput.setAttribute("data-emoji-picker", "");
    if (window.stickpileEmoji) window.stickpileEmoji.attach(emojisInput);
    var vibeInput = detailField("Vibe", "The mood in one word, e.g. tired or smug.", 50);
    if (form.getAttribute("data-vibes")) vibeInput.setAttribute("list", form.getAttribute("data-vibes"));
    var tagsInput = detailField("Tags", "Your own labels for sorting and filtering, e.g. reaction, nsfw (tag:nsfw). Separate with commas. Not sent to Telegram.", 0);
    var copyRow = el("div", "tray-copy");
    var copyAll = button("small", "Use for every file");
    copyAll.id = "tray-copy-" + nextId++;
    var copyHint = el("p", "hint", "Gives the other files these keywords, emojis, vibe and tags. Names stay.");
    copyHint.id = copyAll.id + "-hint";
    copyAll.setAttribute("aria-describedby", copyHint.id);
    copyRow.appendChild(copyAll);
    copyRow.appendChild(copyHint);
    details.appendChild(copyRow);
    var detailInputs = { keywords: keywordsInput, emojis: emojisInput, vibe: vibeInput, tags: tagsInput };

    function detailField(label, hint, maxLength) {
      var row = el("div", "tray-field");
      var input = el("input");
      input.type = "text";
      input.id = "tray-" + label.toLowerCase() + "-" + nextId++;
      input.autocomplete = "off";
      if (maxLength) input.maxLength = maxLength;
      var l = el("label", null, label);
      l.htmlFor = input.id;
      row.appendChild(l);
      row.appendChild(input);
      if (hint) {
        var h = el("span", "hint", hint);
        h.id = input.id + "-hint";
        input.setAttribute("aria-describedby", h.id);
        row.appendChild(h);
      }
      details.appendChild(row);
      return input;
    }

    // Sent with the files: one entry per file, in the files' order.
    var metaInput = el("input");
    metaInput.type = "hidden";
    metaInput.name = "meta";
    form.insertBefore(metaInput, zone);

    var media = el("div", "tray-media");
    var tools = el("div", "tray-tools");
    tools.setAttribute("role", "group");
    tools.setAttribute("aria-label", "Image tools");
    var turnLeft = tool("↺", "Rotate left");
    var turnRight = tool("↻", "Rotate right");
    var flipH = tool("⇆", "Flip", "Flip horizontally");
    var flipV = tool("⇅", "Flip vertically");
    var cropLabel = el("span", "tool-label", "Crop");
    var presets = [{ label: "Free", ratio: null, button: button("small", "Free") }];
    presets.push({ label: "Square", ratio: 1, button: button("small", "Square") });
    if (aspect && Math.abs(aspect.ratio - 1) > 1e-9) {
      presets.push({
        label: aspect.label,
        ratio: aspect.ratio,
        button: button("small", aspect.label, aspect.label + ", the set's aspect ratio"),
      });
    }
    var reset = button("small", "Reset");
    [turnLeft, turnRight, flipH, flipV, cropLabel].forEach(function (b) {
      tools.appendChild(b);
    });
    presets.forEach(function (p) {
      tools.appendChild(p.button);
    });
    tools.appendChild(reset);

    var stage = el("div", "crop-stage");
    var area = el("div", "crop-area");
    var view = el("canvas");
    var box = el("div", "crop-box");
    box.tabIndex = 0;
    box.setAttribute("role", "group");
    box.setAttribute("aria-label", "Crop area");
    ["nw", "ne", "sw", "se"].forEach(function (c) {
      var h = el("span", "handle " + c);
      h.setAttribute("data-handle", c);
      box.appendChild(h);
    });
    area.appendChild(view);
    area.appendChild(box);
    stage.appendChild(area);
    var cropHint = el(
      "p",
      "hint",
      "Drag the corners to crop and the inside to move it. With the crop area focused, arrow keys move it and Shift+arrow keys resize it."
    );
    cropHint.id = "tray-crop-hint-" + nextId++;
    box.setAttribute("aria-describedby", cropHint.id);

    var preview = el("div", "tray-preview");
    var note = el("p", "hint tray-note");
    var info = el("p", "tray-info");
    [tools, stage, cropHint, preview, note, info].forEach(function (n) {
      media.appendChild(n);
    });
    editor.appendChild(details);
    editor.appendChild(media);
    var status = el("p", "tray-status");
    status.setAttribute("role", "status");
    tray.appendChild(head);
    tray.appendChild(list);
    tray.appendChild(editor);
    zone.parentNode.insertBefore(tray, zone.nextSibling);
    // Outside the tray, so "Skipped …" shows even when nothing was added.
    zone.parentNode.insertBefore(status, tray.nextSibling);
    setupDrop(tray);

    // -- state

    function say(text, isError) {
      status.textContent = text || "";
      status.classList.toggle("error", !!isError);
    }

    function uniqueName(base) {
      var taken = {};
      entries.forEach(function (e) {
        taken[e.name] = true;
      });
      if (!taken[base]) return base;
      for (var i = 2; ; i++) if (!taken[base + " " + i]) return base + " " + i;
    }

    function displayName(e) {
      return cleanName(e.name) || e.stem;
    }

    function sync() {
      var dt = new DataTransfer();
      entries.forEach(function (e) {
        dt.items.add(e.file);
      });
      input.files = dt.files;
      var n = entries.length;
      tray.hidden = n === 0;
      zone.classList.toggle("has-files", n > 0);
      title.textContent = n
        ? plural(n, "file", "files") + " ready. Select one to describe it or edit the image."
        : "";
      if (submit) submit.textContent = n ? "Upload " + plural(n, "file", "files") : submitLabel;
      copyRow.hidden = n < 2;
      if (!n) metaInput.value = "";
    }

    function add(files, pasted) {
      var added = [];
      var skipped = [];
      Array.prototype.forEach.call(files, function (file) {
        var type = typeOf(file);
        if (!type) {
          skipped.push(file.name || "the pasted item");
          return;
        }
        var parts = splitName(file.name);
        var stem = parts.stem;
        // Browsers name every pasted image "image.png".
        if (pasted && (!stem || stem === "image")) stem = uniqueName("pasted image");
        stem = cleanName(withoutOrder(stem)) || "item";
        var e = {
          id: nextId++,
          file: file,
          type: type,
          kind: KINDS[type] === "webp" ? "still" : KINDS[type],
          stem: stem,
          name: stem,
          ext: parts.ext || EXT_OF[type],
          url: URL.createObjectURL(file),
          bitmap: null,
          work: null,
          crop: null,
          ratio: null,
          orient: IDENTITY,
          size: null,
          edited: false,
          failed: false,
          // Details as typed; split into lists when sent (metaOf).
          keywords: "",
          emojis: "",
          vibe: "",
          tags: "",
        };
        if (KINDS[type] === "webp") {
          isAnimatedWebp(file).then(function (anim) {
            if (!anim || entries.indexOf(e) < 0) return;
            e.kind = "anim";
            release(e, true);
            if (current === e) show(e);
          });
        }
        entries.push(e);
        list.appendChild(thumb(e));
        added.push(e);
      });
      sync();
      if (added.length) select(added[0]);
      var parts = [];
      if (added.length) parts.push((pasted ? "Pasted " : "Added ") + plural(added.length, "file", "files") + ".");
      if (skipped.length) {
        parts.push(
          "Skipped " + skipped.join(", ") + ": only " + ACCEPTED + " files can be uploaded."
        );
      }
      say(parts.join(" "), skipped.length > 0);
      return added.length;
    }

    function thumb(e) {
      var li = el("li", "tray-item");
      var b = button("tray-thumb");
      b.setAttribute("aria-pressed", "false");
      var media;
      if (e.kind === "video") {
        media = el("video", "thumb-media");
        media.muted = true;
        media.preload = "metadata";
      } else {
        media = el("img", "thumb-media");
        media.alt = "";
      }
      media.src = e.url;
      var name = el("span", "thumb-name", e.name);
      var badge = el("span", "thumb-badge", "edited");
      badge.hidden = true;
      b.appendChild(media);
      b.appendChild(name);
      b.appendChild(badge);
      var rm = button("tray-remove", "×", "Remove " + e.name + " from the upload");
      li.appendChild(b);
      li.appendChild(rm);
      e.li = li;
      e.thumbButton = b;
      e.thumbMedia = media;
      e.thumbName = name;
      e.badge = badge;
      e.removeButton = rm;
      b.title = e.name;
      b.addEventListener("click", function () {
        select(e);
      });
      rm.addEventListener("click", function () {
        remove(e);
      });
      return li;
    }

    function relabel(e) {
      var n = displayName(e);
      e.thumbName.textContent = n;
      e.thumbButton.title = n;
      e.removeButton.setAttribute("aria-label", "Remove " + n + " from the upload");
      if (current === e) editor.setAttribute("aria-label", "Edit " + n);
    }

    // Free the decoded image of an entry without edits (it is decoded again
    // when selected); `all` drops edits too.
    function release(e, all) {
      if (!all && e.edited) return;
      if (e.bitmap && e.bitmap.close) e.bitmap.close();
      e.bitmap = null;
      e.work = null;
      if (all) {
        e.crop = null;
        e.orient = IDENTITY;
        e.edited = false;
        e.badge.hidden = true;
        if (entries.indexOf(e) >= 0) e.thumbMedia.src = e.url;
      }
    }

    // Stop a media element from loading before its blob URL is revoked.
    function unload(m) {
      if (m.tagName === "VIDEO") m.pause();
      m.removeAttribute("src");
      if (m.tagName === "VIDEO") m.load();
    }

    function clearPreview() {
      Array.prototype.forEach.call(preview.children, unload);
      preview.textContent = "";
    }

    function select(e) {
      if (current && current !== e) {
        release(current, false);
        say("");
      }
      current = e;
      entries.forEach(function (x) {
        x.thumbButton.setAttribute("aria-pressed", x === e ? "true" : "false");
      });
      nameInput.value = e.name;
      Object.keys(detailInputs).forEach(function (k) {
        detailInputs[k].value = e[k];
      });
      editor.setAttribute("aria-label", "Edit " + displayName(e));
      show(e);
    }

    function show(e) {
      var still = e.kind === "still" && !e.failed;
      tools.hidden = !still;
      stage.hidden = !still;
      cropHint.hidden = !still;
      preview.hidden = still;
      note.hidden = still;
      clearPreview();
      info.textContent = "";
      if (!still) {
        var m;
        if (e.kind === "video") {
          m = el("video");
          m.controls = true;
          m.muted = true;
          m.loop = true;
        } else {
          m = el("img");
          m.alt = "Preview of " + displayName(e);
        }
        m.src = e.url;
        preview.appendChild(m);
        if (e.failed) {
          note.textContent =
            "This browser cannot read the image, so it cannot be edited here. It is uploaded as it is and the server checks it.";
        } else {
          note.textContent =
            (e.kind === "video" ? "Videos" : "Animations") +
            " are uploaded as they are: you can rename them, but editing here would keep only the first frame.";
        }
        return;
      }
      if (e.work) {
        draw();
        return;
      }
      stage.hidden = true;
      info.textContent = "Loading…";
      createImageBitmap(e.file).then(
        function (bitmap) {
          if (entries.indexOf(e) < 0 || e.kind !== "still") {
            if (bitmap.close) bitmap.close();
            return;
          }
          e.bitmap = bitmap;
          e.size = { w: bitmap.width, h: bitmap.height };
          start(e);
          if (current === e) {
            stage.hidden = false;
            draw();
          } else {
            release(e, false);
          }
        },
        function () {
          e.failed = true;
          if (current === e) show(e);
        }
      );
    }

    // A fresh working copy of the decoded image.
    function start(e) {
      var c = canvas(e.bitmap.width, e.bitmap.height);
      c.getContext("2d").drawImage(e.bitmap, 0, 0);
      e.work = c;
      e.orient = IDENTITY;
      e.crop = e.ratio ? centred(c.width, c.height, e.ratio) : { x: 0, y: 0, w: c.width, h: c.height };
    }

    function draw() {
      var e = current;
      if (!e || !e.work) return;
      var W = e.work.width;
      var H = e.work.height;
      var room = Math.max(120, media.clientWidth - 24);
      var tall = Math.max(STAGE_MIN_H, Math.round(window.innerHeight * STAGE_VH));
      var scale = Math.min(room / W, tall / H, MAX_ZOOM);
      var cw = Math.max(1, Math.round(W * scale));
      var ch = Math.max(1, Math.round(H * scale));
      var dpr = window.devicePixelRatio || 1;
      view.width = Math.max(1, Math.round(cw * dpr));
      view.height = Math.max(1, Math.round(ch * dpr));
      view.style.width = cw + "px";
      view.style.height = ch + "px";
      var g = view.getContext("2d");
      g.imageSmoothingEnabled = scale < 1;
      g.imageSmoothingQuality = "high";
      g.clearRect(0, 0, view.width, view.height);
      g.drawImage(e.work, 0, 0, view.width, view.height);
      e.scale = cw / W;
      presets.forEach(function (p) {
        p.button.setAttribute("aria-pressed", p.ratio === e.ratio ? "true" : "false");
      });
      place();
    }

    function place() {
      var e = current;
      var s = e.scale;
      box.style.left = e.crop.x * s + "px";
      box.style.top = e.crop.y * s + "px";
      box.style.width = e.crop.w * s + "px";
      box.style.height = e.crop.h * s + "px";
      describe(e);
    }

    function describe(e) {
      if (isEdited(e)) {
        var r = pixels(e.crop, e.work.width, e.work.height);
        info.textContent =
          "Original " + e.size.w + " × " + e.size.h + " px. Uploaded as a " + r.w + " × " + r.h + " px PNG.";
      } else {
        info.textContent = "No changes: the original file is uploaded (" + e.size.w + " × " + e.size.h + " px).";
      }
    }

    function isEdited(e) {
      if (!e.work) return e.edited;
      var r = pixels(e.crop, e.work.width, e.work.height);
      var full = r.x === 0 && r.y === 0 && r.w === e.work.width && r.h === e.work.height;
      return !isIdentity(e.orient) || !full;
    }

    // After a finished change: the badge and the thumbnail.
    function commit(e) {
      e.edited = isEdited(e);
      e.badge.hidden = !e.edited;
      if (!e.edited) {
        e.thumbMedia.src = e.url;
        return;
      }
      var r = pixels(e.crop, e.work.width, e.work.height);
      var s = Math.min(1, THUMB / Math.max(r.w, r.h));
      var t = canvas(Math.max(1, Math.round(r.w * s)), Math.max(1, Math.round(r.h * s)));
      var g = t.getContext("2d");
      g.imageSmoothingQuality = "high";
      g.drawImage(e.work, r.x, r.y, r.w, r.h, 0, 0, t.width, t.height);
      e.thumbMedia.src = t.toDataURL("image/png");
    }

    function commitSoon(e) {
      if (commitTimer) clearTimeout(commitTimer);
      commitTimer = setTimeout(function () {
        commitTimer = null;
        if (entries.indexOf(e) >= 0 && e.work) commit(e);
      }, 200);
    }

    function turn(op) {
      var e = current;
      if (!e || !e.work) return;
      var W = e.work.width;
      var H = e.work.height;
      e.work = transformed(e.work, op);
      e.crop = transformedCrop(e.crop, op, W, H);
      e.orient = mul(TURN[op], e.orient);
      // A turned 4:3 crop would be 3:4: start the preset again.
      if ((op === "left" || op === "right") && e.ratio && e.ratio !== 1) {
        e.crop = centred(e.work.width, e.work.height, e.ratio);
      }
      draw();
      commit(e);
    }

    function setRatio(ratio) {
      var e = current;
      if (!e || !e.work) return;
      e.ratio = ratio;
      if (ratio) e.crop = centred(e.work.width, e.work.height, ratio);
      draw();
      commit(e);
    }

    function remove(e) {
      var i = entries.indexOf(e);
      if (i < 0) return;
      entries.splice(i, 1);
      e.li.parentNode.removeChild(e.li);
      if (current === e) {
        current = null;
        clearPreview();
        var next = entries[i] || entries[i - 1];
        if (next) {
          select(next);
          next.thumbButton.focus();
        } else {
          input.focus();
        }
      }
      release(e, true);
      unload(e.thumbMedia);
      URL.revokeObjectURL(e.url);
      sync();
      say("Removed " + displayName(e) + ".");
    }

    function clear() {
      var old = entries;
      entries = [];
      current = null;
      clearPreview();
      list.textContent = "";
      old.forEach(function (e) {
        release(e, true);
        unload(e.thumbMedia);
        URL.revokeObjectURL(e.url);
      });
      sync();
    }

    // The file to send for an entry: the original bytes, renamed if needed,
    // or the edited image as a PNG.
    function fileFor(e) {
      var name = displayName(e);
      if (e.kind === "still" && e.edited && e.work) {
        var r = pixels(e.crop, e.work.width, e.work.height);
        var out = canvas(r.w, r.h);
        out.getContext("2d").drawImage(e.work, r.x, r.y, r.w, r.h, 0, 0, r.w, r.h);
        return new Promise(function (resolve, reject) {
          out.toBlob(function (blob) {
            if (blob) resolve(new File([blob], name + ".png", { type: "image/png" }));
            else reject(new Error("the browser could not encode " + name));
          }, "image/png");
        });
      }
      var filename = name + e.ext;
      if (filename === e.file.name) return Promise.resolve(e.file);
      return Promise.resolve(new File([e.file], filename, { type: e.file.type || e.type }));
    }

    // The `meta` entry of a file (stickpile_api::UploadMeta).
    function metaOf(e) {
      var m = { name: displayName(e) };
      var keywords = splitList(e.keywords);
      var emojis = e.emojis.split(/[\s,]+/).filter(Boolean);
      var tags = splitList(e.tags);
      if (keywords.length) m.keywords = keywords;
      if (emojis.length) m.emojis = emojis;
      if (e.vibe.trim()) m.vibe = e.vibe.trim();
      if (tags.length) m.tags = tags;
      return m;
    }

    function splitList(text) {
      return text
        .split(/[,\n]/)
        .map(function (s) {
          return s.trim();
        })
        .filter(Boolean);
    }

    function submitWith(submitter) {
      if (busy) return;
      busy = true;
      if (submit) submit.disabled = true;
      var meta = JSON.stringify(entries.map(metaOf));
      var edited = entries.filter(function (e) {
        return e.edited;
      }).length;
      if (edited) say("Preparing " + plural(edited, "edited image", "edited images") + "…");
      Promise.all(entries.map(fileFor)).then(
        function (files) {
          // Submit again in a task of its own. Without edits every promise
          // is already settled, so this callback runs in the microtask
          // checkpoint right after the submit listener, while the browser
          // is still firing that submit event, and the HTML spec has
          // requestSubmit() do nothing then: the upload never started.
          setTimeout(function () {
            var dt = new DataTransfer();
            files.forEach(function (f) {
              dt.items.add(f);
            });
            input.files = dt.files;
            metaInput.value = meta;
            busy = false;
            if (submit) submit.disabled = false;
            say("");
            api.passing = true;
            try {
              if (form.requestSubmit) form.requestSubmit(submitter && submitter.form === form ? submitter : undefined);
              else form.submit();
            } finally {
              api.passing = false;
            }
          }, 0);
        },
        function (err) {
          busy = false;
          if (submit) submit.disabled = false;
          say("Could not prepare the upload: " + err.message, true);
        }
      );
    }

    // -- events

    input.addEventListener("change", function () {
      // The picker replaces the input's files; keep the earlier ones.
      var picked = Array.prototype.slice.call(input.files || []);
      if (picked.length) add(picked, false);
      else sync();
    });

    clearAll.addEventListener("click", function () {
      clear();
      say("Removed all files.");
      input.focus();
    });

    nameInput.addEventListener("input", function () {
      if (!current) return;
      current.name = nameInput.value;
      relabel(current);
    });

    Object.keys(detailInputs).forEach(function (k) {
      detailInputs[k].addEventListener("input", function () {
        if (current) current[k] = detailInputs[k].value;
      });
    });

    // Enter in a detail field would submit the form, i.e. upload every file
    // half described. (A datalist suggestion takes Enter before the page.)
    details.addEventListener("keydown", function (ev) {
      if (ev.key === "Enter" && !ev.isComposing && ev.target.tagName === "INPUT") ev.preventDefault();
    });

    copyAll.addEventListener("click", function () {
      var e = current;
      if (!e) return;
      entries.forEach(function (x) {
        if (x === e) return;
        Object.keys(detailInputs).forEach(function (k) {
          x[k] = e[k];
        });
      });
      say(
        "The other " +
          plural(entries.length - 1, "file has", "files have") +
          " the keywords, emojis, vibe and tags of " +
          displayName(e) +
          " now."
      );
    });

    nameInput.addEventListener("blur", function () {
      if (!current) return;
      if (!cleanName(current.name)) current.name = current.stem;
      current.name = cleanName(current.name);
      nameInput.value = current.name;
      relabel(current);
    });

    turnLeft.addEventListener("click", function () {
      turn("left");
    });
    turnRight.addEventListener("click", function () {
      turn("right");
    });
    flipH.addEventListener("click", function () {
      turn("flipH");
    });
    flipV.addEventListener("click", function () {
      turn("flipV");
    });
    presets.forEach(function (p) {
      p.button.addEventListener("click", function () {
        setRatio(p.ratio);
      });
    });
    reset.addEventListener("click", function () {
      var e = current;
      if (!e || !e.bitmap) return;
      e.ratio = null;
      start(e);
      draw();
      commit(e);
      say("Reset " + displayName(e) + " to the original.");
    });

    box.addEventListener("pointerdown", function (ev) {
      var e = current;
      if (!e || !e.work || ev.button !== 0) return;
      ev.preventDefault();
      box.focus({ preventScroll: true });
      drag = {
        corner: ev.target.getAttribute("data-handle"),
        x: ev.clientX,
        y: ev.clientY,
        crop: { x: e.crop.x, y: e.crop.y, w: e.crop.w, h: e.crop.h },
      };
      box.setPointerCapture(ev.pointerId);
    });
    box.addEventListener("pointermove", function (ev) {
      var e = current;
      if (!drag || !e || !e.work) return;
      var dx = (ev.clientX - drag.x) / e.scale;
      var dy = (ev.clientY - drag.y) / e.scale;
      var W = e.work.width;
      var H = e.work.height;
      e.crop = drag.corner
        ? resized(drag.crop, drag.corner, dx, dy, W, H, e.ratio)
        : moved(drag.crop, dx, dy, W, H);
      place();
    });
    ["pointerup", "pointercancel"].forEach(function (t) {
      box.addEventListener(t, function () {
        if (!drag) return;
        drag = null;
        if (current && current.work) commit(current);
      });
    });
    box.addEventListener("keydown", function (ev) {
      var e = current;
      var d = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] }[ev.key];
      if (!d || !e || !e.work) return;
      ev.preventDefault();
      var W = e.work.width;
      var H = e.work.height;
      var step = Math.max(1, Math.round(Math.min(W, H) / 50));
      e.crop = ev.shiftKey
        ? resized(e.crop, "se", d[0] * step, d[1] * step, W, H, e.ratio)
        : moved(e.crop, d[0] * step, d[1] * step, W, H);
      place();
      commitSoon(e);
    });

    var resizeTimer = null;
    window.addEventListener("resize", function () {
      if (resizeTimer) clearTimeout(resizeTimer);
      resizeTimer = setTimeout(function () {
        if (current && current.work && !tray.hidden) draw();
      }, 120);
    });

    api = {
      form: form,
      passing: false,
      count: function () {
        return entries.length;
      },
      add: function (files, pasted) {
        var n = add(files, pasted);
        if (n) tray.scrollIntoView({ block: "nearest" });
        return n;
      },
      clear: clear,
      submitWith: submitWith,
      visible: function () {
        return form.isConnected && form.getClientRects().length > 0;
      },
    };
    trays.push(api);
    // Files the input already holds (e.g. kept by the browser on reload).
    if (input.files && input.files.length) add(Array.prototype.slice.call(input.files), false);
  }

  // Drag-and-drop onto a dropzone or a tray.
  function setupDrop(target) {
    ["dragenter", "dragover"].forEach(function (t) {
      target.addEventListener(t, function (e) {
        if (!hasFiles(e.dataTransfer)) return;
        e.preventDefault();
        target.classList.add("over");
      });
    });
    ["dragleave", "dragend"].forEach(function (t) {
      target.addEventListener(t, function () {
        target.classList.remove("over");
      });
    });
    target.addEventListener("drop", function (e) {
      if (!hasFiles(e.dataTransfer)) return;
      e.preventDefault();
      target.classList.remove("over");
      var form = target.closest("form");
      var tray = trayOf(form);
      var files = e.dataTransfer.files;
      if (!files || !files.length) return;
      if (tray) {
        tray.add(files, false);
        return;
      }
      // Without the tray: the old behaviour, upload at once.
      var input = form && form.querySelector("input[type=file]");
      if (!input) return;
      input.files = files;
      input.dispatchEvent(new Event("change", { bubbles: true }));
      if (form.requestSubmit) form.requestSubmit();
      else form.submit();
    });
  }

  function hasFiles(dt) {
    return !!dt && Array.prototype.indexOf.call(dt.types || [], "Files") >= 0;
  }

  function trayOf(form) {
    for (var i = 0; i < trays.length; i++) if (trays[i].form === form) return trays[i];
    return null;
  }

  function filesIn(dt) {
    if (dt.files && dt.files.length) return Array.prototype.slice.call(dt.files);
    var out = [];
    Array.prototype.forEach.call(dt.items || [], function (item) {
      if (item.kind !== "file") return;
      var f = item.getAsFile();
      if (f) out.push(f);
    });
    return out;
  }

  function isTextField(t) {
    if (!(t instanceof Element)) return false;
    if (t.isContentEditable || t.tagName === "TEXTAREA") return true;
    return t.tagName === "INPUT" && !/^(file|checkbox|radio|button|submit|reset|range|color)$/i.test(t.type);
  }

  // ---- global listeners ------------------------------------------------

  // Paste anywhere on a page with an upload form. A text field keeps a
  // paste that also carries text; an image alone goes to the tray.
  document.addEventListener("paste", function (e) {
    var dt = e.clipboardData;
    if (!dt) return;
    var files = filesIn(dt);
    if (!files.length) return;
    if (isTextField(e.target) && Array.prototype.indexOf.call(dt.types || [], "text/plain") >= 0) return;
    var shown = trays.filter(function (t) {
      return t.visible();
    });
    if (!shown.length) return;
    var active = document.activeElement;
    var tray =
      shown.filter(function (t) {
        return t.form.contains(active);
      })[0] || shown[0];
    e.preventDefault();
    tray.add(files, true);
  });

  // Edited images are rendered before htmx (or the browser) sends the form.
  document.addEventListener(
    "submit",
    function (e) {
      var tray = trayOf(e.target);
      if (!tray || tray.passing || !tray.count()) return;
      e.preventDefault();
      e.stopPropagation();
      tray.submitWith(e.submitter);
    },
    true
  );

  document.addEventListener("htmx:afterRequest", function (e) {
    var tray = trayOf(e.detail && e.detail.elt);
    if (tray && e.detail.successful) tray.clear();
  });

  function enhance(root) {
    if (root.querySelectorAll) root.querySelectorAll("[data-dropzone]").forEach(setupTray);
  }

  document.addEventListener("htmx:load", function (e) {
    enhance(e.target);
  });
  enhance(document);
})();
