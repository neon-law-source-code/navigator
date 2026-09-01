// Harvard-outline narration stage.
//
// Upgrades a server-rendered article[data-harvard-outline] into a keyboard-
// and click-driven highlighter. Inert on every page without that root. There
// is no network: the current unit is a class on the section, so a screencast
// records only the document.

(function () {
  "use strict";

  var root = document.querySelector("[data-harvard-outline]");
  if (!root) {
    return;
  }

  var units = Array.prototype.slice.call(
    root.querySelectorAll(".harvard-unit[data-harvard-index]")
  );
  if (units.length === 0) {
    return;
  }

  var counter = root.querySelector("[data-harvard-counter]");
  var current = 0;
  var params = new URLSearchParams(window.location.search);
  var start = parseInt(params.get("i") || "0", 10);
  if (!isNaN(start) && start >= 0 && start < units.length) {
    current = start;
  }

  function paint() {
    units.forEach(function (unit, index) {
      var on = index === current;
      unit.classList.toggle("is-current", on);
      if (on) {
        unit.setAttribute("aria-current", "true");
      } else {
        unit.removeAttribute("aria-current");
      }
    });
    if (counter) {
      var unit = units[current];
      var path = unit.getAttribute("data-harvard-path") || "";
      var label = path ? " · " + path : "";
      counter.textContent = "§ " + (current + 1) + " / " + units.length + label;
    }
    units[current].scrollIntoView({ block: "center", behavior: "smooth" });
  }

  function step(delta) {
    var next = current + delta;
    if (next < 0 || next >= units.length) {
      return false;
    }
    current = next;
    paint();
    units[current].focus({ preventScroll: true });
    return true;
  }

  units.forEach(function (unit, index) {
    unit.addEventListener("click", function () {
      current = index;
      paint();
    });
  });

  document.addEventListener("keydown", function (event) {
    if (
      event.defaultPrevented ||
      event.metaKey ||
      event.ctrlKey ||
      event.altKey
    ) {
      return;
    }
    var target = event.target;
    if (
      target &&
      (target.isContentEditable ||
        /^(BUTTON|INPUT|SELECT|TEXTAREA)$/.test(target.tagName))
    ) {
      return;
    }
    switch (event.key) {
      case "ArrowDown":
      case "ArrowRight":
      case "n":
      case "N":
      case "PageDown":
      case " ":
      case "Spacebar":
        if (step(1)) {
          event.preventDefault();
        }
        break;
      case "ArrowUp":
      case "ArrowLeft":
      case "p":
      case "P":
      case "PageUp":
        if (step(-1)) {
          event.preventDefault();
        }
        break;
      case "Home":
        current = 0;
        paint();
        event.preventDefault();
        break;
      case "End":
        current = units.length - 1;
        paint();
        event.preventDefault();
        break;
      case "h":
      case "H":
        document.body.classList.toggle("harvard-stage-recording");
        break;
    }
  });

  paint();
})();
