// Queue Plausible analytics calls until the vendor script arrives.
//
// This is the inline stub from Plausible's published snippet, served as a
// first-party file so it needs no per-response nonce. It runs as the first of
// two deferred scripts the render middleware emits (see
// `portal::plausible::PlausibleSite::script_tags`); deferred classic scripts
// execute in document order, so `init()` is queued before the vendor script
// runs and drains it.
window.plausible =
  window.plausible ||
  function () {
    (plausible.q = plausible.q || []).push(arguments);
  };
plausible.init =
  plausible.init ||
  function (options) {
    plausible.o = options || {};
  };
plausible.init();
