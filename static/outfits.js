// The post form's outfits, made quicker with JavaScript. Without this file
// the form still works: the server adds a newly ticked outfit's pieces when
// it's saved. With it, ticking an outfit ticks its pieces straight away, so
// one can be unticked before saving ("no wand this time"). The hidden
// "outfits_live" field tells the server the pieces in the form are already
// what's wanted, so it doesn't add them back.
(function () {
  "use strict";

  function setUp() {
    const live = document.querySelector('input[name="outfits_live"]');
    if (!live) return;
    live.value = "1";

    document.querySelectorAll("input.outfit-choice").forEach((box) => {
      box.addEventListener("change", () => {
        // Unticking an outfit only forgets it; its pieces stay as they are.
        if (!box.checked) return;
        const field = box.dataset.field;
        const ids = (box.dataset.pieces || "").split(",").filter(Boolean);
        for (const id of ids) {
          const piece = document.querySelector(
            `input[type="checkbox"][name="${field}"][value="${id}"]`
          );
          if (piece) piece.checked = true;
        }
      });
    });
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", setUp);
  } else {
    setUp();
  }
})();
