// Small things for every page. The app works without this file.
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
