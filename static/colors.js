// Color rows on the fabric and accessory forms (templates/color_fields.html).
//
// Progressive enhancement: the form works without this script (names only).
// With it, each row gets a swatch picker, and colors can be added/removed.
//
// A color input always holds *some* color (white by default), so it can't
// say "no swatch". The picked value is therefore copied into a hidden
// color_hex field, which stays empty until the user really picks a shade.
"use strict";

document.querySelectorAll(".color-list").forEach((list) => {
  const rows = list.querySelector(".color-rows");
  const template = list.querySelector("template");
  const addButton = list.querySelector(".add-color");

  function wire(row) {
    const picker = row.querySelector(".swatch-picker");
    const hex = row.querySelector('input[name="color_hex"]');
    const clear = row.querySelector(".clear-swatch");
    const remove = row.querySelector(".remove-color");

    // Faded picker + no "No swatch" button = nothing picked yet.
    const show = () => {
      picker.classList.toggle("unset", hex.value === "");
      clear.hidden = hex.value === "";
    };
    const pick = () => {
      hex.value = picker.value;
      show();
    };
    picker.addEventListener("input", pick);
    picker.addEventListener("change", pick);
    clear.addEventListener("click", () => {
      hex.value = "";
      show();
    });
    remove.addEventListener("click", () => row.remove());

    picker.hidden = false;
    remove.hidden = false;
    show();
  }

  rows.querySelectorAll(".color-row").forEach(wire);
  addButton.hidden = false;
  addButton.addEventListener("click", () => {
    const row = template.content.firstElementChild.cloneNode(true);
    rows.append(row);
    wire(row);
    row.querySelector('input[name="color_name"]').focus();
  });
});
