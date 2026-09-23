// copy-code.js — one-click copy for every rendered code block.
//
// The button is server HTML: `button[data-copy-code]` inside `.nav-code`.
// This file only copies. Pages load it when they render a block (the Dioxus
// component hoists one script tag; markdown HTML gets the same tag from the
// document middleware). `script-src` forbids an inline handler, so the
// listener is delegated from the document.
//
// With this script unavailable the block is still the code it always was.

(function () {
  "use strict";

  if (window.__navCopyCode) {
    return;
  }
  window.__navCopyCode = true;

  var resetTimers = new WeakMap();

  function sourceText(block) {
    var code = block.querySelector("pre code") || block.querySelector("code");
    var root = code || block.querySelector("pre");
    if (!root) {
      return "";
    }
    // Highlighted blocks are sibling <span>s. syntect also emits whitespace
    // text nodes to indent that HTML; those are not part of the source, so
    // only element children are copied. A plain block has no spans and its
    // text node is the source.
    if (!root.querySelector("span")) {
      return root.textContent.replace(/\u00a0/g, " ");
    }
    var text = "";
    var child = root.firstChild;
    while (child) {
      if (child.nodeType === 1) {
        text += child.textContent;
      }
      child = child.nextSibling;
    }
    return text.replace(/\u00a0/g, " ");
  }

  function mark(button, label, text) {
    button.textContent = text;
    button.setAttribute("aria-label", label);
  }

  function flashCopied(button) {
    var previousLabel = button.getAttribute("data-copy-label") || "Copy code";
    if (!button.hasAttribute("data-copy-label")) {
      button.setAttribute("data-copy-label", previousLabel);
    }
    mark(button, "Copied", "Copied");
    button.setAttribute("data-copied", "true");
    var pending = resetTimers.get(button);
    if (pending) {
      window.clearTimeout(pending);
    }
    resetTimers.set(
      button,
      window.setTimeout(function () {
        mark(button, button.getAttribute("data-copy-label") || "Copy code", "Copy");
        button.removeAttribute("data-copied");
        resetTimers.delete(button);
      }, 2000)
    );
  }

  function fallbackCopy(text) {
    var area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.top = "0";
    area.style.left = "-9999px";
    document.body.appendChild(area);
    area.focus();
    area.select();
    var ok = false;
    try {
      ok = document.execCommand("copy");
    } catch (err) {
      ok = false;
    }
    document.body.removeChild(area);
    return ok;
  }

  document.addEventListener("click", function (event) {
    var target = event.target;
    if (!target || typeof target.closest !== "function") {
      return;
    }
    var button = target.closest("[data-copy-code]");
    if (!button) {
      return;
    }
    var block = button.closest(".nav-code");
    if (!block) {
      return;
    }
    var text = sourceText(block);
    function done() {
      flashCopied(button);
    }
    if (navigator.clipboard && typeof navigator.clipboard.writeText === "function") {
      navigator.clipboard.writeText(text).then(done, function () {
        if (fallbackCopy(text)) {
          done();
        }
      });
      return;
    }
    if (fallbackCopy(text)) {
      done();
    }
  });
})();
