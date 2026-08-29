// document-review — first-party custom element for the
// comment-only document review surface (Phase A).
//
// It upgrades a server-rendered, read-only document into a
// select-text-and-comment surface WITHOUT a heavy editor framework: the
// browser's native Selection API gives us the selected text and its
// offsets, the CSS Custom Highlight API paints existing comment ranges,
// and comments round-trip to the server as form-encoded POSTs (so the
// existing /app CSRF middleware guards them).
//
// The document stays read-only — the only thing the reader writes is a
// comment. Anchors are character offsets into the document's text
// content (`anchor_start`/`anchor_end`), which is engine-independent: if
// we later swap the read surface for a richer editor, the server's
// comment contract is unchanged.
//
// Expected markup (rendered by webapp::review):
//
//   <document-review
//     data-create-url="/app/projects/:code/review/:doc/comments"
//     data-comments='[…json…]'
//     data-csrf="…">
//     <article class="nr-document">…document html…</article>
//     <aside class="nr-sidebar"></aside>
//   </document-review>

(function () {
  "use strict";

  const HIGHLIGHT_NAME = "nr-comment";

  class DocumentReview extends HTMLElement {
    connectedCallback() {
      if (this._wired) return;
      this._wired = true;

      this.doc = this.querySelector(".nr-document");
      this.sidebar = this.querySelector(".nr-sidebar");
      if (!this.doc || !this.sidebar) return;

      this.createUrl = this.dataset.createUrl || "";
      this.csrf = this.dataset.csrf || "";
      try {
        this.comments = JSON.parse(this.dataset.comments || "[]");
      } catch (_e) {
        this.comments = [];
      }

      this.buildSidebarShell();
      this.renderComments();
      this.paintHighlights();

      // A selection inside the document offers an "Add comment" affordance.
      this.doc.addEventListener("mouseup", () => this.onSelection());
      this.doc.addEventListener("keyup", () => this.onSelection());
    }

    buildSidebarShell() {
      this.sidebar.innerHTML = "";
      const h = document.createElement("h2");
      h.className = "h6 text-uppercase text-body-secondary mb-3";
      h.textContent = "Comments";
      this.sidebar.appendChild(h);

      this.list = document.createElement("div");
      this.list.className = "nr-comment-list d-grid gap-2";
      this.sidebar.appendChild(this.list);

      // Floating composer, hidden until a selection exists.
      this.composer = document.createElement("form");
      this.composer.className = "nr-composer card p-2 mt-3";
      this.composer.hidden = true;
      this.composer.innerHTML =
        '<p class="small text-body-secondary mb-1">Commenting on: ' +
        '<span class="nr-quote fst-italic"></span></p>' +
        '<textarea class="form-control form-control-sm mb-2" rows="3" ' +
        'placeholder="Your comment" required></textarea>' +
        '<div class="d-flex gap-2">' +
        '<button type="submit" class="btn btn-sm btn-primary">Add comment</button>' +
        '<button type="button" class="btn btn-sm btn-link nr-cancel">Cancel</button>' +
        "</div>";
      this.sidebar.appendChild(this.composer);

      this.composer.querySelector(".nr-cancel").addEventListener("click", () => {
        this.clearPending();
      });
      this.composer.addEventListener("submit", (e) => {
        e.preventDefault();
        this.submitComment();
      });
    }

    // Map the current document selection to character offsets into the
    // document's text content. Returns null when the selection is empty
    // or lands outside the document.
    onSelection() {
      const sel = window.getSelection();
      if (!sel || sel.isCollapsed || sel.rangeCount === 0) return;
      const range = sel.getRangeAt(0);
      if (!this.doc.contains(range.commonAncestorContainer)) return;

      const start = this.offsetOf(range.startContainer, range.startOffset);
      const end = this.offsetOf(range.endContainer, range.endOffset);
      if (start == null || end == null || end <= start) return;

      this.pending = { start, end, text: sel.toString() };
      this.composer.hidden = false;
      this.composer.querySelector(".nr-quote").textContent = truncate(sel.toString(), 120);
      this.composer.querySelector("textarea").focus();
    }

    clearPending() {
      this.pending = null;
      this.composer.hidden = true;
      this.composer.querySelector("textarea").value = "";
    }

    // Character offset of (node, nodeOffset) within the document text.
    offsetOf(node, nodeOffset) {
      const walker = document.createTreeWalker(this.doc, NodeFilter.SHOW_TEXT, null);
      let total = 0;
      let current;
      while ((current = walker.nextNode())) {
        if (current === node) return total + nodeOffset;
        total += current.textContent.length;
      }
      // Selection endpoint on an element boundary: best-effort fallback.
      if (node === this.doc) return total;
      return null;
    }

    // Resolve a character offset back to a (textNode, offset) DOM point.
    pointAt(offset) {
      const walker = document.createTreeWalker(this.doc, NodeFilter.SHOW_TEXT, null);
      let total = 0;
      let current;
      while ((current = walker.nextNode())) {
        const len = current.textContent.length;
        if (offset <= total + len) {
          return { node: current, offset: offset - total };
        }
        total += len;
      }
      return null;
    }

    rangeFor(start, end) {
      const a = this.pointAt(start);
      const b = this.pointAt(end);
      if (!a || !b) return null;
      const range = document.createRange();
      range.setStart(a.node, a.offset);
      range.setEnd(b.node, b.offset);
      return range;
    }

    paintHighlights() {
      // CSS Custom Highlight API — paints ranges without mutating the
      // document DOM. Absent in older browsers; comments still list in
      // the sidebar, so this is purely a visual enhancement.
      if (!("highlights" in CSS) || typeof Highlight === "undefined") return;
      const ranges = [];
      for (const c of this.comments) {
        if (c.resolved) continue;
        const r = this.rangeFor(c.anchor_start, c.anchor_end);
        if (r) ranges.push(r);
      }
      try {
        CSS.highlights.set(HIGHLIGHT_NAME, new Highlight(...ranges));
      } catch (_e) {
        /* ignore — visual only */
      }
    }

    renderComments() {
      this.list.innerHTML = "";
      if (this.comments.length === 0) {
        const empty = document.createElement("p");
        empty.className = "text-body-secondary small mb-0";
        empty.textContent = "No comments yet. Select text in the document to add one.";
        this.list.appendChild(empty);
        return;
      }
      for (const c of this.comments) {
        const card = document.createElement("div");
        card.className = "nr-comment card p-2" + (c.resolved ? " opacity-50" : "");
        const quote = document.createElement("p");
        quote.className = "small fst-italic text-body-secondary mb-1";
        quote.textContent = "“" + truncate(c.quoted_text, 100) + "”";
        const body = document.createElement("p");
        body.className = "mb-1";
        body.textContent = c.body;
        const meta = document.createElement("p");
        meta.className = "small text-body-secondary mb-0";
        meta.textContent = (c.author || "You") + (c.resolved ? " · resolved" : "");
        card.append(quote, body, meta);
        card.addEventListener("click", () => this.flash(c));
        this.list.appendChild(card);
      }
    }

    flash(comment) {
      const r = this.rangeFor(comment.anchor_start, comment.anchor_end);
      if (!r) return;
      const rect = r.getBoundingClientRect();
      window.scrollTo({ top: window.scrollY + rect.top - 120, behavior: "smooth" });
    }

    async submitComment() {
      if (!this.pending) return;
      const body = this.composer.querySelector("textarea").value.trim();
      if (!body) return;
      const params = new URLSearchParams();
      params.set("_csrf", this.csrf);
      params.set("anchor_start", String(this.pending.start));
      params.set("anchor_end", String(this.pending.end));
      params.set("quoted_text", this.pending.text);
      params.set("body", body);

      const btn = this.composer.querySelector('button[type="submit"]');
      btn.disabled = true;
      try {
        const res = await fetch(this.createUrl, {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          credentials: "same-origin",
          body: params.toString(),
        });
        if (!res.ok) throw new Error("HTTP " + res.status);
        this.comments = await res.json();
        this.clearPending();
        this.renderComments();
        this.paintHighlights();
      } catch (e) {
        btn.disabled = false;
        const err = this.composer.querySelector(".nr-error") || document.createElement("p");
        err.className = "nr-error small text-danger mb-0 mt-1";
        err.textContent = "Couldn't save your comment. Please try again.";
        this.composer.appendChild(err);
      }
    }
  }

  function truncate(s, n) {
    s = s || "";
    return s.length > n ? s.slice(0, n - 1) + "…" : s;
  }

  if (!customElements.get("document-review")) {
    customElements.define("document-review", DocumentReview);
  }
})();
