// Real-time upload progress for the document-upload form — first-party,
// same-origin, zero telemetry.
//
// `FormCard` renders a native `<form multipart>` that would otherwise submit
// as a blocking full-page navigation, with no feedback while a large PDF
// batch is in flight. This intercepts that one submit, replays it as an
// `XMLHttpRequest` carrying the same `FormData`, and renders `upload.loaded`
// / `upload.total` (the browser's own real-time byte counters) as a bar and
// a label. On success it follows the handler's redirect exactly as the
// native form would have; on failure it re-enables the form so the lawyer
// can retry.
//
// Inert unless `#document-upload-file` is on the page, so it is safe to load
// on any page — like the other first-party scripts.
//
// Expected markup (see `webapp/src/lawyer_project_detail.rs`):
//   <form>
//     <input type="file" id="document-upload-file" name="file" multiple>
//     <button type="submit">Upload</button>
//   </form>
(function () {
  "use strict";

  function formatBytes(n) {
    if (n >= 1024 * 1024) {
      return (n / (1024 * 1024)).toFixed(1) + " MB";
    }
    if (n >= 1024) {
      return (n / 1024).toFixed(0) + " KB";
    }
    return n + " B";
  }

  function init() {
    var input = document.getElementById("document-upload-file");
    if (!input) {
      return;
    }
    var form = input.closest("form");
    if (!form) {
      return;
    }
    var submitButton = form.querySelector('button[type="submit"]');

    var bar = document.createElement("div");
    bar.className = "upload-progress";
    bar.hidden = true;
    bar.innerHTML =
      '<div class="upload-progress__track">' +
      '<div class="upload-progress__fill"></div>' +
      "</div>" +
      '<p class="upload-progress__label" role="status" aria-live="polite"></p>';
    form.appendChild(bar);
    var fill = bar.querySelector(".upload-progress__fill");
    var label = bar.querySelector(".upload-progress__label");

    form.addEventListener("submit", function (event) {
      if (!input.files || input.files.length === 0) {
        return;
      }
      event.preventDefault();

      var totalBytes = 0;
      for (var i = 0; i < input.files.length; i++) {
        totalBytes += input.files[i].size;
      }

      var formData = new FormData(form);
      var xhr = new XMLHttpRequest();
      xhr.open(form.getAttribute("method") || "POST", form.getAttribute("action"), true);

      bar.hidden = false;
      fill.style.width = "0%";
      if (submitButton) {
        submitButton.disabled = true;
      }
      label.textContent = "Uploading 0 B / " + formatBytes(totalBytes);

      xhr.upload.addEventListener("progress", function (progressEvent) {
        if (!progressEvent.lengthComputable) {
          return;
        }
        var pct = Math.round((progressEvent.loaded / progressEvent.total) * 100);
        fill.style.width = pct + "%";
        label.textContent =
          "Uploading " +
          formatBytes(progressEvent.loaded) +
          " / " +
          formatBytes(progressEvent.total) +
          " (" +
          pct +
          "%)";
      });

      xhr.addEventListener("load", function () {
        if (xhr.status >= 200 && xhr.status < 400) {
          fill.style.width = "100%";
          label.textContent = "Upload complete — refreshing…";
          window.location.href = xhr.responseURL || window.location.href;
        } else {
          label.textContent = "Upload failed (" + xhr.status + "). Please try again.";
          if (submitButton) {
            submitButton.disabled = false;
          }
        }
      });

      xhr.addEventListener("error", function () {
        label.textContent = "Upload failed — network error. Please try again.";
        if (submitButton) {
          submitButton.disabled = false;
        }
      });

      xhr.send(formData);
    });
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();
