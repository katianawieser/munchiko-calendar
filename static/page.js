// Small things for every page. The app works without this file.
//
// "Tick all" boxes: a checkbox with class "toggle-all" and data-boxes="from"
// ticks or unticks every checkbox named "from" in its form, and shows a dash
// (indeterminate) when only some are ticked. Hidden until this script runs.
//
// "↑ Top" is a plain link to the top of the page. It starts hidden (so it
// doesn't flash and fade out on every page load), and this shows it once the
// page has been scrolled down a screen, so short pages never show it.
(function () {
  "use strict";
  const button = document.querySelector(".back-to-top");
  if (!button) return;
  const update = () => {
    button.classList.toggle("hidden", window.scrollY < window.innerHeight);
  };
  update();
  window.addEventListener("scroll", update, { passive: true });
})();

(function () {
  "use strict";
  document.querySelectorAll("input.toggle-all").forEach((all) => {
    const form = all.closest("form");
    const boxes = () => [...form.querySelectorAll(`input[type="checkbox"][name="${all.dataset.boxes}"]`)];
    const sync = () => {
      const ticked = boxes().filter((b) => b.checked).length;
      all.checked = ticked === boxes().length;
      all.indeterminate = ticked > 0 && ticked < boxes().length;
    };
    all.hidden = false;
    all.addEventListener("change", () => {
      boxes().forEach((b) => (b.checked = all.checked));
      all.indeterminate = false;
    });
    boxes().forEach((b) => b.addEventListener("change", sync));
    sync();
  });
})();
