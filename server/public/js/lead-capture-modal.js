// Progressive enhancement for the Neon home-page lead form.
//
// The trigger remains a normal /contact link when this script is unavailable.
// Native <dialog> supplies the modal's inert background and Escape behaviour;
// this layer adds focus placement and explicit focus restoration.
(function () {
  "use strict";

  function init() {
    var dialog = document.querySelector("[data-lead-modal]");
    var trigger = document.querySelector("[data-lead-modal-trigger]");
    var closeButton = dialog && dialog.querySelector("[data-lead-modal-close]");

    if (!dialog || !trigger || !closeButton || typeof dialog.showModal !== "function") {
      return;
    }

    var lastFocused = null;

    function restoreFocus() {
      if (lastFocused && typeof lastFocused.focus === "function") {
        lastFocused.focus();
      }
      lastFocused = null;
    }

    function close() {
      if (!dialog.open) {
        return;
      }
      dialog.close();
      restoreFocus();
    }

    trigger.addEventListener("click", function (event) {
      event.preventDefault();
      lastFocused = trigger;
      dialog.showModal();
      closeButton.focus();
    });

    closeButton.addEventListener("click", close);

    dialog.addEventListener("cancel", function (event) {
      event.preventDefault();
      close();
    });

    dialog.addEventListener("click", function (event) {
      if (event.target === dialog) {
        close();
      }
    });
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();
