// In-place avatar upload for `/app/profile` — first-party, same-origin,
// zero telemetry.
//
// `FormCard` renders a native `<form multipart>` that would otherwise leave
// the profile page as a full navigation to `POST /app/avatar`. This
// intercepts that submit, replays it as `fetch` with the same `FormData`,
// and on success cache-busts every `/app/me/avatar` image on the page so
// the preview and the navbar update without leaving `/app/profile`.
//
// The listener is delegated on `document` (capture) so it still fires after
// Dioxus hydrates and replaces the form node. Inert unless the submit
// target sits inside `#profile-avatar`.
//
// Expected markup (see `webapp/src/profile.rs`):
//   <section id="profile-avatar">
//     <form action="/app/avatar" enctype="multipart/form-data">
//       <input type="file" id="profile-avatar-file" name="file">
//       <button type="submit">Upload</button>
//     </form>
//   </section>
(function () {
  "use strict";

  function statusFor(form) {
    var existing = form.querySelector("[data-avatar-upload-status]");
    if (existing) {
      return existing;
    }
    var status = document.createElement("p");
    status.className = "nav-muted";
    status.setAttribute("role", "status");
    status.setAttribute("aria-live", "polite");
    status.setAttribute("data-avatar-upload-status", "");
    form.appendChild(status);
    return status;
  }

  function refreshPreview() {
    var bust = "/app/me/avatar?v=" + Date.now();
    document.querySelectorAll('img[src^="/app/me/avatar"]').forEach(function (img) {
      img.setAttribute("src", bust);
    });
  }

  document.addEventListener(
    "submit",
    function (event) {
      var form = event.target;
      if (!(form instanceof HTMLFormElement)) {
        return;
      }
      if (!form.closest("#profile-avatar")) {
        return;
      }
      var input = form.querySelector("#profile-avatar-file, input[type='file']");
      if (!input || !input.files || input.files.length === 0) {
        return;
      }
      event.preventDefault();
      event.stopPropagation();

      var submitButton = form.querySelector('button[type="submit"]');
      var status = statusFor(form);
      status.hidden = false;
      status.textContent = "Uploading…";
      if (submitButton) {
        submitButton.disabled = true;
      }

      fetch(form.getAttribute("action") || "/app/avatar", {
        method: (form.getAttribute("method") || "POST").toUpperCase(),
        body: new FormData(form),
        credentials: "same-origin",
        headers: { "X-Requested-With": "XMLHttpRequest" },
      })
        .then(function (response) {
          if (!response.ok) {
            throw new Error(String(response.status));
          }
          refreshPreview();
          status.textContent = "Avatar updated.";
          input.value = "";
          if (submitButton) {
            submitButton.disabled = false;
          }
        })
        .catch(function () {
          status.textContent = "Upload failed. Please try again.";
          if (submitButton) {
            submitButton.disabled = false;
          }
        });
    },
    true
  );
})();
