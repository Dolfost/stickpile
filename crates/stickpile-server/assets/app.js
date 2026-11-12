// stickpile web UI enhancements. Plain JavaScript, no build step, loaded
// with `defer` from /static/app.js (the CSP allows no inline scripts).
//
// * Copy buttons (hidden until this script runs):
//   - stills: fetch the PNG (or convert the original to PNG) and put it on
//     the clipboard as image/png;
//   - animations and videos: copy the absolute file URL (browsers cannot
//     put GIF or video files on the clipboard in a form chat apps accept).
//   A successful copy is reported as a usage event (POST /api/v1/usage,
//   client "web"). The Clipboard API needs a secure context (https or
//   localhost); elsewhere a message explains the alternatives.
// * Upload forms ([data-dropzone]) are handled by upload.js: drag-and-drop,
//   paste and the tray for renaming and editing images before the upload.
// * Escape cancels inline editing; dismiss buttons; copy buttons for
//   one-time secrets (code.copyable).
// * Set page: item cards ([data-menu]) get a context menu on right click,
//   the context-menu key / Shift+F10 and their "⋯" button. Ctrl or Shift +
//   right click keeps the browser's own menu. Without this script the
//   browser menu stays. (The "View" options and the theme switch are in
//   prefs.js.)
(function () {
  "use strict";

  function csrfToken() {
    return document.body ? document.body.getAttribute("data-csrf") : null;
  }

  // ---- status messages -------------------------------------------------

  var toastTimer = null;
  function toast(message, isError) {
    var el = document.getElementById("toast");
    if (!el) {
      el = document.createElement("div");
      el.id = "toast";
      el.setAttribute("role", "status");
      el.setAttribute("aria-live", "polite");
      document.body.appendChild(el);
    }
    el.textContent = message;
    el.className = isError ? "toast show error" : "toast show";
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(function () {
      el.className = "toast";
    }, isError ? 7000 : 2500);
  }

  function flashButton(btn, text) {
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
          target: null,
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
    }).catch(function () {
      /* statistics are best effort */
    });
  }

  // ---- clipboard -------------------------------------------------------

  var INSECURE =
    "Copying needs a secure connection (https:// or localhost). Use Download instead, " +
    "or the stickpile desktop client, which pastes into chat apps directly.";

  function toPng(blob) {
    if (blob.type === "image/png") return Promise.resolve(blob);
    return createImageBitmap(blob).then(function (bitmap) {
      var canvas = document.createElement("canvas");
      canvas.width = bitmap.width;
      canvas.height = bitmap.height;
      canvas.getContext("2d").drawImage(bitmap, 0, 0);
      return new Promise(function (resolve, reject) {
        canvas.toBlob(function (png) {
          if (png) resolve(png);
          else reject(new Error("PNG conversion failed"));
        }, "image/png");
      });
    });
  }

  function fetchPng(src) {
    return fetch(src, { credentials: "same-origin" })
      .then(function (r) {
        if (!r.ok) throw new Error("download failed (" + r.status + ")");
        return r.blob();
      })
      .then(toPng);
  }

  function copyImage(btn) {
    var src = btn.getAttribute("data-src");
    if (!window.isSecureContext || !navigator.clipboard || !window.ClipboardItem) {
      toast(INSECURE, true);
      return;
    }
    // Hand the clipboard a promise so the write starts inside the click
    // (Safari requires that); fall back to awaiting the blob.
    var pending = fetchPng(src);
    var write;
    try {
      write = navigator.clipboard.write([new ClipboardItem({ "image/png": pending })]);
    } catch (e) {
      write = pending.then(function (blob) {
        return navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
      });
    }
    write
      .then(function () {
        flashButton(btn, "Copied ✓");
        toast("Image copied. Paste it into your chat.");
        reportUsage(btn.getAttribute("data-item"), btn.getAttribute("data-variant"));
      })
      .catch(function (err) {
        toast("Could not copy the image: " + (err && err.message ? err.message : err) + ". Try Download.", true);
      });
  }

  function copyText(text) {
    if (window.isSecureContext && navigator.clipboard) {
      return navigator.clipboard.writeText(text);
    }
    return Promise.reject(new Error(INSECURE));
  }

  function copyLink(btn) {
    var url = new URL(btn.getAttribute("data-src"), window.location.href).href;
    copyText(url)
      .then(function () {
        flashButton(btn, "Link copied ✓");
        toast("Link copied. It opens for people signed in to this server.");
        reportUsage(btn.getAttribute("data-item"), btn.getAttribute("data-variant"));
      })
      .catch(function () {
        window.prompt("Copy this link:", url);
      });
  }

  // ---- enhancement of new content ---------------------------------------

  function enhance(root) {
    if (!root || !root.querySelectorAll) return;
    root.querySelectorAll("button[data-copy][hidden]").forEach(function (b) {
      b.hidden = false;
    });
    root.querySelectorAll("code.copyable").forEach(function (code) {
      if (code.nextElementSibling && code.nextElementSibling.classList.contains("copy-secret")) return;
      var b = document.createElement("button");
      b.type = "button";
      b.className = "small copy-secret";
      b.textContent = "Copy";
      b.addEventListener("click", function () {
        copyText(code.textContent.trim()).then(
          function () {
            flashButton(b, "Copied ✓");
          },
          function () {
            var range = document.createRange();
            range.selectNodeContents(code);
            var sel = window.getSelection();
            sel.removeAllRanges();
            sel.addRange(range);
            toast("Selected; press Ctrl+C to copy.", false);
          }
        );
      });
      code.insertAdjacentElement("afterend", b);
    });
    root.querySelectorAll("button[data-menu-button][hidden]").forEach(function (b) {
      b.hidden = false;
    });
  }

  // ---- item context menu (set page) --------------------------------------

  // Menu icons: 24×24 outlines, like the server's (web/components.rs).
  var ICONS = {
    copy: ["M9 9h10v10H9z", "M5 15V5h10"],
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
  // grid comes back with the badge moved, in the page's sort.
  function setCover(card, clear, byKeyboard) {
    var url = card.getAttribute("data-cover-url");
    var id = card.id;
    var item = clear ? "" : id.replace(/^item-/, "");
    if (!htmxReady()) {
      postForm(url, { item: item });
      return;
    }
    htmx.ajax("POST", url, { source: card, target: "#set-items", swap: "outerHTML", values: { item: item } }).then(
      function () {
        toast(clear ? "The first item is the cover again." : "“" + cardName(card) + "” is the cover now.");
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
    if (!window.confirm("Delete “" + cardName(card) + "” from this set?")) {
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
    var copy = card.querySelector("button[data-copy]");
    if (copy) {
      first.push({
        label: copy.getAttribute("data-copy") === "image" ? "Copy image" : "Copy link",
        icon: "copy",
        run: function () {
          copy.click();
        },
      });
    }
    var like = card.querySelector("form.like-form button.like");
    if (like) {
      var liked = like.getAttribute("aria-pressed") === "true";
      first.push({
        label: liked ? "Unlike" : "Like",
        icon: "like",
        run: function () {
          like.click();
        },
      });
    }
    groups.push(first);
    var open = [];
    var page = card.getAttribute("data-item-url");
    if (page) open.push({ label: "Open item page", icon: "open", href: page });
    var download = card.getAttribute("data-download-url");
    if (download) open.push({ label: "Download original", icon: "download", href: download, download: true });
    groups.push(open);
    var edit = [];
    var rename = card.querySelector(".field-name a.edit");
    if (rename && htmxReady()) {
      edit.push({
        label: "Rename",
        icon: "edit",
        run: function () {
          rename.click();
        },
        keepFocus: true,
      });
    }
    var editUrl = card.getAttribute("data-edit-url");
    if (editUrl) edit.push({ label: "Edit all fields…", icon: "fields", href: editUrl });
    if (card.hasAttribute("data-cover-url")) {
      var isCover = card.hasAttribute("data-cover");
      edit.push({
        label: isCover ? "Stop using as cover" : "Use as cover",
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
          ["up", "Move up", "up", isFirst],
          ["down", "Move down", "down", isLast],
          ["top", "Move to top", "top", isFirst],
          ["bottom", "Move to bottom", "bottom", isLast],
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
          label: "Delete…",
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
    m.setAttribute("aria-label", "Actions for " + name);
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
    menuReturn =
      button ||
      (active && active !== card && card.contains(active) ? active : null) ||
      card.querySelector("button[data-menu-button]:not([hidden])") ||
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
      if (copy.getAttribute("data-copy") === "image") copyImage(copy);
      else copyLink(copy);
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
    toast("The server could not be reached. Check your connection and try again.", true);
  });

  enhance(document);
})();
