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
// * Drag-and-drop onto upload forms ([data-dropzone]); the plain file
//   input keeps working without this script.
// * Escape cancels inline editing; dismiss buttons; copy buttons for
//   one-time secrets (code.copyable).
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
    root.querySelectorAll("[data-dropzone]").forEach(setupDropzone);
  }

  function setupDropzone(zone) {
    if (zone.getAttribute("data-ready")) return;
    zone.setAttribute("data-ready", "1");
    var input = zone.querySelector("input[type=file]");
    var form = zone.closest("form");
    if (!input || !form) return;
    zone.classList.add("js");
    ["dragenter", "dragover"].forEach(function (t) {
      zone.addEventListener(t, function (e) {
        e.preventDefault();
        zone.classList.add("over");
      });
    });
    ["dragleave", "dragend"].forEach(function (t) {
      zone.addEventListener(t, function () {
        zone.classList.remove("over");
      });
    });
    zone.addEventListener("drop", function (e) {
      e.preventDefault();
      zone.classList.remove("over");
      if (!e.dataTransfer || !e.dataTransfer.files || !e.dataTransfer.files.length) return;
      input.files = e.dataTransfer.files;
      input.dispatchEvent(new Event("change", { bubbles: true }));
      if (form.requestSubmit) form.requestSubmit();
      else form.submit();
    });
    input.addEventListener("change", function () {
      var n = input.files ? input.files.length : 0;
      zone.setAttribute("data-count", n ? n + (n === 1 ? " file selected" : " files selected") : "");
    });
  }

  // ---- global listeners ------------------------------------------------

  document.addEventListener("click", function (e) {
    var t = e.target;
    if (!(t instanceof Element)) return;
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

  document.addEventListener("keydown", function (e) {
    if (e.key !== "Escape") return;
    var t = e.target;
    if (!(t instanceof Element)) return;
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
