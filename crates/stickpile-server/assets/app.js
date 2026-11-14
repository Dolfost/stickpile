// stickpile web UI enhancements. Plain JavaScript, no build step, loaded
// with `defer` from /static/app.js (the CSP allows no inline scripts).
//
// * Copy to the clipboard (buttons hidden until this script runs): the
//   Copy buttons, the quick copy button on card thumbnails, the cards'
//   menus and Ctrl+C (⌘C) on a focused card or on an item page. Browsers
//   only put image/png on the clipboard, so:
//   - stills: the PNG the server chose (the original or the png variant),
//     or the original converted to PNG on a canvas;
//   - animations and videos: a PNG of a still frame (the thumbnail's frame,
//     at full size), and the UI says so; "Copy link" copies the file URL.
//   A successful image copy is reported as a usage event (POST
//   /api/v1/usage, client "web", target "generic", the variant). The
//   Clipboard API needs a secure context (https or localhost); elsewhere a
//   dialog explains it and offers to open or download the image.
// * Upload forms ([data-dropzone]) are handled by upload.js: drag-and-drop,
//   paste and the tray for renaming and editing images before the upload.
// * Escape cancels inline editing; dismiss buttons; copy buttons for
//   one-time secrets (code.copyable).
// * Item cards ([data-menu]: set page, home, search) get a context menu on
//   right click, the context-menu key / Shift+F10 and their "⋯" button
//   (not in the grid view). Ctrl or Shift + right click keeps the browser's
//   own menu. Without this script the
//   browser menu stays. (The "View" options and the theme switch are in
//   prefs.js.)
(function () {
  "use strict";

  // Texts in the page's language (docs/languages.md): the server puts the
  // translations into <body data-i18n>; English needs none.
  var I18N = null;
  function tr(s) {
    if (I18N === null) {
      try {
        I18N = JSON.parse((document.body && document.body.getAttribute("data-i18n")) || "{}");
      } catch (e) {
        I18N = {};
      }
    }
    return I18N[s] || s;
  }
  function trf(s, args) {
    return tr(s).replace(/\{(\w+)\}/g, function (m, k) {
      return Object.prototype.hasOwnProperty.call(args, k) ? args[k] : m;
    });
  }

  function csrfToken() {
    return document.body ? document.body.getAttribute("data-csrf") : null;
  }

  // ---- status messages -------------------------------------------------

  // Two live regions, made when the page loads so screen readers know them
  // before the first message: #toast (polite status) and #toast-alert
  // (role=alert, for errors). Only one shows at a time.
  function toastRegion(isError) {
    var id = isError ? "toast-alert" : "toast";
    var el = document.getElementById(id);
    if (!el) {
      el = document.createElement("div");
      el.id = id;
      el.className = "toast";
      if (isError) {
        el.setAttribute("role", "alert");
      } else {
        el.setAttribute("role", "status");
        el.setAttribute("aria-live", "polite");
      }
      el.setAttribute("aria-atomic", "true");
      document.body.appendChild(el);
    }
    return el;
  }

  var toastTimer = null;
  var toastSeq = 0;
  function toast(message, isError) {
    var el = toastRegion(isError);
    var other = toastRegion(!isError);
    other.className = "toast";
    other.textContent = "";
    if (toastTimer) clearTimeout(toastTimer);
    // Emptied first, so the same message (a second copy) is read again.
    el.textContent = "";
    el.className = isError ? "toast show error" : "toast show";
    var seq = ++toastSeq;
    requestAnimationFrame(function () {
      if (seq === toastSeq) el.textContent = message;
    });
    toastTimer = setTimeout(function () {
      el.className = "toast";
    }, isError ? 8000 : 3500);
  }

  function flashButton(btn, text) {
    if (btn.classList.contains("quick-copy")) {
      // An icon: the check mark shows for a moment (app.css).
      btn.classList.add("done");
      setTimeout(function () {
        btn.classList.remove("done");
      }, 1500);
      return;
    }
    var original = btn.getAttribute("data-label") || btn.textContent;
    btn.setAttribute("data-label", original);
    btn.textContent = text;
    btn.classList.add("done");
    setTimeout(function () {
      btn.textContent = original;
      btn.classList.remove("done");
    }, 1500);
  }

  // ---- usage events ----------------------------------------------------

  function eventId() {
    if (window.crypto && crypto.randomUUID) return crypto.randomUUID();
    var bytes = new Uint8Array(16);
    crypto.getRandomValues(bytes);
    return Array.prototype.map
      .call(bytes, function (b) {
        return ("0" + b.toString(16)).slice(-2);
      })
      .join("");
  }

  // One event per image copy, like the desktop client's: `target` is the
  // profile the PNG follows ("generic", Other / browser), `variant` the file
  // it was made from. Link copies are not counted: no sticker was sent.
  function reportUsage(itemId, variant) {
    var headers = { "Content-Type": "application/json" };
    var token = csrfToken();
    if (token) headers["X-CSRF-Token"] = token;
    var body = {
      events: [
        {
          id: eventId(),
          item_id: itemId,
          copied_at: Math.floor(Date.now() / 1000),
          target: "generic",
          variant: variant || null,
          client: "web",
        },
      ],
    };
    fetch("/api/v1/usage", {
      method: "POST",
      credentials: "same-origin",
      headers: headers,
      body: JSON.stringify(body),
    })
      .then(function (r) {
        if (!r.ok) console.warn("stickpile: the copy was not recorded (" + r.status + ")");
      })
      .catch(function () {
        /* statistics are best effort */
      });
  }

  // ---- clipboard -------------------------------------------------------

  var MAC = /Mac|iPhone|iPad/.test(
    (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || ""
  );
  var COPY_KEYS = MAC ? "⌘C" : "Ctrl+C";
  var COPY_KEYS_ARIA = MAC ? "Meta+C" : "Control+C";

  // The item a copy control acts on: the nearest `.copy-controls`
  // (data-copy-*: item id, name, kind, the file to copy and its variant,
  // the thumbnail and the original), else the one in the control's card.
  function copyScope(el) {
    if (!el) return null;
    var scope = el.closest("[data-copy-item]");
    if (scope) return scope;
    var card = el.closest(".card");
    return card ? card.querySelector("[data-copy-item]") : null;
  }

  function copyInfo(scope) {
    function a(name) {
      return scope.getAttribute("data-copy-" + name) || "";
    }
    return {
      item: a("item"),
      name: a("name"),
      kind: a("kind") || "still",
      src: a("src"),
      variant: a("variant"),
      thumb: a("thumb"),
      original: a("original"),
    };
  }

  // "ok", or why this page cannot put images on the clipboard: "insecure"
  // (plain http://: browsers offer the Clipboard API only on https:// and
  // localhost) or "unsupported" (no ClipboardItem, or no image/png).
  function imageClipboard() {
    if (!window.isSecureContext) return "insecure";
    if (!navigator.clipboard || !navigator.clipboard.write || typeof window.ClipboardItem !== "function") {
      return "unsupported";
    }
    if (typeof ClipboardItem.supports === "function" && !ClipboardItem.supports("image/png")) return "unsupported";
    return "ok";
  }

  function fetchBlob(src) {
    return fetch(src, { credentials: "same-origin" }).then(function (r) {
      if (!r.ok) throw new Error(trf("the download failed (HTTP {status})", { status: r.status }));
      return r.blob();
    });
  }

  function canvasPng(source, width, height) {
    if (!width || !height) return Promise.reject(new Error(tr("the image has no size")));
    var canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    canvas.getContext("2d").drawImage(source, 0, 0, width, height);
    return new Promise(function (resolve, reject) {
      canvas.toBlob(function (png) {
        if (png) resolve(png);
        else reject(new Error(tr("the PNG conversion failed")));
      }, "image/png");
    });
  }

  // Decoding with an <img>, for browsers whose createImageBitmap refuses
  // the format.
  function imgPng(blob) {
    var url = URL.createObjectURL(blob);
    return new Promise(function (resolve, reject) {
      var img = new Image();
      img.onload = function () {
        resolve(canvasPng(img, img.naturalWidth, img.naturalHeight));
      };
      img.onerror = function () {
        reject(new Error(tr("the browser cannot decode this image")));
      };
      img.src = url;
    }).finally(function () {
      URL.revokeObjectURL(url);
    });
  }

  // `blob` as a PNG: a PNG as it is (unless `redraw`), anything else
  // decoded (an animation: its first frame) and encoded on a canvas.
  function imagePng(blob, redraw) {
    if (blob.type === "image/png" && !redraw) return Promise.resolve(blob);
    if (!window.createImageBitmap) return imgPng(blob);
    return createImageBitmap(blob, { imageOrientation: "from-image" })
      .catch(function () {
        return createImageBitmap(blob);
      })
      .then(
        function (bitmap) {
          return canvasPng(bitmap, bitmap.width, bitmap.height).finally(function () {
            if (bitmap.close) bitmap.close();
          });
        },
        function () {
          return imgPng(blob);
        }
      );
  }

  // The frame the thumbnail shows (10 % into the video), at full size.
  function videoPng(src) {
    return new Promise(function (resolve, reject) {
      var v = document.createElement("video");
      var finished = false;
      var timer = setTimeout(function () {
        finish(new Error(tr("the video took too long to load")));
      }, 20000);
      function finish(err) {
        if (finished) return;
        finished = true;
        clearTimeout(timer);
        if (err) reject(err);
        else resolve(canvasPng(v, v.videoWidth, v.videoHeight));
        setTimeout(function () {
          v.removeAttribute("src");
          v.load();
        }, 0);
      }
      v.muted = true;
      v.playsInline = true;
      v.preload = "auto";
      v.addEventListener("loadedmetadata", function () {
        var d = v.duration;
        v.currentTime = isFinite(d) && d > 0 ? d * 0.1 : 0;
      });
      v.addEventListener("seeked", function () {
        finish(null);
      });
      v.addEventListener("error", function () {
        finish(new Error(tr("the browser cannot play this video")));
      });
      v.src = src;
    });
  }

  // A still frame of an animation or video. If the browser cannot decode
  // the original, the thumbnail (the same frame, at most 256 px) is used,
  // and `used` says so.
  function framePng(info, used) {
    var full =
      info.kind === "video"
        ? videoPng(info.src)
        : fetchBlob(info.src).then(function (blob) {
            return imagePng(blob, true);
          });
    return full.catch(function () {
      used.variant = "thumb";
      used.small = true;
      return fetchBlob(info.thumb).then(function (blob) {
        return imagePng(blob, true);
      });
    });
  }

  function copyFailed(info, err) {
    var why = err && err.message ? err.message.replace(/\.$/, "") : String(err);
    if (err && err.name === "NotAllowedError") {
      why = tr("the browser did not allow it (the window must have focus, and the site needs clipboard access)");
    }
    return trf("Could not copy “{name}”: {reason}. Try again, or use Download.", { name: info.name, reason: why });
  }

  function copiedMessage(info, used) {
    var name = { name: info.name };
    if (info.kind === "still") return trf("Copied “{name}”. Paste it into your chat.", name);
    var video = info.kind === "video";
    if (used.small) {
      return video
        ? trf("Copied a small still frame of “{name}”: this browser cannot decode the video. Download keeps the video.", name)
        : trf("Copied a small still frame of “{name}”: this browser cannot decode the animation. Download keeps the animation.", name);
    }
    return video
      ? trf("Copied a still frame of “{name}” (browsers copy only still images). Download keeps the video.", name)
      : trf("Copied a still frame of “{name}” (browsers copy only still images). Download keeps the animation.", name);
  }

  // Where "Copied" shows when the copy came from the menu or the keyboard:
  // the card's quick copy button, else the scope's Copy button.
  function feedbackButton(scope) {
    var card = scope.closest(".card");
    return (card && card.querySelector(".quick-copy:not([hidden])")) || scope.querySelector("button[data-copy=image]");
  }

  var copying = {}; // item id -> true while its copy runs

  function copyImage(scope, btn) {
    var info = copyInfo(scope);
    var support = imageClipboard();
    if (support !== "ok") {
      copyHelp(info, support);
      return;
    }
    if (copying[info.item]) return;
    copying[info.item] = true;
    var shown = btn || feedbackButton(scope);
    if (shown) shown.classList.add("copying");
    var used = { variant: info.variant, small: false };
    var failure = null;
    var png = (
      info.kind === "still"
        ? fetchBlob(info.src).then(function (blob) {
            return imagePng(blob, false);
          })
        : framePng(info, used)
    ).catch(function (err) {
      failure = err;
      throw err;
    });
    var write;
    try {
      // The ClipboardItem is made right here, inside the click or key
      // press, holding a promise of the PNG: Safari requires that, and the
      // permission to write cannot expire while the file loads.
      write = navigator.clipboard.write([new ClipboardItem({ "image/png": png })]);
    } catch (e) {
      // Engines without promises in ClipboardItem want the blob itself.
      write = png.then(function (blob) {
        return navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
      });
    }
    write
      .then(
        function () {
          if (shown) flashButton(shown, tr("Copied ✓"));
          toast(copiedMessage(info, used));
          reportUsage(info.item, used.variant);
        },
        function (err) {
          toast(copyFailed(info, failure || err), true);
        }
      )
      .then(function () {
        delete copying[info.item];
        if (shown) shown.classList.remove("copying");
      });
  }

  // document.execCommand("copy") on a hidden text field: deprecated, but
  // the only way to copy text on plain http:// pages.
  function legacyCopy(text) {
    var field = document.createElement("textarea");
    field.value = text;
    field.setAttribute("readonly", "");
    field.setAttribute("aria-hidden", "true");
    field.className = "copy-buffer";
    var active = document.activeElement;
    document.body.appendChild(field);
    field.select();
    var ok = false;
    try {
      ok = document.execCommand("copy");
    } catch (e) {
      ok = false;
    }
    field.remove();
    if (active && active.focus) active.focus({ preventScroll: true });
    return ok ? Promise.resolve() : Promise.reject(new Error("copying text is not allowed here"));
  }

  function copyText(text) {
    if (window.isSecureContext && navigator.clipboard && navigator.clipboard.writeText) {
      return navigator.clipboard.writeText(text).catch(function () {
        return legacyCopy(text);
      });
    }
    return legacyCopy(text);
  }

  function copyLink(scope, btn) {
    var info = copyInfo(scope);
    var url = new URL(info.original, window.location.href).href;
    copyText(url).then(
      function () {
        if (btn) flashButton(btn, tr("Link copied ✓"));
        toast(tr("Link copied. It opens for people signed in to this server."));
      },
      function () {
        window.prompt(tr("Copy this link:"), url);
      }
    );
  }

  // Why copying cannot work on this page, and what to do instead: open the
  // image (the browser's own "Copy Image" works everywhere) or download it.
  function copyHelp(info, why) {
    var old = document.getElementById("copy-help");
    if (old) old.remove();
    var video = info.kind === "video";
    var d = document.createElement("dialog");
    d.id = "copy-help";
    d.className = "copy-help";
    d.setAttribute("aria-labelledby", "copy-help-title");
    d.setAttribute("aria-describedby", "copy-help-text");
    function add(tag, text, attrs) {
      var el = document.createElement(tag);
      if (text) el.textContent = text;
      Object.keys(attrs || {}).forEach(function (k) {
        el.setAttribute(k, attrs[k]);
      });
      return el;
    }
    var title = why === "insecure" ? tr("Copying images needs HTTPS") : tr("This browser cannot copy images");
    d.appendChild(add("h2", title, { id: "copy-help-title" }));
    var text = add("div", "", { id: "copy-help-text" });
    text.appendChild(
      add(
        "p",
        why === "insecure"
          ? trf("Browsers let web pages put images on the clipboard only on https:// addresses (and on localhost). This page was opened over plain http://, so stickpile cannot copy “{name}”.", { name: info.name })
          : trf("This browser does not let web pages put images on the clipboard, so stickpile cannot copy “{name}”.", {
              name: info.name,
            })
      )
    );
    text.appendChild(
      add(
        "p",
        video
          ? tr("Open the video, or download it and drag the file into your chat.")
          : tr("Open the image and use your browser's own “Copy Image” (right-click it), or download it and drag the file into your chat.")
      )
    );
    d.appendChild(text);
    if (why === "insecure") {
      d.appendChild(
        add(
          "p",
          tr("Copy works when stickpile is opened over https://, for example through tailscale serve. The desktop client copies without a browser, GIFs and videos included."),
          { class: "hint" }
        )
      );
    }
    var row = add("div", "", { class: "inline-row" });
    var open = add("a", video ? tr("Open video") : tr("Open image"), {
      class: "button primary",
      href: info.kind === "still" ? info.src : info.original,
      target: "_blank",
      rel: "noopener",
    });
    var download = add("a", tr("Download"), { class: "button", href: info.original, download: "" });
    var close = add("button", tr("Close"), { type: "button" });
    row.appendChild(open);
    row.appendChild(download);
    row.appendChild(close);
    d.appendChild(row);
    var back = document.activeElement;
    function done() {
      if (d.open) d.close();
    }
    close.addEventListener("click", done);
    open.addEventListener("click", function () {
      setTimeout(done, 0);
    });
    download.addEventListener("click", function () {
      setTimeout(done, 0);
    });
    d.addEventListener("close", function () {
      d.remove();
      if (back && document.contains(back) && back.focus) back.focus({ preventScroll: true });
    });
    document.body.appendChild(d);
    if (d.showModal) {
      d.showModal();
      open.focus();
    } else {
      // No <dialog> support: say it in the alert region instead.
      d.remove();
      toast(trf("{title}. Open the image or download it instead.", { title: title }), true);
    }
  }

  // Ctrl+C (⌘C) copies the focused card's item, or the item of an item page
  // when nothing in particular has focus. Not in text fields, and not when
  // text is selected (that is a text copy).
  function isCopyShortcut(e) {
    // Ctrl or ⌘, not both: whichever the platform copies with (the other
    // does nothing there).
    if (e.altKey || e.shiftKey || e.isComposing || e.ctrlKey === e.metaKey) return false;
    var k = (e.key || "").toLowerCase();
    // Other keyboard layouts (Cyrillic "с"): the key in the C position.
    return k === "c" || (e.code === "KeyC" && !/^[a-z]$/.test(k));
  }

  function shortcutScope(t) {
    if (editableTarget(t)) return null;
    var sel = window.getSelection ? window.getSelection() : null;
    if (sel && !sel.isCollapsed && String(sel).trim()) return null;
    var card = t.closest("[data-menu]");
    if (card) return card.querySelector("[data-copy-item]");
    if (t === document.body || t === document.documentElement || t.id === "main") {
      return document.querySelector("[data-copy-page]");
    }
    return null;
  }

  // ---- enhancement of new content ---------------------------------------

  function enhance(root) {
    if (!root || !root.querySelectorAll) return;
    root.querySelectorAll("button[data-copy][hidden]").forEach(function (b) {
      b.hidden = false;
      if (b.classList.contains("quick-copy")) b.title += " (" + COPY_KEYS + ")";
    });
    if (imageClipboard() === "insecure") {
      root.querySelectorAll("[data-copy-insecure][hidden]").forEach(function (p) {
        p.hidden = false;
      });
    }
    root.querySelectorAll("code.copyable").forEach(function (code) {
      if (code.nextElementSibling && code.nextElementSibling.classList.contains("copy-secret")) return;
      var b = document.createElement("button");
      b.type = "button";
      b.className = "small copy-secret";
      b.textContent = tr("Copy");
      b.addEventListener("click", function () {
        copyText(code.textContent.trim()).then(
          function () {
            flashButton(b, tr("Copied ✓"));
          },
          function () {
            var range = document.createRange();
            range.selectNodeContents(code);
            var sel = window.getSelection();
            sel.removeAllRanges();
            sel.addRange(range);
            toast(tr("Selected; press Ctrl+C to copy."), false);
          }
        );
      });
      code.insertAdjacentElement("afterend", b);
    });
    root.querySelectorAll("button[data-menu-button][hidden]").forEach(function (b) {
      b.hidden = false;
    });
  }

  // ---- item context menu (item cards) ------------------------------------

  // Menu icons: 24×24 outlines, like the server's (web/components.rs).
  var ICONS = {
    copy: ["M9 9h10v10H9z", "M5 15V5h10"],
    link: ["M10 14a4 4 0 0 0 5.66 0l3-3a4 4 0 0 0-5.66-5.66l-1 1", "M14 10a4 4 0 0 0-5.66 0l-3 3a4 4 0 0 0 5.66 5.66l1-1"],
    like: ["M12 20s-7-4.4-7-10a4 4 0 0 1 7-2.6A4 4 0 0 1 19 10c0 5.6-7 10-7 10z"],
    open: ["M14 4h6v6", "M20 4l-9 9", "M18 14v6H4V6h6"],
    download: ["M12 4v11", "M7 10l5 5 5-5", "M5 20h14"],
    edit: ["M4 20h4L19 9l-4-4L4 16z", "M13.5 6.5l4 4"],
    fields: ["M9 6h11", "M9 12h11", "M9 18h11", "M4 6h.01", "M4 12h.01", "M4 18h.01"],
    up: ["M12 19V5", "M6 11l6-6 6 6"],
    down: ["M12 5v14", "M6 13l6 6 6-6"],
    top: ["M5 4h14", "M12 20V9", "M7 14l5-5 5 5"],
    bottom: ["M5 20h14", "M12 4v11", "M7 10l5 5 5-5"],
    delete: ["M4 7h16", "M9 7V4h6v3", "M6 7l1 13h10l1-13", "M10 11v5", "M14 11v5"],
    cover: ["M12 4l2.4 5 5.4.6-4 3.7 1.1 5.4L12 16l-4.9 2.7 1.1-5.4-4-3.7 5.4-.6z"],
  };
  var SVG_NS = "http://www.w3.org/2000/svg";

  function svgIcon(name) {
    var svg = document.createElementNS(SVG_NS, "svg");
    svg.setAttribute("class", "icon menu-icon");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("aria-hidden", "true");
    svg.setAttribute("focusable", "false");
    svg.setAttribute("fill", "none");
    svg.setAttribute("stroke", "currentColor");
    svg.setAttribute("stroke-width", name === "fields" ? "2.4" : "2");
    svg.setAttribute("stroke-linecap", "round");
    svg.setAttribute("stroke-linejoin", "round");
    (ICONS[name] || []).forEach(function (d) {
      var path = document.createElementNS(SVG_NS, "path");
      path.setAttribute("d", d);
      svg.appendChild(path);
    });
    return svg;
  }

  var menu = null; // the open menu element
  var menuCard = null;
  var menuButton = null; // the "⋯" button that opened it, if any
  var menuReturn = null; // where focus goes back to
  var menuByKeyboard = false; // focus follows a moved item only then
  var keyboardOpenedAt = 0;
  var typeahead = "";
  var typeaheadTimer = null;

  function cardName(card) {
    var v = card.querySelector(".field-name .value");
    if (v && v.textContent.trim()) return v.textContent.trim();
    var img = card.querySelector(".thumb img");
    return img ? img.getAttribute("alt") || "" : "";
  }

  function htmxReady() {
    return !!(window.htmx && htmx.ajax);
  }

  // A plain form POST, for when htmx is not available.
  function postForm(url, fields) {
    var form = document.createElement("form");
    form.method = "post";
    form.action = url;
    form.hidden = true;
    var all = {};
    var token = csrfToken();
    if (token) all.csrf_token = token;
    Object.keys(fields).forEach(function (k) {
      all[k] = fields[k];
    });
    Object.keys(all).forEach(function (k) {
      var input = document.createElement("input");
      input.type = "hidden";
      input.name = k;
      input.value = all[k];
      form.appendChild(input);
    });
    document.body.appendChild(form);
    form.submit();
  }

  // After a move or delete the menu is gone: put focus on the item (and
  // scroll to it) when the menu was used from the keyboard; otherwise keep
  // the page where it is.
  function focusCard(id, scroll) {
    var card = document.getElementById(id);
    if (!card) return;
    var target = card.querySelector("button[data-menu-button]:not([hidden])") || card.querySelector(".thumb");
    if (target) target.focus({ preventScroll: !scroll });
  }

  function moveItem(card, dir, byKeyboard) {
    var url = card.getAttribute("data-move-url");
    var id = card.id;
    if (!htmxReady()) {
      postForm(url, { dir: dir }); // the server redirects back to the item
      return;
    }
    htmx
      .ajax("POST", url, { source: card, target: "#set-items", swap: "outerHTML", values: { dir: dir } })
      .then(function () {
        setTimeout(function () {
          focusCard(id, byKeyboard);
        }, 30);
      });
  }

  // The set's cover: this item, or (`clear`) back to the first item. The
  // grid comes back with the badge moved, in the page's sort. An item
  // replaces the set's cover image (`data-cover-image` on the grid), so
  // that asks first.
  function setCover(card, clear, byKeyboard) {
    var url = card.getAttribute("data-cover-url");
    var id = card.id;
    var item = clear ? "" : id.replace(/^item-/, "");
    if (
      !clear &&
      card.closest("[data-cover-image]") &&
      !window.confirm(trf("Use “{name}” as the cover? This removes the set's cover image.", { name: cardName(card) }))
    ) {
      focusCard(id, false);
      return;
    }
    if (!htmxReady()) {
      postForm(url, { item: item });
      return;
    }
    htmx.ajax("POST", url, { source: card, target: "#set-items", swap: "outerHTML", values: { item: item } }).then(
      function () {
        toast(clear ? tr("The first item is the cover again.") : trf("“{name}” is the cover now.", { name: cardName(card) }));
        setTimeout(function () {
          focusCard(id, byKeyboard);
        }, 30);
      }
    );
  }

  function deleteItem(card, byKeyboard) {
    var url = card.getAttribute("data-delete-url");
    if (!htmxReady()) {
      window.location.href = url; // the confirmation page
      return;
    }
    if (!window.confirm(trf("Delete “{name}” from this set?", { name: cardName(card) }))) {
      focusCard(card.id, false);
      return;
    }
    var next = card.nextElementSibling || card.previousElementSibling;
    var nextId = next ? next.id : null;
    htmx.ajax("POST", url, { source: card, target: card, swap: "delete" }).then(function () {
      setTimeout(function () {
        if (nextId && document.getElementById(nextId)) focusCard(nextId, byKeyboard);
        else {
          var head = document.getElementById("items-head");
          if (head) {
            head.setAttribute("tabindex", "-1");
            head.focus();
          }
        }
      }, 30);
    });
  }

  // The entries for one card: only what the server marked as allowed
  // (data-* attributes) and what the card itself offers (like, copy).
  function menuEntries(card) {
    var groups = [];
    var first = [];
    var scope = card.querySelector("[data-copy-item]");
    if (scope) {
      var frame = scope.getAttribute("data-copy-kind") !== "still";
      first.push({
        label: frame ? tr("Copy still frame") : tr("Copy image"),
        icon: "copy",
        keys: COPY_KEYS,
        ariaKeys: COPY_KEYS_ARIA,
        run: function () {
          copyImage(scope, null);
        },
      });
      if (frame) {
        first.push({
          label: tr("Copy link"),
          icon: "link",
          run: function () {
            copyLink(scope, null);
          },
        });
      }
    }
    var like = card.querySelector("form.like-form button.like");
    if (like) {
      var liked = like.getAttribute("aria-pressed") === "true";
      first.push({
        label: liked ? tr("Unlike") : tr("Like"),
        icon: "like",
        run: function () {
          like.click();
        },
      });
    }
    groups.push(first);
    var open = [];
    var page = card.getAttribute("data-item-url");
    if (page) open.push({ label: tr("Open item page"), icon: "open", href: page });
    var download = card.getAttribute("data-download-url");
    if (download) open.push({ label: tr("Download original"), icon: "download", href: download, download: true });
    groups.push(open);
    var edit = [];
    var rename = card.querySelector(".field-name a.edit");
    if (rename && htmxReady()) {
      edit.push({
        label: tr("Rename"),
        icon: "edit",
        run: function () {
          rename.click();
        },
        keepFocus: true,
      });
    }
    var editUrl = card.getAttribute("data-edit-url");
    if (editUrl) edit.push({ label: tr("Edit all fields…"), icon: "fields", href: editUrl });
    if (card.hasAttribute("data-cover-url")) {
      var isCover = card.hasAttribute("data-cover");
      edit.push({
        label: isCover ? tr("Stop using as cover") : tr("Use as cover"),
        icon: "cover",
        keepFocus: true,
        run: function (byKeyboard) {
          setCover(card, isCover, byKeyboard);
        },
      });
    }
    groups.push(edit);
    if (card.hasAttribute("data-move-url")) {
      var isFirst = card.hasAttribute("data-first");
      var isLast = card.hasAttribute("data-last");
      groups.push(
        [
          ["up", tr("Move up"), "up", isFirst],
          ["down", tr("Move down"), "down", isLast],
          ["top", tr("Move to top"), "top", isFirst],
          ["bottom", tr("Move to bottom"), "bottom", isLast],
        ].map(function (m) {
          return {
            label: m[1],
            icon: m[2],
            disabled: m[3],
            keepFocus: true,
            run: function (byKeyboard) {
              moveItem(card, m[0], byKeyboard);
            },
          };
        })
      );
    }
    if (card.hasAttribute("data-delete-url")) {
      groups.push([
        {
          label: tr("Delete…"),
          icon: "delete",
          danger: true,
          keepFocus: true,
          run: function (byKeyboard) {
            deleteItem(card, byKeyboard);
          },
        },
      ]);
    }
    return groups.filter(function (g) {
      return g.length > 0;
    });
  }

  function menuItemsOf(m) {
    return Array.prototype.slice.call(m.querySelectorAll("[role=menuitem]")).filter(function (el) {
      return !el.disabled;
    });
  }

  function focusMenuItem(index) {
    var items = menuItemsOf(menu);
    if (!items.length) return;
    var i = ((index % items.length) + items.length) % items.length;
    items[i].focus({ preventScroll: true });
  }

  function closeMenu(restoreFocus) {
    if (!menu) return;
    var ret = menuReturn;
    menu.remove();
    menu = null;
    menuCard = null;
    if (menuButton) {
      menuButton.setAttribute("aria-expanded", "false");
      menuButton.removeAttribute("aria-controls");
    }
    menuButton = null;
    menuReturn = null;
    if (restoreFocus && ret && document.contains(ret)) ret.focus({ preventScroll: true });
  }

  // `at`: {x, y} for a pointer, or a DOMRect to open next to.
  function openMenu(card, at, button, byKeyboard) {
    closeMenu(false);
    var name = cardName(card);
    var m = document.createElement("div");
    m.className = "ctx-menu";
    m.id = "item-menu";
    m.setAttribute("role", "menu");
    m.setAttribute("aria-label", trf("Actions for {name}", { name: name }));
    m.tabIndex = -1;
    var head = document.createElement("div");
    head.className = "menu-head";
    head.setAttribute("aria-hidden", "true");
    head.textContent = name;
    m.appendChild(head);
    menuEntries(card).forEach(function (group, gi) {
      if (gi > 0) {
        var sep = document.createElement("div");
        sep.setAttribute("role", "separator");
        m.appendChild(sep);
      }
      group.forEach(function (entry) {
        var el;
        if (entry.href) {
          el = document.createElement("a");
          el.href = entry.href;
          if (entry.download) el.setAttribute("download", "");
        } else {
          el = document.createElement("button");
          el.type = "button";
          if (entry.disabled) el.disabled = true;
        }
        el.setAttribute("role", "menuitem");
        el.tabIndex = -1;
        if (entry.danger) el.className = "menu-danger";
        el.setAttribute("data-label", entry.label.toLowerCase());
        el.appendChild(svgIcon(entry.icon));
        var label = document.createElement("span");
        label.textContent = entry.label;
        el.appendChild(label);
        if (entry.keys) {
          // The shortcut that does the same on the card (as native menus show it).
          var keys = document.createElement("span");
          keys.className = "menu-keys";
          keys.setAttribute("aria-hidden", "true");
          keys.textContent = entry.keys;
          el.appendChild(keys);
          el.setAttribute("aria-keyshortcuts", entry.ariaKeys);
        }
        el.addEventListener("click", function (e) {
          if (entry.href) {
            // Links navigate (or download) by themselves; close afterwards.
            setTimeout(function () {
              closeMenu(false);
            }, 0);
            return;
          }
          e.preventDefault();
          var byKeyboard = menuByKeyboard || e.detail === 0;
          closeMenu(!entry.keepFocus);
          entry.run(byKeyboard);
        });
        m.appendChild(el);
      });
    });
    document.body.appendChild(m);
    menu = m;
    menuCard = card;
    menuButton = button || null;
    menuByKeyboard = !!byKeyboard;
    var active = document.activeElement;
    // "⋯" if it is shown (the grid view has none), else the thumbnail.
    var shownButton = card.querySelector("button[data-menu-button]:not([hidden])");
    if (shownButton && !shownButton.getClientRects().length) shownButton = null;
    menuReturn =
      button ||
      (active && active !== card && card.contains(active) ? active : null) ||
      shownButton ||
      card.querySelector(".thumb");
    if (menuButton) {
      menuButton.setAttribute("aria-expanded", "true");
      menuButton.setAttribute("aria-controls", m.id);
    }
    // Position inside the viewport (CSSOM properties are allowed by the CSP).
    var w = m.offsetWidth;
    var h = m.offsetHeight;
    var vw = document.documentElement.clientWidth;
    var vh = document.documentElement.clientHeight;
    var x, y;
    if (at && typeof at.bottom === "number") {
      x = at.right - w;
      if (x < 4) x = at.left;
      y = at.bottom + 4;
      if (y + h > vh - 4) y = at.top - h - 4;
    } else {
      x = at.x;
      y = at.y;
      if (x + w > vw - 4) x = x - w;
      if (y + h > vh - 4) y = vh - h - 4;
    }
    x = Math.max(4, Math.min(x, vw - w - 4));
    y = Math.max(4, Math.min(y, vh - h - 4));
    m.style.left = x + "px";
    m.style.top = y + "px";
    focusMenuItem(0);
  }

  function menuKeydown(e) {
    var items = menuItemsOf(menu);
    var current = items.indexOf(document.activeElement);
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        focusMenuItem(current + 1);
        return;
      case "ArrowUp":
        e.preventDefault();
        focusMenuItem(current < 0 ? -1 : current - 1);
        return;
      case "Home":
      case "PageUp":
        e.preventDefault();
        focusMenuItem(0);
        return;
      case "End":
      case "PageDown":
        e.preventDefault();
        focusMenuItem(-1);
        return;
      case "Escape":
        e.preventDefault();
        closeMenu(true);
        return;
      case "Tab":
        e.preventDefault();
        closeMenu(true);
        return;
      case " ":
      case "Enter":
        if (document.activeElement && document.activeElement.tagName === "A") {
          e.preventDefault();
          document.activeElement.click();
        }
        return;
    }
    // Type-ahead: jump to the next entry starting with the typed letters.
    if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
      e.preventDefault();
      typeahead += e.key.toLowerCase();
      if (typeaheadTimer) clearTimeout(typeaheadTimer);
      typeaheadTimer = setTimeout(function () {
        typeahead = "";
      }, 700);
      // A repeated first letter cycles; more letters refine from here.
      var start = typeahead.length > 1 ? Math.max(current, 0) : current + 1;
      for (var k = 0; k < items.length; k++) {
        var item = items[(start + k) % items.length];
        if ((item.getAttribute("data-label") || "").indexOf(typeahead) === 0) {
          item.focus({ preventScroll: true });
          break;
        }
      }
    }
  }

  function editableTarget(t) {
    return !!t.closest("input, textarea, select, [contenteditable]");
  }

  // ---- global listeners ------------------------------------------------

  document.addEventListener("click", function (e) {
    var t = e.target;
    if (!(t instanceof Element)) return;
    var menuBtn = t.closest("button[data-menu-button]");
    if (menuBtn) {
      var card = menuBtn.closest("[data-menu]");
      if (!card) return;
      e.preventDefault();
      if (menu && menuCard === card) closeMenu(true);
      else openMenu(card, menuBtn.getBoundingClientRect(), menuBtn, e.detail === 0);
      return;
    }
    var copy = t.closest("button[data-copy]");
    if (copy) {
      e.preventDefault();
      var scope = copyScope(copy);
      if (!scope) return;
      if (copy.getAttribute("data-copy") === "link") copyLink(scope, copy);
      else copyImage(scope, copy);
      return;
    }
    var dismiss = t.closest("[data-dismiss]");
    if (dismiss) {
      var box = dismiss.closest(".alert");
      if (box) box.remove();
    }
  });

  document.addEventListener("contextmenu", function (e) {
    var t = e.target;
    if (!(t instanceof Element)) return;
    if (menu && menu.contains(t)) {
      e.preventDefault();
      return;
    }
    // Just opened from the keyboard (some browsers still send the event).
    if (Date.now() - keyboardOpenedAt < 800 && menu) {
      e.preventDefault();
      return;
    }
    if (e.ctrlKey || e.shiftKey || e.metaKey || e.altKey) return; // the browser's menu
    var card = t.closest("[data-menu]");
    if (!card || editableTarget(t)) return;
    e.preventDefault();
    if (e.clientX === 0 && e.clientY === 0) {
      // Keyboard-triggered without a keydown we saw: open next to the focus.
      openMenu(card, (document.activeElement || t).getBoundingClientRect(), null, true);
    } else {
      openMenu(card, { x: e.clientX, y: e.clientY }, null);
    }
  });

  // Close on a press outside the menu, on scrolling and resizing.
  document.addEventListener(
    "pointerdown",
    function (e) {
      if (menu && !(e.target instanceof Element && menu.contains(e.target))) {
        var onButton = e.target instanceof Element && e.target.closest("button[data-menu-button]");
        if (!onButton || onButton !== menuButton) closeMenu(false);
      }
    },
    true
  );
  window.addEventListener(
    "scroll",
    function () {
      closeMenu(false);
    },
    true
  );
  window.addEventListener("resize", function () {
    closeMenu(false);
  });
  window.addEventListener("blur", function () {
    closeMenu(false);
  });
  document.addEventListener("htmx:beforeSwap", function (e) {
    // The card the menu belongs to may be replaced.
    if (menu && menuCard && e.detail && e.detail.target && e.detail.target.contains(menuCard)) closeMenu(false);
  });

  document.addEventListener("keydown", function (e) {
    var t = e.target;
    if (!(t instanceof Element)) return;
    if (menu && menu.contains(t)) {
      menuKeydown(e);
      return;
    }
    var menuKey = e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey);
    if (menuKey) {
      var card = t.closest("[data-menu]");
      if (card && !editableTarget(t)) {
        e.preventDefault();
        keyboardOpenedAt = Date.now();
        var btn = t.closest("button[data-menu-button]");
        openMenu(card, t.getBoundingClientRect(), btn, true);
      }
      return;
    }
    if (isCopyShortcut(e)) {
      var copyFrom = shortcutScope(t);
      if (copyFrom) {
        e.preventDefault();
        if (!e.repeat) copyImage(copyFrom, null);
      }
      return;
    }
    if ((e.key === "ArrowDown" || e.key === "ArrowUp") && t.matches("button[data-menu-button]")) {
      // Menu button pattern: arrows open the menu on the first / last entry.
      var c = t.closest("[data-menu]");
      if (c) {
        e.preventDefault();
        openMenu(c, t.getBoundingClientRect(), t, true);
        if (e.key === "ArrowUp") focusMenuItem(-1);
      }
      return;
    }
    if (e.key !== "Escape") return;
    var editing = t.closest(".field.editing");
    if (editing) {
      var cancel = editing.querySelector(".cancel");
      if (cancel) {
        e.preventDefault();
        cancel.click();
      }
    }
  });

  document.addEventListener("htmx:load", function (e) {
    enhance(e.target);
  });
  document.addEventListener("htmx:beforeRequest", function () {
    var flash = document.getElementById("flash");
    if (flash) flash.textContent = "";
  });
  document.addEventListener("htmx:sendError", function () {
    toast(tr("The server could not be reached. Check your connection and try again."), true);
  });

  toastRegion(false);
  toastRegion(true);
  enhance(document);
})();
