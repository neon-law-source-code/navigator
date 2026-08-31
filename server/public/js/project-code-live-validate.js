// project-code-live-validate — live shape feedback for the matter-open
// "Project code" field (`/app/projects/new`).
//
// A Project code is chosen once and never changes (`docs/glossary.md#project`),
// so a shape mistake caught only after a full-page POST/redirect/GET round
// trip is a worse experience here than on an ordinary field. This mirrors
// `cloud::workspace::is_valid_slug` and `RESERVED_PROJECT_CODES` exactly —
// lowercase letters, digits, and single hyphens; alphanumeric at both ends; no
// `--`; not `new` or `navigator` — as pure client-side checks, so feedback is
// instant and needs no round trip to the server. It never blocks or alters
// submission: the server (`store::projects::is_valid_code`, then the engine)
// remains the sole authority, and a JavaScript failure here leaves the plain
// native form exactly as it was.
//
// Suffix note: Navigator appends a generated 8-letter suffix
// (`store::projects::code_from_name`) that depends on the matter's id, which
// does not exist until the row is created — so the preview below shows an
// illustrative example suffix, never a real one.

(function () {
  "use strict";

  const RESERVED = ["navigator", "new"];
  const EXAMPLE_SUFFIX = "a1b2c3d4";

  // Mirrors `cloud::workspace::is_valid_slug` byte for byte: non-empty, at
  // most 80 characters, only lowercase ASCII letters/digits/hyphens,
  // alphanumeric at both ends, no `--`.
  function shapeProblem(value) {
    if (value.length > 80) return "must be at most 80 characters";
    if (!/^[a-z0-9-]+$/.test(value)) {
      return "can only use lowercase letters, digits, and hyphens";
    }
    if (!/^[a-z0-9]/.test(value)) return "must start with a letter or digit";
    if (!/[a-z0-9]$/.test(value)) return "must end with a letter or digit";
    if (value.includes("--")) return "can't have two hyphens in a row";
    return null;
  }

  function problemFor(value) {
    if (value === "") return null; // untouched — nothing to say yet
    const shape = shapeProblem(value);
    if (shape) return "Not yet valid: " + shape + ".";
    if (RESERVED.indexOf(value) !== -1) {
      return "`" + value + "` is reserved and can't be used as a code stem.";
    }
    return null;
  }

  function wire() {
    const input = document.getElementById("code");
    if (!input || input.dataset.liveValidateWired) return;
    input.dataset.liveValidateWired = "1";

    const status = document.createElement("p");
    status.id = "code-live-status";
    status.className = "nav-field__help";
    status.setAttribute("aria-live", "polite");
    const wrapper = input.closest(".nav-field") || input.parentElement;
    wrapper.appendChild(status);

    // Announce the live status alongside the field's existing description,
    // rather than replacing it (the shape/consequence copy stays reachable).
    const describedBy = (input.getAttribute("aria-describedby") || "").split(/\s+/).filter(Boolean);
    describedBy.push(status.id);
    input.setAttribute("aria-describedby", describedBy.join(" "));

    function render() {
      const value = input.value.trim().toLowerCase();
      const problem = problemFor(value);
      if (problem) {
        status.textContent = problem;
        status.classList.add("nav-field__help--live-invalid");
        input.setAttribute("aria-invalid", "true");
      } else {
        input.removeAttribute("aria-invalid");
        status.classList.remove("nav-field__help--live-invalid");
        status.textContent = value === "" ? "" : "Your matter's code will be `" + value + "-" + EXAMPLE_SUFFIX + "` (Navigator generates the real suffix when the matter opens).";
      }
    }

    input.addEventListener("input", render);
    render();
  }

  wire();
})();
