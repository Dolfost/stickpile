// stickpile display preferences: the colour theme and the "View" options of
// the set page and of set lists. Plain JavaScript, loaded from /static/
// WITHOUT defer (the CSP allows no inline scripts), so it runs before the
// first paint wherever it is included:
//
// * In <head>: the theme. Signed in, the server renders <html data-theme>
//   from the account ("light" or "dark"; nothing for "system", which
//   follows prefers-color-scheme) plus <meta name="stickpile-theme">; that
//   choice is remembered in localStorage (stickpile.theme) and applied here
//   on pages without a session, such as the sign-in page.
// * As the first child of an element with data-view-prefs="items" (the set
//   page's #items-section), "home" (the start page's recently copied items)
//   or "sets" (set lists): the remembered display options become classes on
//   that element (a style class, `hide-<part>`) before the rest of it is
//   parsed, so nothing shows up and vanishes again. On the set page a `view`
//   URL parameter wins and is remembered.
// * Once the document is parsed: the "View" controls
//   (details[data-view-menu]) apply a change at once and remember it, and
//   keep the set page's `?view=` in the address bar in sync; the top bar's
//   switch (button[data-theme-toggle]) flips light and dark from the theme
//   in effect, applies it at once and saves it (PATCH /api/v1/me with the
//   CSRF header when signed in; localStorage only when signed out).
//
// Without this script: the account's theme, every field shown, and the set
// page's style through its filter form (`?view=`).
(function () {
  "use strict";

  var root = document.documentElement;

  // Texts in the page's language (docs/languages.md): the server puts the
  // translations into <body data-i18n>; English needs none. Read when a
  // text is needed: this script starts in <head>, before <body> exists.
  var I18N = null;
  function tr(s) {
    if (I18N === null) {
      if (!document.body) return s;
      try {
        I18N = JSON.parse(document.body.getAttribute("data-i18n") || "{}");
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

  function load(key) {
    try {
      return window.localStorage.getItem(key);
    } catch (e) {
      return null;
    }
  }

  function store(key, value) {
    try {
      window.localStorage.setItem(key, value);
    } catch (e) {
      /* private mode or storage disabled: not remembered */
    }
  }

  function has(obj, key) {
    return Object.prototype.hasOwnProperty.call(obj, key);
  }

  // ---- display options ---------------------------------------------------

  // The notes of styles that turn toggles off; functions, so the text is
  // looked up when shown (see tr).
  function compactNote() {
    return tr("Compact shows the name and the emojis.");
  }
  function gridNote() {
    return tr("Grid shows the thumbnails only.");
  }

  var VIEWS = {
    items: {
      styleKey: "stickpile.items.view",
      // The old "Compact view" checkbox: "compact" or "normal".
      legacyKey: "stickpile.setView",
      hideKey: "stickpile.items.hide",
      // style -> class on the element (none for regular)
      styles: { regular: "", compact: "compact", grid: "view-grid" },
      parts: ["emojis", "keywords", "vibe", "tags", "labels"],
      param: "view",
      // Toggles a style has no room for, and why.
      off: {
        compact: { parts: ["keywords", "vibe", "tags"], note: compactNote },
        grid: { parts: ["emojis", "keywords", "vibe", "tags"], note: gridNote },
      },
    },
    // The start page's recently copied items: the set page's cards with
    // their set, remembered apart from the set pages.
    home: {
      styleKey: "stickpile.home.view",
      legacyKey: null,
      hideKey: "stickpile.home.hide",
      styles: { regular: "", compact: "compact", grid: "view-grid" },
      parts: ["set", "emojis", "keywords", "vibe", "tags", "labels"],
      param: null,
      off: {
        compact: { parts: ["set", "keywords", "vibe", "tags"], note: compactNote },
        grid: { parts: ["set", "emojis", "keywords", "vibe", "tags"], note: gridNote },
      },
    },
    sets: {
      styleKey: "stickpile.sets.view",
      legacyKey: null,
      hideKey: "stickpile.sets.hide",
      styles: { regular: "", compact: "compact" },
      parts: ["cover", "description", "count", "badges", "owner", "slug"],
      param: null,
      off: {},
    },
  };

  function urlStyle(v) {
    if (!v.param) return null;
    var p = new URLSearchParams(window.location.search).get(v.param);
    if (p === null) return null;
    return has(v.styles, p) ? p : "regular"; // the server shows anything else as regular
  }

  function storedStyle(v) {
    var s = load(v.styleKey);
    if (s === null && v.legacyKey) {
      var old = load(v.legacyKey);
      if (old !== null) s = old === "compact" ? "compact" : "regular";
    }
    return s !== null && has(v.styles, s) ? s : null;
  }

  function storedHidden(v) {
    return (load(v.hideKey) || "").split(/\s+/).filter(function (p) {
      return v.parts.indexOf(p) >= 0;
    });
  }

  function renderedStyle(v, el) {
    for (var s in v.styles) {
      if (has(v.styles, s) && v.styles[s] && el.classList.contains(v.styles[s])) return s;
    }
    return "regular";
  }

  // The style to show: the URL's (remembered from now on), else the
  // remembered one, else what the server rendered.
  function currentStyle(v, el) {
    var s = urlStyle(v);
    if (s !== null) {
      store(v.styleKey, s);
      return s;
    }
    s = storedStyle(v);
    return s !== null ? s : renderedStyle(v, el);
  }

  function apply(el, v, style, hidden) {
    Object.keys(v.styles).forEach(function (s) {
      if (v.styles[s]) el.classList.toggle(v.styles[s], s === style);
    });
    v.parts.forEach(function (p) {
      el.classList.toggle("hide-" + p, hidden.indexOf(p) >= 0);
    });
  }

  // Keep `?view=` in the address bar, so a reload, the filter form's Apply
  // and htmx requests (HX-Current-URL) agree with what is shown.
  function syncUrl(param, style) {
    if (!window.history || !history.replaceState) return;
    var url = new URL(window.location.href);
    if (style === "regular") url.searchParams.delete(param);
    else url.searchParams.set(param, style);
    if (url.href !== window.location.href) history.replaceState(history.state, "", url.href);
  }

  // Keep an open panel inside the window (CSSOM properties are allowed by
  // the CSP); by default it hangs from the right edge of its button.
  function placePanel(menu) {
    var panel = menu.querySelector(".view-panel");
    if (!panel) return;
    panel.style.left = "";
    panel.style.right = "";
    var r = panel.getBoundingClientRect();
    var vw = document.documentElement.clientWidth;
    if (r.left < 8) {
      panel.style.right = "auto";
      panel.style.left = 8 - menu.getBoundingClientRect().left + "px";
    } else if (r.right > vw - 8) {
      panel.style.right = r.right - (vw - 8) + "px";
    }
  }

  function setupViewMenu(menu) {
    var kind = menu.getAttribute("data-view-menu");
    var v = has(VIEWS, kind) ? VIEWS[kind] : null;
    if (!v) return;
    var hosts = Array.prototype.slice.call(document.querySelectorAll('[data-view-prefs="' + kind + '"]'));
    var style = hosts.length ? currentStyle(v, hosts[0]) : storedStyle(v) || "regular";
    var hidden = storedHidden(v);
    var radios = menu.querySelectorAll('.view-style input[type="radio"]');
    var boxes = menu.querySelectorAll("input[data-view-part]");
    var note = menu.querySelector("[data-view-note]");

    function render() {
      hosts.forEach(function (h) {
        apply(h, v, style, hidden);
      });
      radios.forEach(function (r) {
        r.checked = r.value === style;
      });
      var off = v.off[style] || { parts: [], note: null };
      boxes.forEach(function (b) {
        b.checked = hidden.indexOf(b.value) < 0;
        b.disabled = off.parts.indexOf(b.value) >= 0;
      });
      if (note) {
        var text = off.note ? off.note() : "";
        note.textContent = text;
        note.hidden = !text;
      }
      if (v.param) syncUrl(v.param, style);
    }

    menu.hidden = false;
    var show = menu.querySelector("[data-view-show]");
    if (show) show.hidden = false;
    render();

    radios.forEach(function (r) {
      r.addEventListener("change", function () {
        if (!r.checked) return;
        style = r.value;
        store(v.styleKey, style);
        render();
      });
    });
    boxes.forEach(function (b) {
      b.addEventListener("change", function () {
        hidden = v.parts.filter(function (p) {
          return p === b.value ? !b.checked : hidden.indexOf(p) >= 0;
        });
        store(v.hideKey, hidden.join(" "));
        render();
      });
    });
    menu.addEventListener("toggle", function () {
      if (menu.open) placePanel(menu);
    });
    // A whole option is its label, so a press anywhere on it selects it.
    // But a press on the text, the hint or the padding (nothing focusable)
    // moves the focus to the closest focusable ancestor, <main>, before the
    // label's click arrives: the panel took that for "focus left" and
    // closed under the pointer, and the click landed on the page. Keep the
    // focus where it is on a press inside the panel (the click still
    // selects), then put it on the chosen option, so the keyboard carries
    // on there and the panel stays open for more changes.
    var panel = menu.querySelector(".view-panel");
    if (panel) {
      panel.addEventListener("mousedown", function (e) {
        if (e.target instanceof Element && !e.target.closest("input")) e.preventDefault();
      });
      panel.addEventListener("click", function (e) {
        var t = e.target instanceof Element ? e.target : null;
        var label = t && t.closest("label");
        var input = t && (t.closest("input") || (label && label.querySelector("input")));
        if (input && !input.disabled && document.activeElement !== input) input.focus({ preventScroll: true });
      });
    }
    // Escape closes the panel and returns to its button.
    menu.addEventListener("keydown", function (e) {
      if (e.key !== "Escape" || !menu.open) return;
      e.preventDefault();
      e.stopPropagation();
      menu.open = false;
      var summary = menu.querySelector("summary");
      if (summary) summary.focus();
    });
    // Focus moving elsewhere closes it too (Tab out of the panel); not to a
    // container of the panel, which only means a press on something that
    // takes no focus. Presses outside are handled on pointerdown (below).
    menu.addEventListener("focusout", function (e) {
      var to = e.relatedTarget;
      if (menu.open && to instanceof Element && !menu.contains(to) && !to.contains(menu)) menu.open = false;
    });
  }

  // ---- theme -------------------------------------------------------------

  var THEME_KEY = "stickpile.theme";
  var prefersDark = window.matchMedia ? window.matchMedia("(prefers-color-scheme: dark)") : null;

  // The signed-in account's theme ("system", "light", "dark"), else null.
  function accountTheme() {
    var m = document.querySelector('meta[name="stickpile-theme"]');
    return m ? m.getAttribute("content") : null;
  }

  // "light", "dark", or anything else for the device's setting.
  function setTheme(theme) {
    var fixed = theme === "light" || theme === "dark";
    if (fixed) root.setAttribute("data-theme", theme);
    else root.removeAttribute("data-theme");
    var meta = document.querySelector('meta[name="color-scheme"]');
    if (meta) meta.setAttribute("content", fixed ? theme : "light dark");
  }

  function effectiveTheme() {
    var t = root.getAttribute("data-theme");
    if (t === "light" || t === "dark") return t;
    return prefersDark && prefersDark.matches ? "dark" : "light";
  }

  // An error in the page's status region (app.js handles "Dismiss").
  function flash(message) {
    var region = document.getElementById("flash");
    if (!region) return;
    var box = document.createElement("div");
    box.className = "alert error";
    box.setAttribute("role", "alert");
    box.appendChild(document.createTextNode(message + " "));
    var dismiss = document.createElement("button");
    dismiss.type = "button";
    dismiss.className = "link dismiss";
    dismiss.setAttribute("data-dismiss", "");
    dismiss.textContent = tr("Dismiss");
    box.appendChild(dismiss);
    region.textContent = "";
    region.appendChild(box);
  }

  function saveTheme(theme) {
    var headers = { "Content-Type": "application/json" };
    var token = document.body ? document.body.getAttribute("data-csrf") : null;
    if (token) headers["X-CSRF-Token"] = token;
    fetch("/api/v1/me", {
      method: "PATCH",
      credentials: "same-origin",
      headers: headers,
      body: JSON.stringify({ theme: theme }),
    })
      .then(function (r) {
        if (!r.ok) throw new Error(trf("the server answered {status}", { status: r.status }));
        var m = document.querySelector('meta[name="stickpile-theme"]');
        if (m) m.setAttribute("content", theme);
      })
      .catch(function (err) {
        flash(
          trf("The theme could not be saved ({error}); it changes back when the page reloads.", {
            error: err && err.message ? err.message : err,
          })
        );
      });
  }

  function setupThemeSwitch() {
    var buttons = document.querySelectorAll("button[data-theme-toggle]");
    if (!buttons.length) return;
    // The button is "Dark theme", pressed when the page is dark; without
    // JavaScript its form sends the other theme in the hidden field.
    function sync() {
      var dark = effectiveTheme() === "dark";
      buttons.forEach(function (b) {
        b.hidden = false;
        b.setAttribute("aria-pressed", dark ? "true" : "false");
        b.title = dark ? tr("Switch to the light theme") : tr("Switch to the dark theme");
        var field = b.form ? b.form.querySelector('input[name="theme"]') : null;
        if (field) field.value = dark ? "light" : "dark";
      });
    }
    sync();
    if (prefersDark) {
      if (prefersDark.addEventListener) prefersDark.addEventListener("change", sync);
      else if (prefersDark.addListener) prefersDark.addListener(sync);
    }
    buttons.forEach(function (b) {
      b.addEventListener("click", function (e) {
        e.preventDefault();
        var next = effectiveTheme() === "dark" ? "light" : "dark";
        setTheme(next);
        store(THEME_KEY, next);
        sync();
        // The settings page's choice follows.
        var radio = document.querySelector('input[type="radio"][name="theme"][value="' + next + '"]');
        if (radio) radio.checked = true;
        if (accountTheme() !== null) saveTheme(next);
      });
    });
  }

  // ---- start -------------------------------------------------------------

  var script = document.currentScript;
  var host = script ? script.parentElement : null;
  var kind = host ? host.getAttribute("data-view-prefs") : null;
  if (kind && has(VIEWS, kind)) {
    var v = VIEWS[kind];
    apply(host, v, currentStyle(v, host), storedHidden(v));
    return;
  }

  // In <head>: the theme now, the controls once the document is parsed.
  var account = accountTheme();
  if (account !== null) store(THEME_KEY, account);
  else setTheme(load(THEME_KEY));

  function ready() {
    setupThemeSwitch();
    document.querySelectorAll("details[data-view-menu]").forEach(setupViewMenu);
    document.addEventListener(
      "pointerdown",
      function (e) {
        document.querySelectorAll("details[data-view-menu][open]").forEach(function (m) {
          if (!(e.target instanceof Node && m.contains(e.target))) m.open = false;
        });
      },
      true
    );
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", ready);
  else ready();
})();
