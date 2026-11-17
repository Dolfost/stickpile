// stickpile search suggestions. Plain JavaScript, no build step, loaded
// with `defer` from /static/suggest.js (the CSP allows no inline scripts).
//
// The search fields marked with [data-search-suggest] (the search page's
// field and the one in the top bar) offer, while a `key:value` filter is
// typed, the values the desktop popups offer under their search field:
// the vibes, tags, sets and owners of the items you can search (most used
// first), the words of type:, is: and sort:, and for e: emojis with their
// names (e: alone: your emojis; e:sk: the emojis named so, yours first).
// The matching is the server's (GET /api/v1/search/suggest, the popups'
// Rust code), never done here.
//
// The field is a combobox and the list under it its listbox (the focus
// stays in the field; the markup and look are the emoji suggestions' of
// emoji.js). Up and Down move the highlight, which starts on the value a
// half-typed filter stands for; Tab, or Enter on a highlighted value, puts
// it in place of what is typed (an emoji in place of its name) and a
// space; a click too. Enter without a highlight searches as before.
// Escape, a space, moving away or leaving the field close the list.
// Without JavaScript the fields work as plain search fields.
(function () {
  "use strict";

  var URL_SUGGEST = "/api/v1/search/suggest";
  var DELAY = 120; // ms of quiet typing before asking the server
  var nextId = 1;
  var current = null; // the field whose list is open

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
  // A count: `forms` is tr() of a plural key, "{n} value|{n} values", whose
  // translation has the language's forms (Ukrainian: one|few|many), chosen
  // as the server's stickpile_i18n::plural_form does; {n} is filled in.
  function trp(n, forms) {
    var f = forms.split("|");
    var i = n === 1 ? 0 : 1;
    if (document.documentElement.lang === "uk") {
      var m10 = n % 10;
      var m100 = n % 100;
      i = m10 === 1 && m100 !== 11 ? 0 : m10 >= 2 && m10 <= 4 && (m100 < 12 || m100 > 14) ? 1 : 2;
    }
    return f[Math.min(i, f.length - 1)].replace(/\{n\}/g, n);
  }

  function el(tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  // The length in UTF-8 bytes of `s`: the server counts the cursor so.
  function utf8Length(s) {
    return new TextEncoder().encode(s).length;
  }

  function attach(input) {
    if (input.stickpileSuggest) return;
    var id = "search-suggest-" + nextId++;
    var box = el("div", "emoji-suggest search-suggest");
    box.hidden = true;
    var note = el("p", "emoji-suggest-note");
    var list = el("ul", "emoji-suggest-list");
    list.id = id;
    list.setAttribute("role", "listbox");
    list.setAttribute("aria-label", tr("Search suggestions"));
    var keys = el("p", "emoji-suggest-keys", tr("Up and Down choose, Tab or Enter puts it in, Esc closes"));
    keys.setAttribute("aria-hidden", "true");
    box.appendChild(note);
    box.appendChild(list);
    box.appendChild(keys);
    // The count for screen readers (the field keeps the focus).
    var live = el("span", "sr-only");
    live.setAttribute("role", "status");
    live.setAttribute("aria-live", "polite");
    // In <body>, so no ancestor of the field clips or moves the list.
    document.body.appendChild(box);
    document.body.appendChild(live);

    input.setAttribute("role", "combobox");
    input.setAttribute("aria-autocomplete", "list");
    input.setAttribute("aria-controls", id);
    input.setAttribute("aria-expanded", "false");
    input.autocomplete = "off"; // no browser suggestions over the list

    var s = {
      input: input,
      box: box,
      note: note,
      list: list,
      live: live,
      open: false,
      offer: null, // the server's answer shown
      value: "", // the field's text when it was asked
      caret: 0, // where the caret was then
      active: -1,
      timer: 0,
      sayTimer: 0,
      asked: 0, // requests sent; an answer to an older one is dropped
      abort: null,
    };
    input.stickpileSuggest = s;

    input.addEventListener("input", function () {
      ask(s);
    });
    input.addEventListener("focus", function () {
      ask(s);
    });
    input.addEventListener("click", function () {
      ask(s);
    });
    input.addEventListener("keyup", function (ev) {
      // The caret moved: the filter being typed may be another one.
      if (/^(ArrowLeft|ArrowRight|Home|End)$/.test(ev.key)) ask(s);
    });
    input.addEventListener("blur", function () {
      close(s);
    });
    input.addEventListener("keydown", function (ev) {
      keydown(s, ev);
    });
    // A click on the list must not take the focus from the field.
    box.addEventListener("mousedown", function (ev) {
      ev.preventDefault();
    });
    list.addEventListener("click", function (ev) {
      var o = ev.target instanceof Element && ev.target.closest("[role=option]");
      if (o && s.open) pick(s, +o.getAttribute("data-index"));
    });
    list.addEventListener("mousemove", function (ev) {
      var o = ev.target instanceof Element && ev.target.closest("[role=option]");
      if (o && +o.getAttribute("data-index") !== s.active) highlight(s, +o.getAttribute("data-index"));
    });
  }

  // Ask the server for the offers at the caret, once typing pauses.
  function ask(s) {
    clearTimeout(s.timer);
    var input = s.input;
    var caret = input.selectionStart;
    if (caret == null || caret !== input.selectionEnd || document.activeElement !== input) {
      close(s);
      return;
    }
    var before = input.value.slice(0, caret);
    // Only a `key:` token can be offered values (the server says which).
    var token = before.split(/\s/).pop();
    if (token.indexOf(":") < 0) {
      close(s);
      return;
    }
    s.timer = setTimeout(function () {
      fetchOffer(s, input.value, caret);
    }, DELAY);
  }

  function fetchOffer(s, value, caret) {
    if (s.abort) s.abort.abort();
    s.abort = typeof AbortController === "function" ? new AbortController() : null;
    var n = ++s.asked;
    var url =
      URL_SUGGEST + "?q=" + encodeURIComponent(value) + "&cursor=" + utf8Length(value.slice(0, caret));
    fetch(url, {
      credentials: "same-origin",
      headers: { Accept: "application/json" },
      signal: s.abort ? s.abort.signal : undefined,
    })
      .then(function (r) {
        if (!r.ok) throw new Error(String(r.status));
        return r.json();
      })
      .then(function (offer) {
        // Stale: another request went out, or the field changed meanwhile.
        if (n !== s.asked || s.input.value !== value || s.input.selectionStart !== caret) return;
        if (document.activeElement !== s.input || !offer || !offer.words || !offer.words.length) {
          close(s);
          return;
        }
        s.caret = caret;
        s.value = value;
        render(s, offer);
      })
      .catch(function () {
        /* offers are best effort: the search works without them */
        if (n === s.asked) close(s);
      });
  }

  function render(s, offer) {
    s.offer = offer;
    s.open = true;
    current = s;
    var frag = document.createDocumentFragment();
    offer.words.forEach(function (word, i) {
      var name = (offer.labels && offer.labels[i]) || "";
      var o = el("li", "emoji-option");
      o.id = s.list.id + "-" + i;
      o.setAttribute("role", "option");
      o.setAttribute("data-index", String(i));
      if (name) {
        // An emoji with its name: e:sk offers 💀 skull.
        o.title = name;
        o.appendChild(el("span", "emoji-option-char", word));
        o.appendChild(el("span", "emoji-option-name", name));
      } else {
        o.appendChild(el("span", "search-suggest-word", word));
      }
      frag.appendChild(o);
    });
    s.list.textContent = "";
    s.list.appendChild(frag);
    s.note.textContent = offer.key;
    s.box.hidden = false;
    s.input.setAttribute("aria-expanded", "true");
    highlight(s, offer.chosen == null ? -1 : offer.chosen);
    place(s);
    say(s, trp(offer.words.length, tr("{n} suggestion|{n} suggestions")));
  }

  function close(s) {
    clearTimeout(s.timer);
    if (!s.open) return;
    s.open = false;
    s.offer = null;
    s.active = -1;
    s.box.hidden = true;
    s.list.textContent = "";
    s.input.setAttribute("aria-expanded", "false");
    s.input.removeAttribute("aria-activedescendant");
    if (current === s) current = null;
  }

  function highlight(s, i) {
    var options = s.list.children;
    s.active = i;
    for (var k = 0; k < options.length; k++) options[k].setAttribute("aria-selected", k === i ? "true" : "false");
    if (i >= 0 && options[i]) s.input.setAttribute("aria-activedescendant", options[i].id);
    else s.input.removeAttribute("aria-activedescendant");
  }

  function keydown(s, ev) {
    if (!s.open || ev.isComposing || ev.keyCode === 229) return;
    var plain = !ev.ctrlKey && !ev.metaKey && !ev.altKey;
    var n = s.offer.words.length;
    // Typed on since the list was drawn (its answer is on the way): the
    // list is not what Enter or Tab would put in, so they do what they
    // do without it.
    var stale = s.input.value !== s.value || s.input.selectionStart !== s.caret;
    if (stale && (ev.key === "Enter" || ev.key === "Tab")) {
      close(s);
      return;
    }
    switch (ev.key) {
      case "ArrowDown":
      case "ArrowUp":
        if (!plain) return;
        ev.preventDefault();
        var down = ev.key === "ArrowDown";
        highlight(s, s.active < 0 ? (down ? 0 : n - 1) : (s.active + (down ? 1 : n - 1)) % n);
        return;
      case "Enter":
        // A highlighted value goes in; what is typed already is searched.
        if (!plain || s.active < 0 || s.offer.words[s.active] === s.offer.typed) {
          close(s);
          return;
        }
        ev.preventDefault();
        pick(s, s.active);
        return;
      case "Tab":
        // Tab puts a value in and stays in the field (Shift+Tab leaves).
        if (!plain || ev.shiftKey) {
          close(s);
          return;
        }
        ev.preventDefault();
        pick(s, s.active < 0 ? 0 : s.active);
        return;
      case "Escape":
        ev.preventDefault(); // a search field would clear itself
        close(s);
        return;
    }
  }

  // Put word `i` in place of the value typed (up to the end of its token)
  // with a space after it, the caret after them, and search with it.
  function pick(s, i) {
    var offer = s.offer;
    var word = offer && offer.words[i];
    close(s);
    if (word == null) return;
    var input = s.input;
    var v = input.value;
    var start = Math.max(0, s.caret - offer.typed.length);
    var end = s.caret;
    while (end < v.length && !/\s/.test(v.charAt(end))) end++;
    var after = v.slice(end).replace(/^\s+/, "");
    var text = word + " ";
    input.value = v.slice(0, start) + text + after;
    var caret = start + text.length;
    input.setSelectionRange(caret, caret);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  }

  // Under the field (above it when there is no room below), fixed, at
  // least as wide as the field.
  function place(s) {
    if (!s.open) return;
    if (!s.input.isConnected) {
      close(s);
      return;
    }
    var box = s.box;
    var f = s.input.getBoundingClientRect();
    var vw = document.documentElement.clientWidth;
    var vh = document.documentElement.clientHeight;
    box.style.setProperty("--emoji-field-width", f.width + "px");
    var w = box.offsetWidth;
    var h = box.offsetHeight;
    var x = Math.max(4, Math.min(f.left, vw - w - 4));
    var y = f.bottom + 3;
    if (y + h > vh - 4 && f.top - h - 3 >= 4) y = f.top - h - 3;
    box.style.left = x + "px";
    box.style.top = y + "px";
    box.classList.toggle("offscreen", f.bottom < 0 || f.top > vh);
  }

  // The count, once typing pauses.
  function say(s, text) {
    clearTimeout(s.sayTimer);
    s.sayTimer = setTimeout(function () {
      s.live.textContent = text;
    }, 500);
  }

  window.addEventListener(
    "scroll",
    function () {
      if (current) place(current);
    },
    true
  );
  window.addEventListener("resize", function () {
    if (current) place(current);
  });

  function enhance(root) {
    if (!root || !root.querySelectorAll) return;
    if (root.matches && root.matches("input[data-search-suggest]")) attach(root);
    root.querySelectorAll("input[data-search-suggest]").forEach(attach);
  }

  document.addEventListener("htmx:load", function (e) {
    enhance(e.target);
  });
  enhance(document);
})();
